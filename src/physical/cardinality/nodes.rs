//! \[DESIGN\] The rule of each node (RULES.md D370), every one at least one
//! row when its input has one:
//!
//! * a scan, its table's live rows; a list of rows, how many;
//! * a filter, its input's rows times its conjuncts' selectivities, each
//!   over the base table its columns come from (`cardinality/lineage.rs`),
//!   an equality of two relations' columns by classes, a filter over a
//!   product as the join it is (`cardinality/joins.rs`);
//! * an operand moved over a guard (`optimizer::is_guarded`), the join of
//!   it and the guard's one row, so its own rows times its own conjuncts':
//!   the guard's EXISTS keeps that row or makes none of the statement's,
//!   and no conjunct relates the two, so nothing takes a third of them;
//! * a product, the product; an outer join, the inner join's rows, at
//!   least each preserved operand's; a semi join and an antijoin, the
//!   share of the left's rows `cardinality/semi.rs` gives;
//! * a grouping and DISTINCT, their groups (`cardinality/groups.rs`);
//! * UNION ALL, the sum; UNION, the groups of the sum; INTERSECT, at most
//!   the smaller operand; EXCEPT, at most the left;
//! * a limit, its input's past its offset, at most its count;
//! * a projection, a sort, a window and a pattern recognition, their
//!   input's; an UNNEST, its input's times `UNNEST_FANOUT`;
//! * what has no count, the defaults of `cardinality.rs`.

use super::joins::{columns_of, is_chain};
use super::lineage::{Over, one_table, origin};
use super::{Card, Estimates, FUNCTION_ROWS, MAX_ESTIMATE_DEPTH, RECURSIVE_ROWS, UNKNOWN_ROWS, UNNEST_FANOUT, at_least_one};
use crate::logical::{Cond, Expr, Limit, LogicalPlan, OuterJoin, SetOp, map_cond_columns, split_and};
use crate::physical::estimate::{Known, selectivity};
use crate::storage::cost_of;
use crate::value::Value;

impl Estimates<'_> {
    /// The card of `plan`, `depth` levels below the node first asked, as
    /// kept this asking or found now and kept.
    pub(super) fn card_at(&self, plan: &LogicalPlan, depth: usize) -> Card {
        if depth > MAX_ESTIMATE_DEPTH {
            return Card::guess(UNKNOWN_ROWS);
        }
        let at = std::ptr::from_ref(plan).addr();
        if let Some(card) = self.kept_card(at) {
            return card;
        }
        // [EXTENSION] the rows given in its place, if any are (RULES.md D377).
        let card = self.injected(plan).unwrap_or_else(|| self.found(plan, depth));
        self.keep_card(at, card);
        card
    }

    /// The card of `plan` by its node's rule.
    fn found(&self, plan: &LogicalPlan, depth: usize) -> Card {
        let next = depth.saturating_add(1);
        match plan {
            LogicalPlan::Scan { table, .. } => self.table_rows(table).map_or(Card::guess(UNKNOWN_ROWS), Card::exact),
            LogicalPlan::Filter { cond, input } => self.filter_at((cond, input), next),
            LogicalPlan::Product { .. } => self.chain(None, plan, depth),
            LogicalPlan::OuterJoin { kind, cond, left, right } => self.outer_at((*kind, cond), (left, right), next),
            LogicalPlan::SemiJoin { anti, cond, left, right } => self.semi_at((*anti, cond), (left, right), next),
            LogicalPlan::Aggregate { groups, seeds, input, .. } => self.grouped(groups, seeds.len(), input, next),
            LogicalPlan::Distinct { input } => self.distinct_at(input, next),
            LogicalPlan::SetOp { op, all, left, right, .. } => self.combined((*op, *all), (left, right), next),
            LogicalPlan::Limit { limit, input } => limited(limit, self.card_at(input, next)),
            LogicalPlan::Project { input, .. } | LogicalPlan::Sort { input, .. } => self.card_at(input, next),
            LogicalPlan::Window(w) => self.card_at(&w.input, next),
            LogicalPlan::Recognize(r) => self.card_at(&r.input, next),
            LogicalPlan::Values { rows, .. } => Card::exact(cost_of(rows.len())),
            LogicalPlan::Unnest(u) => Card::guess(self.card_at(&u.input, next).rows * UNNEST_FANOUT),
            LogicalPlan::Recursive { .. } | LogicalPlan::WorkTable { .. } => Card::guess(RECURSIVE_ROWS),
            LogicalPlan::Polymorphic(..) => Card::guess(FUNCTION_ROWS),
            LogicalPlan::System { .. } | LogicalPlan::Transition { .. } => Card::guess(UNKNOWN_ROWS),
        }
    }

    /// The card of a filter of `cond` over `input`.
    pub(in crate::physical) fn filter_card(&self, cond: &Cond, input: &LogicalPlan) -> Card {
        self.asked(|| self.filter_at((cond, input), 1))
    }

    /// The card of the filter of `cond` over `input`: over a chain, the
    /// join's, a guarded operand's among them; else its input's kept by
    /// each conjunct.
    fn filter_at(&self, (cond, input): (&Cond, &LogicalPlan), depth: usize) -> Card {
        if is_chain(input) {
            return self.chain(Some(cond), input, depth);
        }
        self.card_at(input, depth).kept(self.selectivity(cond, Over::One(input)))
    }

    /// The card of DISTINCT of `input`: the groups of all its columns.
    pub(in crate::physical) fn distinct_card(&self, input: &LogicalPlan) -> Card {
        self.asked(|| self.distinct_at(input, 1))
    }

    /// See `distinct_card`.
    fn distinct_at(&self, input: &LogicalPlan, depth: usize) -> Card {
        let all: Vec<usize> = (0..input.width()).collect();
        self.grouped(&all, 0, input, depth)
    }

    /// The fraction of the rows `cond` keeps, over `over`'s columns, the
    /// conjuncts of each table together (`cardinality/tables.rs`).
    pub(super) fn selectivity(&self, cond: &Cond, over: Over<'_>) -> f64 {
        let mut conjuncts = Vec::new();
        split_and(self.valued(cond).into_owned(), &mut conjuncts);
        self.conjunction(&conjuncts, over)
    }

    /// The fraction of the rows the conjunct `c`, over `over`'s columns,
    /// keeps: by the statistics of the one table its columns come from,
    /// if one; of an equality of two relations' columns, by their distinct
    /// values; else by the defaults.
    pub(super) fn share(&self, c: &Cond, over: Over<'_>) -> f64 {
        let columns = columns_of(c);
        if !columns.is_empty()
            && let Some((table, places)) = one_table(over, &columns)
        {
            let stats = self.statistics(table);
            let local = map_cond_columns(c.clone(), &|col| places.iter().find(|&&(at, _)| at == col).map_or(col, |&(_, place)| place));
            return selectivity(&local, Known { stats: stats.as_deref(), cost: self.cost() });
        }
        self.equality_share(over, c).unwrap_or_else(|| selectivity(c, Known { stats: None, cost: self.cost() }))
    }

    /// The distinct values of `plan`'s column `column`: as its table's
    /// statistics count them, else its table's rows, as if a key; of a
    /// column of no table, `plan`'s rows.
    pub(in crate::physical) fn distinct(&self, plan: &LogicalPlan, column: usize) -> f64 {
        let Some(found) = origin(plan, column) else { return self.rows(plan).max(1.0_f64) };
        let counted = self.statistics(found.table).and_then(|s| s.distinct(found.column));
        counted.or_else(|| self.table_rows(found.table)).unwrap_or(UNKNOWN_ROWS)
    }

    /// The card of an outer join of `kind` on `cond` of `left` and `right`.
    pub(in crate::physical) fn outer_card(&self, (kind, cond): (OuterJoin, &Cond), (left, right): (&LogicalPlan, &LogicalPlan)) -> Card {
        self.asked(|| self.outer_at((kind, cond), (left, right), 1))
    }

    /// See `outer_card`: the inner join's rows, at least each preserved
    /// operand's.
    fn outer_at(&self, (kind, cond): (OuterJoin, &Cond), (left, right): (&LogicalPlan, &LogicalPlan), depth: usize) -> Card {
        let (l, r) = (self.card_at(left, depth), self.card_at(right, depth));
        let product = l.rows * r.rows;
        let mut rows = at_least_one(product * self.selectivity(cond, Over::Pair(left, right)), product);
        if kind.keeps_left() {
            rows = rows.max(l.rows);
        }
        if kind.keeps_right() {
            rows = rows.max(r.rows);
        }
        Card { rows, upper: l.upper * r.upper + l.upper + r.upper }
    }

    /// The card of the set operation `op`, ALL if `all`, of `left` and
    /// `right`.
    fn combined(&self, (op, all): (SetOp, bool), (left, right): (&LogicalPlan, &LogicalPlan), depth: usize) -> Card {
        let (l, r) = (self.card_at(left, depth), self.card_at(right, depth));
        let distinct = |plan: &LogicalPlan, card: Card| {
            if all {
                return card.rows;
            }
            let columns: Vec<usize> = (0..plan.width()).collect();
            self.groups(plan, &columns, card.rows)
        };
        match op {
            SetOp::Union => {
                let sum = l.rows + r.rows;
                let rows = if all { sum } else { super::groups::cardenas(distinct(left, l) + distinct(right, r), sum) };
                Card { rows, upper: l.upper + r.upper }
            }
            SetOp::Intersect => {
                let rows = at_least_one(distinct(left, l).min(distinct(right, r)), l.rows.min(r.rows));
                Card { rows, upper: l.upper.min(r.upper) }
            }
            SetOp::Except => Card { rows: distinct(left, l), upper: l.upper },
        }
    }

    /// The card of `limit` over `input`.
    pub(in crate::physical) fn limit_card(&self, limit: &Limit, input: &LogicalPlan) -> Card {
        limited(limit, self.card(input))
    }
}

/// The card of `limit` over an input of `input`: its rows past the offset,
/// at most its count, where each is a constant row count.
fn limited(limit: &Limit, input: Card) -> Card {
    let passed = match &limit.offset {
        Some(Expr::Const(Value::Exact(d))) => (d.scale == 0).then(|| usize::try_from(d.units).ok()).flatten().map_or(0.0_f64, cost_of),
        Some(_) | None => 0.0_f64,
    };
    let left = (input.rows - passed).max(0.0_f64);
    match limit.most() {
        Some(most) => Card { rows: left.min(cost_of(most)), upper: input.upper.min(cost_of(most)) },
        None => Card { rows: left, upper: input.upper },
    }
}

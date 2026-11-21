//! \[DESIGN\] (RULES.md D550) Whether a grouping or DISTINCT is streamed:
//! made of rows in the order of its keys, a group at a time, nothing held
//! (`exec/group/streamed.rs`), or hashed, its groups held in a table. The
//! keys may be asked of its input in any order of its columns, and any
//! direction, as rows alike in every key come together in each.
//!
//! A grouping whose input gives the order asked of it, a sort or an
//! ordered read below, streams. Under an ORDER BY of its keys, the input
//! not in their order, it is streamed over a sort of its rows when that
//! costs less than hashing them and sorting the groups. Of no order asked,
//! it is streamed over a read of its one table in an order the table keeps
//! or an index gives, when that read and a comparison of each row cost
//! less than the cheapest read and a hashed grouping, and the grouping
//! would not run in parallel, as a hashed one may (RULES.md D290). A
//! grouping by a collation, which compares its values' sort keys, not
//! their order, one making groups before any row, one by no column, and one
//! of a set function DISTINCT or of an inverse distribution is hashed.

use super::cost::HASH_ROW;
use super::cost::spill::grouped_spilled;
use super::indexed::access_cost;
use super::{Built, PhysicalPlanner, Plan};
use crate::catalog::QualifiedName;
use crate::logical::{AggCall, Expr, LogicalPlan, SortKey, render_keys};
use crate::settings::{OFF, STREAMED_GROUPING};
use crate::storage::cost_of;
use crate::value::collation::CollationRef;

/// \[DESIGN\] The cost of a group a hashed grouping makes, beside hashing
/// its rows, in rows of a scan: its key copied into the table and its
/// set functions' states made, about as much as reading a row.
const GROUP_MADE: f64 = 1.0;
/// \[DESIGN\] The bytes of a group a hashed grouping holds for each of its
/// columns, a key's or a set function's: a value and its share of the
/// table's slots, as `exec/group.rs` counts a state (`STATE_BYTES`).
const COLUMN_BYTES: f64 = 64.0;

/// A grouping's or DISTINCT's keys and size, as its choice is made.
#[derive(Clone, Copy)]
pub(super) struct Grouping<'a> {
    /// Its input.
    pub(super) input: &'a LogicalPlan,
    /// The columns of its input it groups by.
    pub(super) keys: &'a [usize],
    /// The columns of its input it reads, if a filter below may be told
    /// them (`build_input`).
    pub(super) reads: Option<&'a [usize]>,
    /// Its estimated groups.
    pub(super) groups: f64,
    /// The columns of each group it holds: its keys and its set functions.
    pub(super) width: usize,
    /// Whether it may be streamed at all (`streamable`).
    pub(super) streamable: bool,
}

/// Whether a grouping by `collations`, of `seeds`, computing `aggs`, may
/// be streamed: none of its columns grouped by a collation, no group made
/// before any row, none of its set functions DISTINCT or an inverse
/// distribution function.
pub(super) fn streamable(collations: &[Option<CollationRef>], seeds: usize, aggs: &[AggCall]) -> bool {
    let alone = aggs.iter().any(|a| a.distinct || a.distribution.is_some());
    collations.iter().all(Option::is_none) && seeds == 0 && !alone
}

impl PhysicalPlanner<'_> {
    /// The input of the grouping `g` and whether the grouping is streamed,
    /// asked `inner`, its columns' order, if any, as `make` makes it of
    /// them: whether the plan gives `inner`'s order.
    pub(super) fn grouped(&self, g: Grouping<'_>, inner: &[SortKey], make: impl Fn(Plan, bool) -> Plan) -> Built {
        let streams = g.streamable && !g.keys.is_empty() && self.db.settings.word(STREAMED_GROUPING) != OFF;
        // A row goal passes to an input read in the keys' order, each group
        // taking its share of its rows; one hashed reads them all
        // (`goals.rs`).
        let every = !inner.is_empty() && g.keys.iter().all(|&k| inner.iter().any(|i| i.column == k));
        let goal = if streams && every { self.divided(|| g.groups / self.card(g.input).rows.max(1.0_f64)) } else { self.whole() };
        let b = goal.grouped_input(g, inner);
        if !streams {
            return Built { ordered: !inner.is_empty() && b.ordered, plan: make(b.plan, false) };
        }
        if !inner.is_empty() {
            return self.asked_order(g, inner, b, make);
        }
        let cheapest = access_cost(&b.plan);
        let hashed = make(b.plan, false);
        if self.first.is_none() && super::parallel::runs_parallel(self.db, &hashed) {
            return Built::unordered(hashed);
        }
        let Some(order) = self.free_order(g.input, g.keys) else { return Built::unordered(hashed) };
        let read = self.divided(|| g.groups / self.card(g.input).rows.max(1.0_f64)).grouped_input(g, &order);
        let cheaper = read.ordered && self.ordered_cheaper(g, (access_cost(&read.plan), cheapest));
        Built::unordered(if cheaper { make(read.plan, true) } else { hashed })
    }

    /// The input of `g`, asked `order`.
    fn grouped_input(&self, g: Grouping<'_>, order: &[SortKey]) -> Built {
        match g.reads {
            Some(reads) => self.build_input(g.input, order, reads),
            None => self.build(g.input, order),
        }
    }

    /// The grouping `g` of `b`, its input asked `inner`: streamed if `b`
    /// gives that order and it is of every key; else hashed as it is, or
    /// streamed over a sort of its keys, `inner`'s first, if that costs
    /// less than hashing and sorting the groups.
    fn asked_order(&self, g: Grouping<'_>, inner: &[SortKey], b: Built, make: impl Fn(Plan, bool) -> Plan) -> Built {
        let every = g.keys.iter().all(|&k| inner.iter().any(|i| i.column == k));
        if b.ordered {
            return Built { plan: make(b.plan, every), ordered: true };
        }
        if !self.sorts_first(g) {
            return Built::unordered(make(b.plan, false));
        }
        let mut keys = inner.to_vec();
        keys.extend(g.keys.iter().filter(|&&k| !inner.iter().any(|i| i.column == k)).map(|&k| SortKey::ascending(k)));
        let detail = self.described(g.input, |names| render_keys(&keys, names));
        let sorted = Plan::Sort { keys, detail, input: Box::new(b.plan), most: None };
        Built { plan: make(sorted, true), ordered: true }
    }

    /// Whether sorting the input of `g` and streaming it costs less than
    /// hashing it and sorting its groups.
    fn sorts_first(&self, g: Grouping<'_>) -> bool {
        let w = self.weights();
        let n = self.card(g.input).rows;
        w.sort(n) + n * w.sort_compare < self.hashed_cost(n, g) + w.sort(g.groups)
    }

    /// What hashing `n` rows into the groups of `g` costs: each row hashed,
    /// each group made, and, past its grant, each row partitioned, written
    /// and read back (`cost/spill.rs`, RULES.md D651).
    fn hashed_cost(&self, n: f64, g: Grouping<'_>) -> f64 {
        let bytes = cost_of(g.width.max(1)) * COLUMN_BYTES;
        let spilled = grouped_spilled((g.groups, bytes), n, self.weights().grant_bytes);
        n * HASH_ROW + g.groups * GROUP_MADE + spilled
    }

    /// Whether a read of the input of `g` in its keys' order, of cost
    /// `ordered`, streamed, costs less than the cheapest read, of cost
    /// `cheapest`, hashed; of reads whose costs are unknown, not.
    fn ordered_cheaper(&self, g: Grouping<'_>, (ordered, cheapest): (Option<f64>, Option<f64>)) -> bool {
        let (Some(read), Some(cheapest)) = (ordered, cheapest) else { return false };
        let n = self.card(g.input).rows;
        // Under a row goal, the rows of the groups asked alone are read
        // and compared (`goals.rs`).
        let compared = self.first.map_or(n, |f| (cost_of(f) * n / g.groups.max(1.0_f64)).min(n));
        read + compared * self.weights().sort_compare < cheapest + self.hashed_cost(n, g)
    }

    /// An order of the columns `keys` of `input`, a read of one table,
    /// that the table keeps its rows in or an index of it gives, each
    /// ascending, if one of them does.
    fn free_order(&self, input: &LogicalPlan, keys: &[usize]) -> Option<Vec<SortKey>> {
        let (table, columns) = table_columns(input, keys, 0)?;
        let of_table = |order: &[usize]| -> Option<Vec<SortKey>> {
            let leading = order.get(..columns.len())?;
            let mapped = leading.iter().map(|t| columns.iter().position(|c| c == t).and_then(|at| keys.get(at)));
            mapped.map(|k| k.map(|&k| SortKey::ascending(k))).collect()
        };
        if let Some(order) = self.db.storage_of(table).ok().and_then(|s| of_table(&s.ordered_by())) {
            return Some(order);
        }
        let ordered = |ix: &&crate::catalog::IndexDesc| self.db.methods.get(&ix.method).is_some_and(|m| m.ordered()) && !ix.is_collated();
        self.db.catalog.indexes(table).into_iter().filter(ordered).find_map(|ix| of_table(&ix.columns))
    }
}

/// \[DESIGN\] How many projections and filters `table_columns` looks
/// through to a table's read (RULES.md R-CF-6): more than a query's
/// select list and WHERE clause put over it.
const MAX_READ_DEPTH: usize = 8;

/// The table `plan` reads, if it is a read of one table, filtered or
/// projected to columns, and the columns of it `columns` of `plan` are,
/// `depth` levels below the grouping.
fn table_columns<'p>(plan: &'p LogicalPlan, columns: &[usize], depth: usize) -> Option<(&'p QualifiedName, Vec<usize>)> {
    if depth > MAX_READ_DEPTH {
        return None;
    }
    match plan {
        LogicalPlan::Scan { table, .. } => Some((table, columns.to_vec())),
        LogicalPlan::Filter { input, .. } => table_columns(input, columns, depth.saturating_add(1)),
        LogicalPlan::Project { exprs, input } => {
            let of = |&c: &usize| if let Some(Expr::Column(at)) = exprs.get(c) { Some(*at) } else { None };
            let below: Vec<usize> = columns.iter().map(of).collect::<Option<_>>()?;
            table_columns(input, &below, depth.saturating_add(1))
        }
        LogicalPlan::OuterJoin { .. }
        | LogicalPlan::SemiJoin { .. }
        | LogicalPlan::Product { .. }
        | LogicalPlan::Aggregate { .. }
        | LogicalPlan::Distinct { .. }
        | LogicalPlan::SetOp { .. }
        | LogicalPlan::System { .. }
        | LogicalPlan::Values { .. }
        | LogicalPlan::Sort { .. }
        | LogicalPlan::Limit { .. }
        | LogicalPlan::Recursive { .. }
        | LogicalPlan::WorkTable { .. }
        | LogicalPlan::Transition { .. }
        | LogicalPlan::Unnest(_)
        | LogicalPlan::Window(_)
        | LogicalPlan::Recognize(_)
        | LogicalPlan::Polymorphic(_) => None,
    }
}

// Unit tests: which groupings may be streamed (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::collation::{CASE_INSENSITIVE, Pad, builtin};

    /// Verifies: [DESIGN] RULES.md D550: a grouping by a collation, one of
    /// groups made before any row, and one of a DISTINCT set function is
    /// hashed; one of none of them may be streamed.
    ///
    /// MC/DC of `collations.iter().all(..) && seeds == 0 && !alone`: each
    /// condition alone false against all true.
    #[test]
    fn a_collation_seeds_or_a_distinct_function_keep_a_grouping_hashed() {
        let db = crate::db::State::with_builtins(Box::new(crate::catalog::MemoryCatalog::default()));
        let collation = builtin().into_iter().find(|c| c.name() == CASE_INSENSITIVE).unwrap();
        let collated = [None, Some(CollationRef { collation, pad: Pad::Space })];
        let func = db.functions.aggregate("COUNT").unwrap();
        let result = func.0.result_type(None).unwrap();
        let count = AggCall { func, arg: None, distinct: false, filter: None, distribution: None, result, label: "COUNT(*)".into() };
        let distinct = AggCall { distinct: true, ..count.clone() };
        assert!(streamable(&[None, None], 0, std::slice::from_ref(&count)));
        assert!(!streamable(&collated, 0, std::slice::from_ref(&count)));
        assert!(!streamable(&[None], 1, std::slice::from_ref(&count)));
        assert!(!streamable(&[None], 0, &[distinct]));
    }
}

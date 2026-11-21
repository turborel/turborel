//! \[DESIGN\] General unnesting (Neumann and Kemper, "Unnesting Arbitrary
//! Queries", 2015, §3.2; RULES.md D312). The dependent join of the outer
//! rows with a subquery is the join of those rows with the dependent join
//! of D, the set of their distinct bindings, with the subquery, D's own
//! made of a projection of them with duplicates removed (the magic set).
//! That dependent join is pushed down the subquery's plan, operator by
//! operator, until what it reaches reads no outer reference and it is a
//! product with D.
//!
//! The plan made gives, for each binding, the rows the subquery gives for
//! it, D's columns first, then the subquery's. Each outer reference is
//! the column of D of its slot. As D is a set, a grouping groups by its
//! columns too, a window partitions by them, DISTINCT and the set
//! operators compare them, and a join of two sides that both read outer
//! references joins them on D (`domain/joins.rs`). A grouping of no
//! grouping columns, which has a row even of no rows, is computed over
//! the outer join of D with its rows, so a binding with none has its group
//! (the COUNT bug, Kiessling 1985); ORDER BY with OFFSET or FETCH FIRST
//! keeps its rows for each binding by their numbers (`domain/ordered.rs`,
//! Neumann, "Improving Unnesting of Complex Queries", 2025, §4.4).
//!
//! Each binding is a value of an outer expression and the count of its
//! trailing spaces, so two strings that compare equal but differ are two
//! bindings (`calls::Trailing`), as they are two values; and bindings are
//! matched as IS NOT DISTINCT FROM matches them [ISO/IEC 9075-2:2023
//! §8.15], so a null binding is one. Every part of the subquery is then
//! evaluated for the bindings the outer rows have and no others, as often
//! as for each of their rows' once, so whether it raises an error is the
//! same (RULES.md D15).

mod joins;
mod ordered;

use std::cell::Cell;
use std::sync::Arc;

use super::Bound;
use super::calls::Trailing;
use super::pure::plan_pure;
use crate::functions::ScalarRef;
use crate::logical::{
    AggCall, Cond, Expr, LogicalPlan, OuterJoin, Subquery, Unnest, and_all, expr_columns, map_expr_columns, plan_has_outer,
};
use crate::value::Value;

/// \[DESIGN\] The most copies of D a subquery's unnesting may read. Each
/// reads the outer rows again, so a subquery whose correlation reaches
/// more of its leaves is left nested.
pub(super) const MAX_DOMAINS: usize = 4;

/// D, the distinct bindings of the outer rows, and how many copies of it
/// the plan made reads.
pub(super) struct Domain {
    /// Its plan.
    plan: LogicalPlan,
    /// Its columns, each outer expression's, then each one's trailing
    /// spaces.
    width: usize,
    /// The copies read so far.
    copies: Cell<usize>,
}

/// `sub`, applied to the rows of `rows`, unnested by its domain, if it
/// can be, as the rows of its outer join, D's columns first; the keys,
/// matched as not distinct, are the outer rows' bindings.
pub(super) fn pushed(sub: &Subquery, rows: &LogicalPlan) -> Option<Bound> {
    // D reads the outer rows again, so they must be the same rows.
    if !plan_pure(rows) {
        return None;
    }
    let trailing = ScalarRef(Arc::new(Trailing));
    let mut keys = sub.outer.clone();
    keys.extend(sub.outer.iter().map(|o| Expr::Call(trailing.clone(), vec![o.clone()])));
    let (from, read) = narrowed(rows, keys.clone());
    let projected = LogicalPlan::Project { exprs: read, input: Box::new(from) };
    let domain = Domain { plan: LogicalPlan::Distinct { input: Box::new(projected) }, width: keys.len(), copies: Cell::new(0) };
    let plan = domain.over(&sub.plan)?;
    Some(Bound { plan, keys, not_distinct: true, defaults: None, unique: super::one_row(&sub.plan) })
}

/// The rows D is made of, and `keys` over them: of a product whose one
/// side the keys read, that side's rows, each once for any row of the
/// other, so D has the bindings of the product's rows without making it.
fn narrowed(rows: &LogicalPlan, keys: Vec<Expr>) -> (LogicalPlan, Vec<Expr>) {
    let LogicalPlan::Product { left, right } = rows else { return (rows.clone(), keys) };
    let mut read = Vec::new();
    for k in &keys {
        expr_columns(k, &mut read);
    }
    let lw = left.width();
    let (side, other, keys) = if read.iter().all(|&c| c < lw) {
        (left, right, keys)
    } else if read.iter().all(|&c| c >= lw) {
        (right, left, keys.into_iter().map(|k| map_expr_columns(k, &|c| c.saturating_sub(lw))).collect())
    } else {
        return (rows.clone(), keys);
    };
    let (side, keys) = narrowed(side, keys);
    // A row if the other side has any.
    let any = LogicalPlan::Project { exprs: vec![Expr::Const(Value::Bool(true))], input: other.clone() };
    let guarded = LogicalPlan::Product { left: Box::new(side), right: Box::new(LogicalPlan::Distinct { input: Box::new(any) }) };
    (guarded, keys)
}

/// `e`, of a row whose columns follow `by` others, each outer reference
/// the column of D of its slot.
pub(super) fn shifted(e: Expr, by: usize) -> Expr {
    if let Expr::Column(c) = e {
        return Expr::Column(c.saturating_add(by));
    }
    if let Expr::Outer(slot, _) = e {
        return Expr::Column(slot);
    }
    e.map_children(&mut |x| shifted(x, by))
}

/// `c`, as `shifted` makes its expressions.
pub(super) fn shifted_cond(c: Cond, by: usize) -> Cond {
    c.map_exprs(&mut |e| shifted(e, by))
}

/// Of columns `a` and `b`, that they are not distinct.
pub(super) fn same(a: usize, b: usize) -> Cond {
    Cond::Not(Box::new(Cond::Distinct(Expr::Column(a), Expr::Column(b))))
}

impl Domain {
    /// D's columns, as the first of a row's.
    fn kept(&self) -> impl Iterator<Item = Expr> {
        (0..self.width).map(Expr::Column)
    }

    /// A copy of D, counted; none past `MAX_DOMAINS`.
    fn copy(&self) -> Option<LogicalPlan> {
        let copies = self.copies.get().saturating_add(1);
        self.copies.set(copies);
        (copies <= MAX_DOMAINS).then(|| self.plan.clone())
    }

    /// D's dependent join with `plan`, pushed down it if it reads an
    /// outer reference, else D's product with it.
    pub(super) fn over(&self, plan: &LogicalPlan) -> Option<LogicalPlan> {
        if plan_has_outer(plan) {
            return self.push(plan);
        }
        Some(LogicalPlan::Product { left: Box::new(self.copy()?), right: Box::new(plan.clone()) })
    }

    /// D's dependent join with `plan`, which reads an outer reference,
    /// pushed down its operator.
    fn push(&self, plan: &LogicalPlan) -> Option<LogicalPlan> {
        let by = self.width;
        match plan {
            LogicalPlan::Filter { cond, input } => {
                Some(LogicalPlan::Filter { cond: shifted_cond(cond.clone(), by), input: Box::new(self.over(input)?) })
            }
            LogicalPlan::Project { exprs, input } => {
                let exprs = self.kept().chain(exprs.iter().map(|e| shifted(e.clone(), by))).collect();
                Some(LogicalPlan::Project { exprs, input: Box::new(self.over(input)?) })
            }
            LogicalPlan::Distinct { input } => Some(LogicalPlan::Distinct { input: Box::new(self.over(input)?) }),
            LogicalPlan::Aggregate { groups, collations, aggs, seeds, input } => {
                if !seeds.is_empty() {
                    return None;
                }
                if groups.is_empty() { self.whole(aggs, input) } else { self.grouped((groups, collations), aggs, input) }
            }
            LogicalPlan::Values { rows, .. } => self.values(rows),
            LogicalPlan::Unnest(u) => self.unnest(u),
            LogicalPlan::Product { left, right } => self.product(left, right),
            LogicalPlan::OuterJoin { kind, cond, left, right } => self.outer_join((*kind, cond), left, right),
            LogicalPlan::SetOp { op, all, compared, left, right } => self.set_op((*op, *all, *compared), left, right),
            LogicalPlan::Sort { keys, input } => self.sort(keys, input),
            LogicalPlan::Limit { limit, input } => self.limit(limit, input),
            LogicalPlan::Window(w) => self.window(w),
            LogicalPlan::Scan { .. }
            | LogicalPlan::System { .. }
            | LogicalPlan::SemiJoin { .. }
            | LogicalPlan::MarkJoin(_)
            | LogicalPlan::Recursive { .. }
            | LogicalPlan::WorkTable { .. }
            | LogicalPlan::Transition { .. }
            | LogicalPlan::Recognize(_)
            | LogicalPlan::Polymorphic(_) => None,
        }
    }

    /// A grouping by `groups`, under `collations`, and by D's columns too
    /// (Neumann and Kemper 2015, §3.2, whose Γ is over D's attributes
    /// too).
    fn grouped(
        &self,
        (groups, collations): (&[usize], &[Option<crate::value::collation::CollationRef>]),
        aggs: &[AggCall],
        input: &LogicalPlan,
    ) -> Option<LogicalPlan> {
        let by = self.width;
        let mut columns: Vec<usize> = (0..by).collect();
        columns.extend(groups.iter().map(|g| g.saturating_add(by)));
        let mut collated = Vec::new();
        if !collations.is_empty() {
            collated.extend(std::iter::repeat_n(None, by));
            collated.extend(collations.iter().cloned());
        }
        let aggs = aggs.iter().map(|a| a.clone().map_exprs(&mut |e| shifted(e, by))).collect();
        Some(LogicalPlan::Aggregate { groups: columns, collations: collated, aggs, seeds: Vec::new(), input: Box::new(self.over(input)?) })
    }

    /// A grouping of no grouping columns, of each binding's rows, over
    /// D's outer join with them, so a binding with none has its group;
    /// each set function takes only the rows the join found, by a marker
    /// that is null on the row of nulls [ISO/IEC 9075-2:2023 §10.9 GR4].
    fn whole(&self, aggs: &[AggCall], input: &LogicalPlan) -> Option<LogicalPlan> {
        let by = self.width;
        let rows = self.over(input)?;
        let wide = rows.width();
        let mut exprs: Vec<Expr> = (0..wide).map(Expr::Column).collect();
        exprs.push(Expr::Const(Value::Bool(true)));
        let marked = LogicalPlan::Project { exprs, input: Box::new(rows) };
        let on = and_all((0..by).map(|i| same(i, by.saturating_add(i))).collect()).unwrap_or(Cond::Const(crate::logical::Truth::True));
        let join = LogicalPlan::OuterJoin { kind: OuterJoin::Left, cond: on, left: Box::new(self.copy()?), right: Box::new(marked) };
        // The join's rows hold D, D's copy, the input's columns and the marker.
        let found = Cond::IsNull(Expr::Column(by.saturating_add(wide)), true);
        let aggs = aggs
            .iter()
            .map(|a| {
                let a = a.clone().map_exprs(&mut |e| shifted(e, by.saturating_mul(2)));
                let filter = Some(a.filter.clone().map_or_else(|| found.clone(), |f| Cond::And(Box::new(f), Box::new(found.clone()))));
                AggCall { filter, ..a }
            })
            .collect();
        Some(LogicalPlan::Aggregate { groups: (0..by).collect(), collations: Vec::new(), aggs, seeds: Vec::new(), input: Box::new(join) })
    }

    /// A VALUES of one row, its expressions over D's columns; none of more.
    fn values(&self, rows: &[Vec<Expr>]) -> Option<LogicalPlan> {
        let [row] = rows else { return None };
        let exprs = self.kept().chain(row.iter().map(|e| shifted(e.clone(), self.width))).collect();
        Some(LogicalPlan::Project { exprs, input: Box::new(self.copy()?) })
    }

    /// A collection derived table laterally over its input, which follows
    /// D's columns; not of a GRAPH_TABLE, whose bound table reads its
    /// input's columns where unnesting does not move them.
    fn unnest(&self, u: &Unnest<LogicalPlan>) -> Option<LogicalPlan> {
        if u.graph.is_some() {
            return None;
        }
        let by = self.width;
        let input = self.over(&u.input)?;
        let mut moved = u.with_input(|_| ());
        moved.collections = moved.collections.into_iter().map(|e| shifted(e, by)).collect();
        moved.outer = moved.outer.map(|c| shifted_cond(c, by));
        Some(LogicalPlan::Unnest(Box::new(moved.over(input))))
    }
}

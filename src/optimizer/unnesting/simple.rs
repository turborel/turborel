//! \[DESIGN\] Simple unnesting (Neumann and Kemper, "Unnesting Arbitrary
//! Queries", 2015, §3.1 and §4; RULES.md D314). A subquery correlated only
//! by equalities of its own columns with outer references has those
//! equalities pulled up out of it, so it is computed once for every value
//! of the columns, and joined with the outer rows on them; no domain of
//! bindings is made.
//!
//! The equalities rise through filters, projections, DISTINCT and
//! groupings, each keeping the columns as its own, so a grouping groups
//! by them too. A grouping of no grouping columns then has a group for each
//! value that has rows, not one of no rows for the values that have none
//! (the COUNT bug). Its value for those is the value its select list has
//! over the empty group, given where the join finds no row
//! (`Pulled::defaults`).
//!
//! The subquery is then computed for values of the columns no outer row
//! has, so no part of it may raise an error (RULES.md D15). Its set
//! functions are COUNT, MAX and MIN alone (`pure::plan_raises`).

use super::Bound;
use super::pure::plan_raises;
use crate::functions::Registry;
use crate::logical::{AggCall, Cond, Expr, LogicalPlan, Subquery, and_all, plan_has_outer, split_and};
use crate::value::CompOp;

/// A subquery's plan, its correlation pulled up, which gives its columns,
/// then one for each equality pulled up, the value of the column the equality
/// compares with an outer reference.
struct Pulled {
    /// The plan.
    plan: LogicalPlan,
    /// For each key column, the expression over the outer references it
    /// equals.
    keys: Vec<Expr>,
    /// Of a plan whose grouping of no grouping columns was made a grouping
    /// by the keys, the value of each of its columns over no row, as
    /// constants or expressions of them.
    defaults: Option<Vec<Expr>>,
}

/// `sub` unnested simply, if it can be, as a bound subquery with its key
/// columns first; the keys are its outer expressions' values compared
/// with them, as an equality compares them, a null matching none.
pub(super) fn pulled(sub: &Subquery, functions: &Registry) -> Option<Bound> {
    let pulled = pull(&sub.plan, functions)?;
    if pulled.keys.is_empty() || plan_raises(&pulled.plan, functions) {
        return None;
    }
    let width = pulled.plan.width();
    let k = pulled.keys.len();
    let own = width.checked_sub(k)?;
    let order = (own..width).chain(0..own).map(Expr::Column).collect();
    let plan = LogicalPlan::Project { exprs: order, input: Box::new(pulled.plan) };
    let keys = pulled.keys.into_iter().map(|e| outer_bound(e, &sub.outer)).collect();
    Some(Bound { plan, keys, not_distinct: false, defaults: pulled.defaults })
}

/// `e`, over outer reference slots, over the rows the subquery is
/// applied to, each slot replaced by its outer expression.
fn outer_bound(e: Expr, outer: &[Expr]) -> Expr {
    if let Expr::Outer(i, name) = e {
        return outer.get(i).cloned().unwrap_or(Expr::Outer(i, name));
    }
    e.map_children(&mut |x| outer_bound(x, outer))
}

/// `plan`, its correlation pulled up, if every correlation is an equality
/// it can pull up.
fn pull(plan: &LogicalPlan, functions: &Registry) -> Option<Pulled> {
    if !plan_has_outer(plan) {
        return Some(Pulled { plan: plan.clone(), keys: Vec::new(), defaults: None });
    }
    match plan {
        LogicalPlan::Filter { cond, input } => filtered(cond, input, functions),
        LogicalPlan::Project { exprs, input } => projected(exprs, input, functions),
        LogicalPlan::Distinct { input } => {
            let inner = pull(input, functions)?;
            Some(Pulled { plan: LogicalPlan::Distinct { input: Box::new(inner.plan) }, ..inner })
        }
        LogicalPlan::Aggregate { groups, collations, aggs, seeds, input } => {
            if !seeds.is_empty() || aggs.iter().any(|a| a.exprs().into_iter().any(expr_has_outer)) {
                return None;
            }
            grouped((groups, collations), aggs, input, functions)
        }
        LogicalPlan::Scan { .. }
        | LogicalPlan::Product { .. }
        | LogicalPlan::OuterJoin { .. }
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

/// Whether `e` has an outer reference, among its subqueries' outer
/// expressions too.
fn expr_has_outer(e: &Expr) -> bool {
    let mut found = matches!(e, Expr::Outer(..));
    e.for_each_child(&mut |child| found = found || expr_has_outer(child));
    found
}

/// Whether `e` reads a column of its row.
fn reads_columns(e: &Expr) -> bool {
    let mut found = matches!(e, Expr::Column(_));
    e.for_each_child(&mut |child| found = found || reads_columns(child));
    found
}

/// Of a conjunct "a = b", one side reading columns and no outer reference
/// and the other outer references and no column, those sides, the
/// column's expression first, then the outer one's.
fn equated(c: &Cond) -> Option<(Expr, Expr)> {
    let Cond::Compare(a, CompOp::Eq, b) = c else { return None };
    let inner = |e: &Expr| reads_columns(e) && !expr_has_outer(e) && e.all_subqueries().is_empty();
    let outer = |e: &Expr| expr_has_outer(e) && !reads_columns(e) && e.all_subqueries().is_empty() && !e.can_raise();
    if inner(a) && outer(b) {
        Some((a.clone(), b.clone()))
    } else if inner(b) && outer(a) {
        Some((b.clone(), a.clone()))
    } else {
        None
    }
}

/// A filter of `cond` over `input`, its correlated conjuncts equalities
/// pulled up as keys.
fn filtered(cond: &Cond, input: &LogicalPlan, functions: &Registry) -> Option<Pulled> {
    let inner = pull(input, functions)?;
    if inner.defaults.is_some() {
        // A HAVING over a grouping of no grouping columns may keep no row
        // of the empty group, which no default can say.
        return None;
    }
    let mut conjuncts = Vec::new();
    split_and(cond.clone(), &mut conjuncts);
    let (mut rest, mut pairs) = (Vec::new(), Vec::new());
    for c in conjuncts {
        if !cond_has_outer(&c) {
            rest.push(c);
            continue;
        }
        pairs.push(equated(&c)?);
    }
    let width = inner.plan.width();
    let rows = match and_all(rest) {
        Some(c) => LogicalPlan::Filter { cond: c, input: Box::new(inner.plan) },
        None => inner.plan,
    };
    let mut exprs: Vec<Expr> = (0..width).map(Expr::Column).collect();
    let mut keys = inner.keys;
    for (column, outer) in pairs {
        exprs.push(column);
        keys.push(outer);
    }
    Some(Pulled { plan: LogicalPlan::Project { exprs, input: Box::new(rows) }, keys, defaults: None })
}

/// Whether `c` has an outer reference, among its subqueries' outer
/// expressions too.
fn cond_has_outer(c: &Cond) -> bool {
    let mut found = false;
    c.for_each_expr(&mut |e| found = found || expr_has_outer(e));
    found
}

/// A projection of `exprs` over `input`, the keys of `input` kept after
/// them; its defaults the expressions over the input's.
fn projected(exprs: &[Expr], input: &LogicalPlan, functions: &Registry) -> Option<Pulled> {
    if exprs.iter().any(expr_has_outer) {
        return None;
    }
    let inner = pull(input, functions)?;
    let own = inner.plan.width().checked_sub(inner.keys.len())?;
    let mut out: Vec<Expr> = exprs.to_vec();
    out.extend((own..inner.plan.width()).map(Expr::Column));
    let defaults = inner.defaults.map(|d| exprs.iter().map(|e| substituted(e.clone(), &d)).collect());
    Some(Pulled { plan: LogicalPlan::Project { exprs: out, input: Box::new(inner.plan) }, keys: inner.keys, defaults })
}

/// `e` with each column replaced by `values`' expression for it.
fn substituted(e: Expr, values: &[Expr]) -> Expr {
    if let Expr::Column(c) = e {
        return values.get(c).cloned().unwrap_or(Expr::Column(c));
    }
    e.map_children(&mut |x| substituted(x, values))
}

/// A grouping of `input` by `groups`, under `collations`, computing
/// `aggs`, grouping by the keys too, which follow its set functions.
fn grouped(
    (groups, collations): (&[usize], &[Option<crate::value::collation::CollationRef>]),
    aggs: &[AggCall],
    input: &LogicalPlan,
    functions: &Registry,
) -> Option<Pulled> {
    let inner = pull(input, functions)?;
    if inner.defaults.is_some() {
        return None;
    }
    let k = inner.keys.len();
    let own = inner.plan.width().checked_sub(k)?;
    let mut by: Vec<usize> = groups.to_vec();
    by.extend(own..own.saturating_add(k));
    let mut collated = collations.to_vec();
    if !collated.is_empty() {
        collated.extend(std::iter::repeat_n(None, k));
    }
    let grouping =
        LogicalPlan::Aggregate { groups: by, collations: collated, aggs: aggs.to_vec(), seeds: Vec::new(), input: Box::new(inner.plan) };
    // Its grouping columns, its set functions, then the keys.
    let (g, n) = (groups.len(), aggs.len());
    let order = (0..g).chain(g.saturating_add(k)..g.saturating_add(k).saturating_add(n)).chain(g..g.saturating_add(k));
    let plan = LogicalPlan::Project { exprs: order.map(Expr::Column).collect(), input: Box::new(grouping) };
    let defaults = if groups.is_empty() { Some(aggs.iter().map(empty_value).collect::<Option<Vec<Expr>>>()?) } else { None };
    Some(Pulled { plan, keys: inner.keys, defaults })
}

/// [ISO/IEC 9075-2:2023 §10.9 GR7] The value of `a` over no row, which is
/// what its accumulator, given none, finishes with.
fn empty_value(a: &AggCall) -> Option<Expr> {
    a.func.0.accumulator(a.result.clone()).finish().ok().map(Expr::Const)
}

//! \[DESIGN\] The copies of the rows of an aggregation of more than one
//! slot read again for each copy (RULES.md D342).
//!
//! Rows of a table's scan, filtered and projected by expressions that
//! give the same value of a row each time, are the same each time they
//! are read. Of slots without a filter, each copy is then a projection of
//! its own reading of them: the columns used, its marker, and the value
//! of each slot, its argument for its own slot and null for the others.
//! The copies are a UNION ALL of those projections, which the first
//! grouping reads by batches where a projection's expressions are its
//! columns and constants, as the copies a collection derived table makes
//! are not.

use super::Shape;
use super::levels::{Layout, marker};
use crate::logical::{Cond, Expr, LogicalPlan, SetOp, expr_columns};
use crate::value::Value;

/// The copies of `input` as the first grouping reads them, each slot's
/// value after the columns used and the marker, and where those are; if
/// `input` may be read again and no slot has a filter.
pub(super) fn appended(groups: &[usize], shape: &Shape, input: &LogicalPlan) -> Option<(LogicalPlan, Layout)> {
    let copies = shape.copies();
    if copies < 2 || shape.slots.iter().any(|s| s.filter.is_some()) || !rereadable(input) {
        return None;
    }
    let mut used = groups.to_vec();
    for a in &shape.aggs {
        a.exprs().into_iter().for_each(|e| expr_columns(e, &mut used));
    }
    used.sort_unstable();
    used.dedup();
    let copy = |n: usize| {
        let mut exprs: Vec<Expr> = used.iter().map(|&c| Expr::Column(c)).collect();
        exprs.push(marker(n));
        for (s, slot) in shape.slots.iter().enumerate() {
            exprs.push(if s.saturating_add(1) == n { slot.arg.clone() } else { Expr::Const(Value::Null) });
        }
        LogicalPlan::Project { exprs, input: Box::new(input.clone()) }
    };
    let rows = (2..=copies).fold(copy(1), |left, n| LogicalPlan::SetOp {
        op: SetOp::Union,
        all: true,
        compared: None,
        left: Box::new(left),
        right: Box::new(copy(n)),
    });
    // The marker is read as the column after the rows' own would be.
    let wide = input.width();
    used.push(wide);
    let plain = if copies > shape.slots.len() { copies } else { 1 };
    Some((rows, Layout { used, marker: Some(wide), plain, truths: Vec::new(), width: groups.len() }))
}

/// Whether the rows of `plan` are the same each time it is read: of a
/// table's scan, filtered and projected by steady expressions.
fn rereadable(plan: &LogicalPlan) -> bool {
    match plan {
        LogicalPlan::Scan { located, .. } => !located,
        LogicalPlan::Filter { cond, input } => steady_cond(cond) && rereadable(input),
        LogicalPlan::Project { exprs, input } => exprs.iter().all(steady) && rereadable(input),
        LogicalPlan::Product { .. }
        | LogicalPlan::OuterJoin { .. }
        | LogicalPlan::SemiJoin { .. }
        | LogicalPlan::MarkJoin(_)
        | LogicalPlan::Aggregate { .. }
        | LogicalPlan::Distinct { .. }
        | LogicalPlan::SetOp { .. }
        | LogicalPlan::Values { .. }
        | LogicalPlan::System { .. }
        | LogicalPlan::Sort { .. }
        | LogicalPlan::Limit { .. }
        | LogicalPlan::Recursive { .. }
        | LogicalPlan::Unnest(..)
        | LogicalPlan::Window(..)
        | LogicalPlan::Recognize(..)
        | LogicalPlan::Polymorphic(..)
        | LogicalPlan::WorkTable { .. }
        | LogicalPlan::Transition { .. } => false,
    }
}

/// Whether `e` gives the same value of a row each time, and nothing else
/// of it: no subquery, no value drawn, no routine invoked, no warning, no
/// function that may give another value.
fn steady(e: &Expr) -> bool {
    let mut steady_here = match e {
        Expr::Subquery(..) | Expr::NextValue(_) | Expr::Invoked(_) | Expr::Warned(..) => false,
        Expr::Call(f, _) => f.0.deterministic(),
        Expr::Column(_)
        | Expr::Const(_)
        | Expr::Neg(_)
        | Expr::Arith(..)
        | Expr::Outer(..)
        | Expr::Dynamic(_)
        | Expr::Cast(..)
        | Expr::Domain(..)
        | Expr::SortKey(..) => true,
        Expr::Case(whens, _) => whens.iter().all(|(c, _)| !c.has_subquery()),
        Expr::Truth(c) => !c.has_subquery(),
    };
    e.for_each_child(&mut |c| steady_here = steady_here && steady(c));
    steady_here
}

/// Whether `c` is of the same truth of a row each time (`steady`).
fn steady_cond(c: &Cond) -> bool {
    let mut steady_here = !c.has_subquery();
    c.for_each_expr(&mut |e| steady_here = steady_here && steady(e));
    steady_here
}

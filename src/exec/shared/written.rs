//! \[DESIGN\] (RULES.md D1129) A grouping written out, to find the places
//! a statement computes it alike. A grouping by hash is written with the
//! plan below it, operator by operator: the tables read and the columns
//! read of each, the conditions, the expressions, the keys of the joins,
//! the grouping columns and the set functions with their arguments.
//! Costs, estimates and what EXPLAIN shows are left out, and so are the
//! marks of a part run in parallel and of a counted operator, which
//! change how a plan runs and not its rows.
//!
//! Two groupings written alike are the same operators of the same tables
//! in the same order, so they give the same rows, read by one statement.
//! Only a plan that gives the same rows each time it runs is written: of
//! scans of whole tables in no order, filters, projections, hash joins
//! and groupings by hash of COUNT, SUM, AVG, MAX and MIN, whose
//! expressions are of columns and constants by deterministic functions,
//! with no subquery, outer reference, parameter, sequence or routine. A
//! function is written by its name, which with its arguments' types
//! names one function of a statement (RULES.md D74). Any other plan is
//! not written, and is run in each place as before.

use std::fmt::Write;

use super::MAX_DEPTH;
use crate::logical::{AggCall, Cond, Expr};
use crate::physical::Plan;

/// `plan` written out, if it is a grouping by hash that can be shared,
/// under the marks of a part run in parallel and of a counted operator.
pub(super) fn grouping(plan: &Plan) -> Option<String> {
    let mut out = String::new();
    let hashed = matches!(peeled(plan, 0), Some(Plan::Aggregate { streamed: false, .. }));
    hashed.then(|| plain(plan, &mut out, 0)).flatten().map(|()| out)
}

/// `plan` under the marks of a part run in parallel and of a counted
/// operator.
fn peeled(plan: &Plan, depth: usize) -> Option<&Plan> {
    if depth >= MAX_DEPTH {
        return None;
    }
    if let Plan::Parallel(p) = plan {
        return peeled(&p.input, depth.saturating_add(1));
    }
    if let Plan::Counted(c) = plan {
        return peeled(&c.input, depth.saturating_add(1));
    }
    Some(plan)
}

/// The input of the grouping `plan` is, under its marks, to change in
/// place: none of a plan that is no grouping by hash.
pub(super) fn below(plan: &mut Plan, depth: usize) -> Option<&mut Plan> {
    if depth >= MAX_DEPTH {
        return None;
    }
    match plan {
        Plan::Parallel(p) => below(&mut p.input, depth.saturating_add(1)),
        Plan::Counted(c) => below(&mut c.input, depth.saturating_add(1)),
        Plan::Aggregate { input, streamed: false, .. } => Some(input),
        Plan::Aggregate { .. }
        | Plan::SeqScan { .. }
        | Plan::Nearest(_)
        | Plan::IndexScan { .. }
        | Plan::Filter { .. }
        | Plan::Product { .. }
        | Plan::HashJoin { .. }
        | Plan::OuterJoin { .. }
        | Plan::IndexJoin { .. }
        | Plan::MergeJoin(_)
        | Plan::RangeJoin(_)
        | Plan::SemiJoin(_)
        | Plan::MarkJoin(_)
        | Plan::Project { .. }
        | Plan::Append { .. }
        | Plan::MergeAppend { .. }
        | Plan::SetOp { .. }
        | Plan::Values { .. }
        | Plan::System { .. }
        | Plan::Sort { .. }
        | Plan::Limit { .. }
        | Plan::Distinct { .. }
        | Plan::Recursive { .. }
        | Plan::WorkTable { .. }
        | Plan::Transition { .. }
        | Plan::Unnest(..)
        | Plan::Window { .. }
        | Plan::Recognize { .. }
        | Plan::Polymorphic(..)
        | Plan::Transfer(_)
        | Plan::Shared(_) => None,
    }
}

/// Writes `plan` to `out`: none of a plan not written, as the module
/// says, `out` then of no use.
fn plain(plan: &Plan, out: &mut String, depth: usize) -> Option<()> {
    if depth >= MAX_DEPTH {
        return None;
    }
    let deeper = depth.saturating_add(1);
    if let Plan::Parallel(p) = plan {
        return plain(&p.input, out, deeper);
    }
    if let Plan::Counted(c) = plan {
        return plain(&c.input, out, deeper);
    }
    if let Plan::SeqScan { table, columns, order, backward: false, most: None, located: false, .. } = plan {
        return order.is_empty().then(|| write!(out, "scan({table:?} {columns:?})").ok()).flatten();
    }
    if let Plan::Filter { cond, input, .. } = plan {
        pure_cond(cond, 0).then(|| write!(out, "filter({cond:?} ").ok()).flatten()?;
        return closed(input, out, deeper);
    }
    if let Plan::Project { exprs, input, .. } = plan {
        exprs.iter().all(|e| pure(e, 0)).then(|| write!(out, "project({exprs:?} ").ok()).flatten()?;
        return closed(input, out, deeper);
    }
    if let Plan::HashJoin { left, right, keys, .. } = plan {
        write!(out, "join({keys:?} ").ok()?;
        plain(left, out, deeper)?;
        out.push(' ');
        return closed(right, out, deeper);
    }
    // Any other operator is not written, so its plan is shared nowhere.
    let Plan::Aggregate { groups, collations, aggs, seeds, input, streamed: false, .. } = plan else { return None };
    write!(out, "group({groups:?} {collations:?} {seeds:?} ").ok()?;
    aggs.iter().try_for_each(|a| set_function(a, out))?;
    closed(input, out, deeper)
}

/// Writes `plan`, then closes its operator's bracket.
fn closed(plan: &Plan, out: &mut String, depth: usize) -> Option<()> {
    plain(plan, out, depth)?;
    out.push(')');
    Some(())
}

/// Writes the set function `a`, one of COUNT, SUM, AVG, MAX and MIN of a
/// pure argument and filter: its name, argument, filter and result's
/// type, not its label, which a place's own names make.
fn set_function(a: &AggCall, out: &mut String) -> Option<()> {
    let pure_parts = a.arg.as_ref().is_none_or(|e| pure(e, 0)) && a.filter.as_ref().is_none_or(|c| pure_cond(c, 0));
    if a.func.0.columnar().is_none() || a.distribution.is_some() || !pure_parts {
        return None;
    }
    write!(out, "[{:?} {:?} {} {:?} {:?}]", a.func, a.arg, a.distinct, a.filter, a.result).ok()
}

/// Whether `e` has one value of a row each time it is evaluated: of
/// columns and constants by deterministic functions, arithmetic, casts
/// and CASE, with no subquery, outer reference, parameter, sequence,
/// routine, domain's check or warning.
fn pure(e: &Expr, depth: usize) -> bool {
    let deeper = depth.saturating_add(1);
    let own = match e {
        Expr::Call(f, _) => f.0.deterministic() && !f.0.warns(),
        Expr::Case(whens, _) => whens.iter().all(|(c, _)| pure_cond(c, deeper)),
        Expr::Truth(c) => pure_cond(c, deeper),
        Expr::Column(_) | Expr::Const(_) | Expr::Neg(_) | Expr::Arith(..) | Expr::Cast(..) | Expr::SortKey(..) => true,
        Expr::Subquery(..)
        | Expr::Invoked(_)
        | Expr::NextValue(_)
        | Expr::Outer(..)
        | Expr::Dynamic(_)
        | Expr::Domain(..)
        | Expr::Warned(..) => false,
    };
    let mut parts = depth < MAX_DEPTH;
    e.for_each_child(&mut |child| parts = parts && pure(child, deeper));
    own && parts
}

/// Whether `c` has one truth of a row each time it is judged, as `pure`
/// says of expressions, and holds no subquery.
fn pure_cond(c: &Cond, depth: usize) -> bool {
    let mut all = depth < MAX_DEPTH && c.subqueries().is_empty();
    c.for_each_expr(&mut |e| all = all && pure(e, depth.saturating_add(1)));
    all
}

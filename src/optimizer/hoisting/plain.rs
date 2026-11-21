//! \[DESIGN\] Which operands a guard may read again (RULES.md D373). The
//! guard of a hoisted part evaluates each other operand of the product
//! once more, in EXISTS. That is harmless only of an operand whose
//! evaluation has no effect and gives the same rows each time: a read of
//! tables and values through filters, projections, joins, groupings,
//! windows, sorts and set operations, of expressions that invoke no
//! routine, draw no sequence value, call no function that is not
//! deterministic and hold no subquery. A table function, polymorphic or
//! built in, may record its run or cost as much as the query, and a
//! system table, an unnested collection, a recursive query, a row pattern
//! and a trigger's transition table are left out too, so a part that
//! needs a guard over any of them is not hoisted.

use super::found::pure;
use crate::logical::{Cond, Expr, LogicalPlan, MAX_JUDGED_DEPTH};

/// Whether evaluating `plan` again has no effect and gives the same rows.
pub(super) fn plain(plan: &LogicalPlan) -> bool {
    plain_at(plan, 0)
}

/// `plain` of `plan` at nesting `depth`.
fn plain_at(plan: &LogicalPlan, depth: usize) -> bool {
    if depth > MAX_JUDGED_DEPTH {
        return false;
    }
    let own = match plan {
        LogicalPlan::Filter { cond, .. } | LogicalPlan::OuterJoin { cond, .. } | LogicalPlan::SemiJoin { cond, .. } => quiet_cond(cond),
        LogicalPlan::MarkJoin(m) => m.conds().all(quiet_cond),
        LogicalPlan::Project { exprs, .. } => exprs.iter().all(quiet),
        LogicalPlan::Values { rows, .. } => rows.iter().flatten().all(quiet),
        LogicalPlan::Aggregate { aggs, .. } => {
            aggs.iter().all(|a| a.exprs().into_iter().all(quiet) && a.filter.as_ref().is_none_or(quiet_cond))
        }
        LogicalPlan::Window(w) => w.calls.iter().all(|c| c.exprs().chain(c.frame.offsets()).all(quiet)),
        LogicalPlan::Limit { limit, .. } => limit.exprs().all(quiet),
        LogicalPlan::Scan { .. }
        | LogicalPlan::Product { .. }
        | LogicalPlan::SetOp { .. }
        | LogicalPlan::Distinct { .. }
        | LogicalPlan::Sort { .. } => true,
        LogicalPlan::Polymorphic(_)
        | LogicalPlan::System { .. }
        | LogicalPlan::Unnest(_)
        | LogicalPlan::Recognize(_)
        | LogicalPlan::Recursive { .. }
        | LogicalPlan::WorkTable { .. }
        | LogicalPlan::Transition { .. } => false,
    };
    own && plan.inputs().all(|input| plain_at(input, depth.saturating_add(1)))
}

/// Whether `e` gives the same value each time and has no effect.
fn quiet(e: &Expr) -> bool {
    pure(e, 0)
}

/// Whether `c` gives the same truth each time and has no effect.
fn quiet_cond(c: &Cond) -> bool {
    let mut all = c.subqueries().is_empty();
    c.for_each_expr(&mut |e| all = all && pure(e, 0));
    all
}

//! \[DESIGN\] (RULES.md D1132) Whether running a scalar subquery is
//! seen by nothing but its value: it raises no error and no warning,
//! whatever rows its tables hold, so running it once more, or sooner, or
//! where its statement would not have run it, changes nothing a
//! statement gives but the time it takes.
//!
//! It raises no error if it reads nothing of the rows around it, gives
//! one row at most, so no cardinality violation [ISO/IEC 9075:1992 §7.11
//! GR1], and its plan raises nothing as the declared types of the
//! columns it reads judge it (`pure::plan_raises`, RULES.md D1060). It
//! raises no warning if its plan holds no subquery, no invocation and no
//! warning an expression carries, has no window, and each set function
//! of it takes no argument or a column that holds no null, so no null
//! value is eliminated [ISO/IEC 9075:1992 §6.5 GR1b]: one declared so,
//! or one a filter below the set function compares, which keeps no row
//! of a null there, as the comparison is unknown [§8.2 GR1a].

use super::filled::never_null;
use super::pure::plan_raises;
use crate::catalog::Catalog;
use crate::db::State;
use crate::logical::{Cond, Expr, LogicalPlan, MAX_JUDGED_DEPTH, Subquery, map_plan_exprs, split_and};
use crate::optimizer::RuleContext;

/// Whether running `s` raises no error and no warning, as the module
/// says, of `db`'s tables.
pub(crate) fn quiet(s: &Subquery, db: &State) -> bool {
    let silent = s.outer.is_empty() && s.plan.one_row_at_most() && warns_none(&s.plan, &*db.catalog, 0);
    silent && !plan_raises(&s.plan, &RuleContext { db, seen: &crate::physical::Seen::default() })
}

/// Whether `plan`, `depth` operators down, raises no warning: its
/// expressions none, its operators and set functions none.
fn warns_none(plan: &LogicalPlan, catalog: &dyn Catalog, depth: usize) -> bool {
    let mut silent = true;
    drop(map_plan_exprs(plan.clone(), &mut |e| {
        silent = silent && expr_silent(&e);
        e
    }));
    silent && nodes_silent(plan, catalog, depth)
}

/// Whether `e` holds no subquery, no invocation and no warning. A
/// worklist, so no depth bounds it.
fn expr_silent(e: &Expr) -> bool {
    let mut work = vec![e];
    while let Some(next) = work.pop() {
        let silent = match next {
            Expr::Subquery(..) | Expr::Invoked(_) | Expr::Warned(..) | Expr::NextValue(_) | Expr::Dynamic(_) => false,
            Expr::Column(_)
            | Expr::Const(_)
            | Expr::Neg(_)
            | Expr::Arith(..)
            | Expr::Outer(..)
            | Expr::Case(..)
            | Expr::Cast(..)
            | Expr::Domain(..)
            | Expr::SortKey(..)
            | Expr::Call(..)
            | Expr::Truth(_) => true,
        };
        if !silent {
            return false;
        }
        next.for_each_child(&mut |child| work.push(child));
    }
    true
}

/// Whether each operator of `plan`, `depth` down, is one that raises no
/// warning of its own, each grouping's set functions of no argument or
/// of a column that holds no null; past `MAX_JUDGED_DEPTH` none is
/// (RULES.md R-CF-6).
fn nodes_silent(plan: &LogicalPlan, catalog: &dyn Catalog, depth: usize) -> bool {
    if depth > MAX_JUDGED_DEPTH {
        return false;
    }
    let own = match plan {
        LogicalPlan::Aggregate { aggs, input, .. } => aggs.iter().all(|a| {
            let filled = match &a.arg {
                None => true,
                Some(Expr::Column(c)) => filled(input, *c, catalog, 0),
                Some(_) => false,
            };
            filled && a.distribution.is_none()
        }),
        LogicalPlan::Scan { located, .. } => !located,
        LogicalPlan::Filter { .. }
        | LogicalPlan::Project { .. }
        | LogicalPlan::Product { .. }
        | LogicalPlan::Distinct { .. }
        | LogicalPlan::Sort { .. }
        | LogicalPlan::Limit { .. } => true,
        LogicalPlan::OuterJoin { .. }
        | LogicalPlan::SemiJoin { .. }
        | LogicalPlan::MarkJoin(_)
        | LogicalPlan::SetOp { .. }
        | LogicalPlan::System { .. }
        | LogicalPlan::Values { .. }
        | LogicalPlan::Recursive { .. }
        | LogicalPlan::WorkTable { .. }
        | LogicalPlan::Transition { .. }
        | LogicalPlan::Unnest(_)
        | LogicalPlan::Window(_)
        | LogicalPlan::Recognize(_)
        | LogicalPlan::Polymorphic(_) => false,
    };
    own && plan.inputs().all(|p| nodes_silent(p, catalog, depth.saturating_add(1)))
}

/// Whether column `column` of `plan`'s rows holds no null, `depth`
/// operators down: one a filter compares or judges IS NOT NULL, through
/// projections of columns, else as `never_null` judges it; past
/// `MAX_JUDGED_DEPTH` not known (RULES.md R-CF-6).
fn filled(plan: &LogicalPlan, column: usize, catalog: &dyn Catalog, depth: usize) -> bool {
    if depth > MAX_JUDGED_DEPTH {
        return false;
    }
    let deeper = depth.saturating_add(1);
    if let LogicalPlan::Filter { cond, input } = plan {
        let mut conjuncts = Vec::new();
        split_and(cond.clone(), &mut conjuncts);
        return conjuncts.iter().any(|c| rejects(c, column)) || filled(input, column, catalog, deeper);
    }
    if let LogicalPlan::Project { exprs, input } = plan {
        let Some(Expr::Column(c)) = exprs.get(column) else { return false };
        return filled(input, *c, catalog, deeper);
    }
    never_null(plan, column, catalog, 0)
}

/// Whether `c`, a conjunct of a filter, is not true of a row whose column
/// `column` is null: a comparison of the column, or its IS NOT NULL.
fn rejects(c: &Cond, column: usize) -> bool {
    let is = |e: &Expr| if let Expr::Column(at) = e { *at == column } else { false };
    match c {
        Cond::Compare(a, _, b) => is(a) || is(b),
        Cond::IsNull(e, true) => is(e),
        Cond::IsNull(_, false)
        | Cond::Const(_)
        | Cond::And(..)
        | Cond::Or(..)
        | Cond::Not(_)
        | Cond::Is(..)
        | Cond::Distinct(..)
        | Cond::Like(..)
        | Cond::Exists(_)
        | Cond::Quantified(..)
        | Cond::Unique(..)
        | Cond::Match { .. }
        | Cond::In(..)
        | Cond::Value(_) => false,
    }
}

// Unit tests: what raises nothing and warns of nothing, and each refusal
// (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;

//! \[DESIGN\] The object rows of a searched UPDATE or DELETE planned as a
//! join, each followed by its id (RULES.md D775). A semi join keeps its
//! left's rows alone, each once, but no table access reads them, so the
//! ids of the rows it keeps are not those an access gives with them
//! (`exec::table_entries`). Its object table's scan is then located: each
//! row it reads is followed by its id (`storage::located`), which the
//! semi joins over it, the filters and the projections keep last.

use crate::logical::{Expr, LogicalPlan, map_cond_columns};
use crate::storage::ROW_ID;

/// How many nodes down the object table's scan is looked for, past which
/// the plan is not located (RULES.md R-CF-6).
const MAX_LOCATED_DEPTH: usize = 64;

/// `plan`, the object rows optimized, its object table's scan located and
/// each row followed by its id, if each node over that scan is a filter,
/// a projection or a semi join's left operand.
pub(super) fn located(plan: LogicalPlan) -> Option<LogicalPlan> {
    located_at(plan, 0)
}

/// `located` of `plan`, `depth` nodes down.
fn located_at(plan: LogicalPlan, depth: usize) -> Option<LogicalPlan> {
    let deeper = (depth < MAX_LOCATED_DEPTH).then_some(depth.saturating_add(1))?;
    match plan {
        LogicalPlan::Scan { table, mut names, needed, located: false } => {
            names.push(ROW_ID.to_owned());
            Some(LogicalPlan::Scan { table, names, needed, located: true })
        }
        LogicalPlan::Filter { cond, input } => Some(LogicalPlan::Filter { cond, input: Box::new(located_at(*input, deeper)?) }),
        LogicalPlan::Project { mut exprs, input } => {
            exprs.push(Expr::Column(input.width()));
            Some(LogicalPlan::Project { exprs, input: Box::new(located_at(*input, deeper)?) })
        }
        LogicalPlan::SemiJoin { anti, cond, left, right } => {
            let width = left.width();
            let cond = map_cond_columns(cond, &|c| if c < width { c } else { c.saturating_add(1) });
            Some(LogicalPlan::SemiJoin { anti, cond, left: Box::new(located_at(*left, deeper)?), right })
        }
        LogicalPlan::Scan { located: true, .. }
        | LogicalPlan::Aggregate { .. }
        | LogicalPlan::Values { .. }
        | LogicalPlan::Limit { .. }
        | LogicalPlan::Sort { .. }
        | LogicalPlan::Distinct { .. }
        | LogicalPlan::Product { .. }
        | LogicalPlan::OuterJoin { .. }
        | LogicalPlan::MarkJoin(_)
        | LogicalPlan::SetOp { .. }
        | LogicalPlan::System { .. }
        | LogicalPlan::Recursive { .. }
        | LogicalPlan::WorkTable { .. }
        | LogicalPlan::Transition { .. }
        | LogicalPlan::Unnest(_)
        | LogicalPlan::Window(_)
        | LogicalPlan::Recognize(_)
        | LogicalPlan::Polymorphic(_) => None,
    }
}

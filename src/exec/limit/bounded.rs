//! \[DESIGN\] The input of OFFSET and FETCH FIRST told how many of its first
//! rows are asked for (`Most`): a sort, which then holds those alone
//! (`exec/sort/top.rs`), or a scan in an order, which then reads no more
//! than those of a source it sorts (`TableStorage::scan_ordered`), each
//! through projections that keep each row and raise no exception, so no
//! row they would compute is left out. A scan is not told of WITH TIES,
//! whose peers are of ORDER BY's keys, perhaps fewer than the scan's.

use crate::logical::{Expr, Most};
use crate::physical::Plan;

/// `plan`, its first `most` rows alone asked for, told so where it can be.
pub(super) fn bounded(plan: Plan, most: Most) -> Plan {
    match plan {
        Plan::Sort { keys, detail, input, .. } => Plan::Sort { keys, detail, input, most: Some(most) },
        Plan::SeqScan { table, columns, rows, cost, order, backward, most: before } => {
            let most = if order.is_empty() || most.ties { before } else { Some(most.rows) };
            Plan::SeqScan { table, columns, rows, cost, order, backward, most }
        }
        Plan::Project { exprs, detail, input } => {
            let input = if exprs.iter().any(Expr::can_raise) { input } else { Box::new(bounded(*input, most)) };
            Plan::Project { exprs, detail, input }
        }
        plan @ (Plan::IndexScan { .. }
        | Plan::Filter { .. }
        | Plan::Product { .. }
        | Plan::HashJoin { .. }
        | Plan::IndexJoin { .. }
        | Plan::OuterJoin { .. }
        | Plan::Aggregate { .. }
        | Plan::Distinct { .. }
        | Plan::SetOp { .. }
        | Plan::System { .. }
        | Plan::Values { .. }
        | Plan::Append { .. }
        | Plan::Limit { .. }
        | Plan::Recursive { .. }
        | Plan::WorkTable { .. }
        | Plan::Unnest(_)
        | Plan::Window { .. }
        | Plan::Recognize { .. }) => plan,
    }
}

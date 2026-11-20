//! \[DESIGN\] The rows a physical plan carries as estimated (RULES.md
//! D370): each operator's own estimate where it holds one, a projection's
//! and a sort's their input's, so a plan's rows can be read at any node,
//! as the executor's hash tables are sized by and an EXPLAIN may show.

use super::{Plan, Preserved};
use crate::storage::cost_of;

impl Plan {
    /// Its estimated rows, if the planner estimated them.
    pub fn est(&self) -> Option<f64> {
        match self {
            Plan::SeqScan { rows, .. } => Some(cost_of(*rows)),
            Plan::IndexScan { est, .. }
            | Plan::Filter { est, .. }
            | Plan::Product { est, .. }
            | Plan::HashJoin { est, .. }
            | Plan::OuterJoin { est, .. }
            | Plan::Aggregate { est, .. }
            | Plan::Distinct { est, .. }
            | Plan::SetOp { est, .. }
            | Plan::Append { est, .. }
            | Plan::Limit { est, .. }
            | Plan::Window { est, .. } => Some(*est),
            Plan::MergeJoin(m) => Some(m.est),
            Plan::Project { input, .. } | Plan::Sort { input, .. } => input.est(),
            Plan::Parallel(p) => p.input.est(),
            Plan::GroupJoin(g) => Some(g.est),
            Plan::IndexJoin { left, est, outer, .. } => {
                let left = left.est()?;
                let found = left * est;
                Some(if matches!(outer, Preserved::Inner) { found } else { found.max(left) })
            }
            Plan::MergeAppend { inputs, .. } => inputs.iter().map(Plan::est).sum(),
            Plan::Values { rows, .. } => Some(cost_of(rows.len())),
            Plan::Nearest(_)
            | Plan::System { .. }
            | Plan::Recursive { .. }
            | Plan::WorkTable { .. }
            | Plan::Transition { .. }
            | Plan::Unnest(_)
            | Plan::Recognize { .. }
            | Plan::Polymorphic(_) => None,
        }
    }

    /// The sum of the estimates of `left` and `right`, as an append of
    /// them gives, of those known.
    pub(super) fn summed(left: &Plan, right: &Plan) -> f64 {
        left.est().unwrap_or(0.0_f64) + right.est().unwrap_or(0.0_f64)
    }
}

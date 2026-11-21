//! \[DESIGN\] (RULES.md D651, D652) What a hash join of two relations the
//! enumeration does not plan costs, as the planner weighs probing an
//! index instead (`physical/indexed.rs`): the right read and each row of
//! both hashed, shared by the threads of their parts (`cost/parallel.rs`),
//! a part's start, and past the grant both operands spilled
//! (`cost/spill.rs`), as `enumerate/weighed.rs` costs one of a chain.

use super::HASH_ROW;
use super::parallel::PART_START;
use super::spill::{hash_spilled, row_bytes};
use crate::logical::LogicalPlan;
use crate::physical::robust::Linear;
use crate::physical::{PhysicalPlanner, Plan};
use crate::storage::cost_of;

impl PhysicalPlanner<'_> {
    /// The cost of a hash join of `left` and `right`, the right read by
    /// `read` costing `cost`, linear in the left's rows.
    pub(in crate::physical) fn hash_linear(&self, (left, right): (&LogicalPlan, &LogicalPlan), (read, cost): (&Plan, f64)) -> Linear {
        let w = self.weights();
        let (rows_l, rows_r) = (self.rows_of(left), self.rows_of(right));
        let on_right = w.degrees.degree_of(crate::physical::parallel::stream(read).map_or(0.0_f64, cost_of));
        let on_left = w.degrees.degree_of(self.held_rows(left));
        let start = if on_left.max(on_right) > 1.0_f64 { PART_START } else { 0.0_f64 };
        let (built, plan) = if rows_l < rows_r { (rows_l, left) } else { (rows_r, right) };
        let spill = hash_spilled((built, row_bytes(plan.width(), None)), rows_l.max(rows_r), w.grant_bytes);
        Linear { fixed: (cost + HASH_ROW * rows_r) / on_right + start + spill, per_row: HASH_ROW / on_left }
    }

    /// The rows the table `plan` reads under its filters holds, the rows
    /// of a part reading it: none if it reads no table so.
    fn held_rows(&self, plan: &LogicalPlan) -> f64 {
        let mut at = plan;
        while let LogicalPlan::Filter { input, .. } = at {
            at = input;
        }
        let LogicalPlan::Scan { table, .. } = at else { return 0.0_f64 };
        self.table_rows(table).unwrap_or(0.0_f64)
    }
}

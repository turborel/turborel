//! \[DESIGN\] (RULES.md D651, D652) What a hash join of two relations the
//! enumeration does not plan costs, as the planner weighs probing an
//! index instead (`physical/indexed.rs`): the right read and each row of
//! both hashed, shared by the threads of their parts (`cost/parallel.rs`),
//! a part's start, and past the grant both operands spilled
//! (`cost/spill.rs`), as `enumerate/weighed.rs` costs one of a chain.

use super::hashing::hash_row;
use super::parallel::{PART_START, ROW_STORE_GAIN, gain_of};
use super::spill::{hash_spilled, row_bytes};
use crate::catalog::QualifiedName;
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
        let streamed = crate::physical::parallel::stream(read).map_or(0.0_f64, cost_of);
        let on_right = w.degrees.speedup(streamed, self.gain(right));
        let on_left = w.degrees.speedup(self.held_rows(left), self.gain(left));
        let threads = w.degrees.degree_of(streamed).max(w.degrees.degree_of(self.held_rows(left)));
        let start = if threads > 1.0_f64 { PART_START } else { 0.0_f64 };
        let (built, plan) = if rows_l < rows_r { (rows_l, left) } else { (rows_r, right) };
        let bytes = row_bytes(plan.width(), None);
        let spill = hash_spilled((built, bytes), rows_l.max(rows_r), w.grant_bytes);
        let each = hash_row(built * bytes);
        Linear { fixed: (cost + each * rows_r) / on_right + start + spill, per_row: each / on_left }
    }

    /// The rows the table `plan` reads under its filters holds, the rows
    /// of a part reading it: none if it reads no table so.
    fn held_rows(&self, plan: &LogicalPlan) -> f64 {
        table_of(plan).and_then(|t| self.table_rows(t)).unwrap_or(0.0_f64)
    }

    /// The share of each thread past the first a part reading `plan`
    /// gains: a row store's if it reads no table so.
    fn gain(&self, plan: &LogicalPlan) -> f64 {
        table_of(plan).map_or(ROW_STORE_GAIN, |t| gain_of(self.db, t))
    }
}

/// The table `plan` reads under its filters, if it reads one so.
fn table_of(plan: &LogicalPlan) -> Option<&QualifiedName> {
    let mut at = plan;
    while let LogicalPlan::Filter { input, .. } = at {
        at = input;
    }
    if let LogicalPlan::Scan { table, .. } = at { Some(table) } else { None }
}

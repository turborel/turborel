//! \[DESIGN\] (RULES.md D552) Row goals: how many of its first rows a
//! FETCH FIRST reads of its input passes down to the inputs below it that
//! give their rows as they read them, each told how many of its own first
//! rows that takes, so an ordered read below is priced by the share of it
//! read (`PhysicalPlanner::first`, `physical/access.rs`). Carey and
//! Kossmann, "On Saying 'Enough Already!' in SQL", SIGMOD 1997, push such
//! a count through the operators that do not block.
//!
//! A projection, a sort and a scan pass the goal as it is. A sort's input
//! is read whole unless an ordered read gives its order, and that read
//! gives the sort's first rows as its own. So a sort's input that does
//! not give its order is planned again with no goal, and so is the input
//! of a grouping asked an order it does not give (`ordered.rs`,
//! `streamed.rs`, RULES.md D1218). A filter divides it by the
//! share of its input's rows it keeps, a table read's own condition too.
//! A read that probes by the condition reads only rows it keeps, so the
//! goal is then more than it reads, which prices the ordered read at no
//! less than its share. A join that gives its rows in the order of its
//! left operand, by a hash table of its right or an index of its right's
//! table, or a product, divides it by the rows each left row is estimated
//! to give, and tells its left operand alone, as its right is read whole. A
//! streamed grouping or DISTINCT multiplies it by the rows of a group
//! (`physical/streamed.rs`). UNION ALL passes it to each operand. Any
//! other node, a hashed grouping, an outer join, a window, stops it, as it
//! reads its input whole before it gives a row.

use super::{PhysicalPlanner, parallel_rows};
use crate::logical::{LogicalPlan, SetOp};
use crate::storage::cost_of;

/// Whether a node of `logical`'s kind passes a row goal to its inputs,
/// dividing it as the module says, or keeps none for them.
pub(super) fn passes(logical: &LogicalPlan) -> bool {
    match logical {
        LogicalPlan::Scan { .. }
        | LogicalPlan::Project { .. }
        | LogicalPlan::Sort { .. }
        | LogicalPlan::Filter { .. }
        | LogicalPlan::Product { .. }
        | LogicalPlan::Aggregate { .. }
        | LogicalPlan::Distinct { .. } => true,
        LogicalPlan::SetOp { op, all, .. } => *op == SetOp::Union && *all,
        LogicalPlan::OuterJoin { .. }
        | LogicalPlan::SemiJoin { .. }
        | LogicalPlan::MarkJoin(_)
        | LogicalPlan::System { .. }
        | LogicalPlan::Values { .. }
        | LogicalPlan::Limit { .. }
        | LogicalPlan::Recursive { .. }
        | LogicalPlan::WorkTable { .. }
        | LogicalPlan::Transition { .. }
        | LogicalPlan::Unnest(_)
        | LogicalPlan::Window(_)
        | LogicalPlan::Recognize(_)
        | LogicalPlan::Polymorphic(_) => false,
    }
}

impl<'a> PhysicalPlanner<'a> {
    /// This planner with no row goal: for an input read whole.
    pub(super) fn whole(&self) -> PhysicalPlanner<'a> {
        PhysicalPlanner { first: None, ..*self }
    }

    /// This planner with its row goal divided by `per`, the rows of its
    /// output each row of an input gives, asked only if it has a goal:
    /// none, if it has none or `per` is not a count of rows. A goal of a
    /// row or more stays one.
    pub(super) fn divided(&self, per: impl FnOnce() -> f64) -> PhysicalPlanner<'a> {
        let Some(n) = self.first else { return *self };
        let per = per();
        let first = (per.is_finite() && per > 0.0_f64).then(|| parallel_rows((cost_of(n) / per).ceil()).max(1));
        PhysicalPlanner { first, ..*self }
    }

    /// The rows each row of `input` gives of an output estimated at `rows`:
    /// the output's over the input's.
    pub(super) fn per_row(&self, rows: f64, input: &LogicalPlan) -> f64 {
        rows / self.card(input).rows.max(1.0_f64)
    }
}

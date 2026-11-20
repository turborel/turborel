//! \[DESIGN\] Conjuncts over a window's partitioning columns moved below it
//! (RULES.md D374). A window function's value of a row is computed over the
//! rows of its window partition alone, the rows not distinct from it in
//! the partitioning columns [ISO/IEC 9075-2:2023 §7.15 GR3, §4.17.15]. A
//! conjunct that reads those columns alone and treats rows not distinct
//! alike (`views.rs` `alike`) keeps a whole partition or none of it, so
//! the partitions it keeps are the same with their rows filtered first,
//! and so is every function's value of each of their rows. A window gives
//! a row for each row of its input, so the conjunct is evaluated on the
//! same rows below it as above it, a conjunct that can raise too. Those
//! left above are then evaluated on the rows of the partitions kept
//! alone, so none of them may raise (RULES.md D15).

use super::views::alike;
use super::{Rule, RuleContext, wrap};
use crate::logical::{Cond, LogicalPlan, Window, cond_columns, split_and};

/// The rule's name, as EXPLAIN shows it and its setting names it.
pub const PUSH_FILTER_THROUGH_WINDOW: &str = "push_filter_through_window";

/// Conjuncts over a window's partitioning columns moved below it, as the
/// module says.
pub struct PushFilterThroughWindow;

impl Rule for PushFilterThroughWindow {
    fn name(&self) -> &'static str {
        PUSH_FILTER_THROUGH_WINDOW
    }
    fn apply(&self, plan: &LogicalPlan, _ctx: &RuleContext<'_>) -> Option<LogicalPlan> {
        let LogicalPlan::Filter { cond, input } = plan else { return None };
        let LogicalPlan::Window(window) = &**input else { return None };
        let mut conjuncts = Vec::new();
        split_and(cond.clone(), &mut conjuncts);
        let (down, keep): (Vec<Cond>, Vec<Cond>) = conjuncts.into_iter().partition(|c| partitioned(c, &window.partition));
        if down.is_empty() || keep.iter().any(Cond::can_raise) {
            return None;
        }
        let Window { partition, order, calls, input: rows } = &**window;
        let below = Window { partition: partition.clone(), order: order.clone(), calls: calls.clone(), input: wrap(rows.clone(), down) };
        Some(wrap(LogicalPlan::Window(Box::new(below)), keep))
    }
}

/// Whether `c` reads the partitioning columns `partition` alone and treats
/// rows not distinct alike.
fn partitioned(c: &Cond, partition: &[usize]) -> bool {
    let mut columns = Vec::new();
    cond_columns(c, &mut columns);
    columns.iter().all(|col| partition.contains(col)) && alike(c)
}

#[cfg(test)]
mod tests;

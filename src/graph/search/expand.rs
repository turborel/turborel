//! \[DESIGN\] A breadth-first search's levels expanded a batch of states
//! at a time: the hops of every vertex of the batch asked of the view at
//! once (`GraphView::expand`), so a view over tables and an adjacency
//! index reads them in one pass, not one vertex at a time. The states
//! are then taken in the level's order, as before, so the search finds
//! what it found.

use super::states::States;
use crate::error::Result;
use crate::graph::run::Runner;

/// \[DESIGN\] The states whose hops are found at once: enough to read an
/// index's lists in order, few enough that their hops are not many.
const EXPAND_BATCH: usize = 1024;

/// Finds at once the hops of the vertices of `nodes`, states of `states`,
/// in the ways their threads may follow edges.
///
/// # Errors
///
/// Any error of the view.
pub(super) fn prefetch(runner: &mut Runner<'_>, states: &States, nodes: &[u32]) -> Result<()> {
    let asked = nodes.iter().map(|&n| (states.vertex(n), runner.orientation(states.threads(n)))).collect();
    runner.prefetch(asked)
}

/// At the first state of each batch of `nodes`, at `at`, finds the hops of
/// the batch's (`prefetch`), `node` the state of each.
///
/// # Errors
///
/// Any error of the view.
pub(super) fn ahead<T>(runner: &mut Runner<'_>, states: &States, (nodes, at): (&[T], usize), node: impl Fn(&T) -> u32) -> Result<()> {
    if at.checked_rem(EXPAND_BATCH) != Some(0) {
        return Ok(());
    }
    let batch: Vec<u32> = nodes.get(at..at.saturating_add(EXPAND_BATCH).min(nodes.len())).unwrap_or_default().iter().map(node).collect();
    prefetch(runner, states, &batch)
}

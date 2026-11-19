//! \[DESIGN\] What a reachability index's labels are now (RULES.md D116):
//! not made yet, made and exact, or dropped, as an edge was taken back or
//! the memory budget refused them (D38). Unmade, they are made as first
//! read; dropped, by maintenance, at once after a removal, and after a
//! refusal only once as many edges were added as they held, so
//! maintenance does not read the index whole again for nothing.

use std::sync::Arc;

use super::landmarks::Landmarks;
use crate::memory::{MemoryBudget, Reservation};

/// \[DESIGN\] Bytes reserved at a time as the labels grow, so the budget
/// is asked once per megabyte, not per edge.
const RESERVE_STEP: usize = 1 << 20;
/// \[DESIGN\] The edges added after a refusal below which the labels are
/// not made again, whatever they held.
const LEAST_ADDED: usize = 64;
/// What a reservation of them is shown as.
const HOLDER: &str = "a reachability index's labels";

/// The labels' state.
enum State {
    /// Not made since the index was opened or created.
    Unmade,
    /// Made, and exact.
    Made(Landmarks, Option<Reservation>),
    /// Dropped as an edge was taken back.
    Stale,
    /// Dropped as the budget refused them, of this many bytes.
    Refused(usize),
}

/// \[DESIGN\] A reachability index's labels, and the budget they are held of.
pub(super) struct Labels {
    budget: Option<Arc<MemoryBudget>>,
    state: State,
    /// Edges added since they were last refused.
    added: usize,
}

impl Labels {
    /// None yet, to be held of `budget`, if any.
    pub(super) fn new(budget: Option<Arc<MemoryBudget>>) -> Labels {
        Labels { budget, state: State::Unmade, added: 0 }
    }

    /// Whether they are to be made as they are read.
    pub(super) fn unmade(&self) -> bool {
        matches!(self.state, State::Unmade)
    }

    /// Whether maintenance is to make them.
    pub(super) fn due(&self) -> bool {
        match self.state {
            State::Stale => true,
            State::Refused(bytes) => self.added >= LEAST_ADDED.max(bytes.checked_div(RESERVE_STEP).unwrap_or_default()),
            State::Unmade | State::Made(..) => false,
        }
    }

    /// Makes them of `edges`, unless the budget refuses them.
    pub(super) fn build(&mut self, edges: &[(Vec<u8>, Vec<u8>)]) {
        let made = Landmarks::made(edges);
        let bytes = made.bytes().saturating_add(RESERVE_STEP);
        self.added = 0;
        self.state = match self.budget.as_ref().map(|b| b.reserve(bytes, HOLDER)) {
            Some(Ok(r)) => State::Made(made, Some(r)),
            Some(Err(_)) => State::Refused(bytes),
            None => State::Made(made, None),
        };
    }

    /// The edge from `source` to `destination` added, by their keys' bytes.
    pub(super) fn added(&mut self, source: &[u8], destination: &[u8]) {
        self.added = self.added.saturating_add(1);
        let State::Made(made, held) = &mut self.state else { return };
        made.insert(source, destination);
        let wanted = made.bytes();
        let short = held.as_ref().is_some_and(|h| h.bytes() < wanted);
        if short && held.as_mut().is_some_and(|h| h.resize(wanted.saturating_add(RESERVE_STEP)).is_err()) {
            self.state = State::Refused(wanted);
            self.added = 0;
        }
    }

    /// The last version of an edge taken back: the labels may claim a
    /// path that is gone, so they are dropped.
    pub(super) fn removed(&mut self) {
        if matches!(self.state, State::Made(..)) {
            self.state = State::Stale;
        }
    }

    /// What `ask` answers of the labels, if they are made.
    pub(super) fn answer<T>(&self, ask: impl FnOnce(&Landmarks) -> T) -> Option<T> {
        match &self.state {
            State::Made(made, _) => Some(ask(made)),
            State::Unmade | State::Stale | State::Refused(_) => None,
        }
    }
}

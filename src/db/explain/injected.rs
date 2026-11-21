//! \[EXTENSION\] Cardinalities a session gives the planner in place of its
//! estimates (RULES.md D377, `physical/cardinality/injected.rs`). They are
//! set by the library alone, no SQL, and kept with the session, so every
//! statement it plans after uses them, until it gives others or none. The
//! snapshot each of its statements runs in carries them, as it carries
//! the session's own settings.

use std::sync::Arc;

use super::super::{Session, State};
use crate::physical::Injected;

impl Session {
    /// \[EXTENSION\] Gives the planner `injected` in place of its estimates
    /// for every statement the session plans from now, to test it. Each
    /// names a set of a query's relations, by their correlation names, and
    /// the rows of their join with their filters. None given, it estimates
    /// again.
    pub fn with_cardinalities(&self, injected: Injected) {
        self.inner.borrow_mut().injected = (!injected.is_empty()).then(|| Arc::new(injected));
    }

    /// `st`, a snapshot for a statement of the session, with the rows it
    /// gave in place of estimates, if it gave any.
    pub(in crate::db) fn with_injected(&self, st: Arc<State>) -> Arc<State> {
        let Some(injected) = self.inner.borrow().injected.clone() else { return st };
        let mut copy = State::clone(&st);
        copy.optimizer.inject(injected);
        Arc::new(copy)
    }
}

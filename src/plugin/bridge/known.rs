//! \[EXTENSION\] What a plug-in's engine knows of a table, or its method of
//! an index, kept for the planner (RULES.md D278).
//!
//! The first time the planner asks, the program is asked (STATISTICS),
//! and its answer is kept, so the planner's later questions cost no call.
//! It is asked again at the planner's next question once the rows or
//! entries written since reach the count its answer gave to refresh
//! after, and at once when the table is analyzed (`Database::analyze`);
//! an answer of none, or a call that fails, is kept as none until then,
//! so the planner takes its defaults, as of a program that answers no
//! statistics. No lock is held across the call (RULES.md R-ERR-4).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::error::Result;
use crate::sync::lock_builtin;

/// An answer kept, and what makes it stale.
#[derive(Debug)]
struct Held<T> {
    /// Whether the program was asked since it was last made stale.
    asked: bool,
    answer: Option<Arc<T>>,
    /// The writes counted when it was asked.
    at: u64,
    /// The writes after which it is stale; 0 never.
    refresh: u64,
}

/// What a program knows of one table or index, kept.
#[derive(Debug)]
pub(super) struct Known<T> {
    held: Mutex<Held<T>>,
    written: AtomicU64,
}

impl<T> Default for Known<T> {
    fn default() -> Known<T> {
        Known { held: Mutex::new(Held { asked: false, answer: None, at: 0, refresh: 0 }), written: AtomicU64::new(0) }
    }
}

impl<T> Known<T> {
    /// Counts `n` rows or entries written.
    pub(super) fn written(&self, n: u64) {
        self.written.fetch_add(n, Ordering::Relaxed);
    }

    /// Whether the answer kept is stale, or none was asked yet.
    pub(super) fn due(&self) -> bool {
        let held = lock_builtin(&self.held);
        let since = self.written.load(Ordering::Relaxed).saturating_sub(held.at);
        !held.asked || (held.refresh > 0 && since >= held.refresh)
    }

    /// The answer, `ask`ed of the program first if it is due: each answer
    /// with the writes after which it is stale.
    pub(super) fn get(&self, ask: impl FnOnce() -> Result<Option<(T, u64)>>) -> Option<Arc<T>> {
        if !self.due() {
            return lock_builtin(&self.held).answer.clone();
        }
        self.refresh(ask)
    }

    /// The answer `ask`ed of the program now, and kept.
    pub(super) fn refresh(&self, ask: impl FnOnce() -> Result<Option<(T, u64)>>) -> Option<Arc<T>> {
        let at = self.written.load(Ordering::Relaxed);
        let (answer, refresh) = match ask() {
            Ok(Some((t, refresh))) => (Some(Arc::new(t)), refresh),
            Ok(None) | Err(_) => (None, 0),
        };
        *lock_builtin(&self.held) = Held { asked: true, answer: answer.clone(), at, refresh };
        answer
    }
}

#[cfg(test)]
mod tests;

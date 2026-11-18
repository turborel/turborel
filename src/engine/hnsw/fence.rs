//! \[DESIGN\] A fence maintenance raises before writers, so it makes
//! progress under constant writes.
//!
//! Maintenance repairs a copy of the graph with no lock held (R-ERR-4),
//! then swaps it in only if the graph had no change meanwhile. A steady
//! stream of inserts and removes can change it every time, so the copy is
//! let go again and again and maintenance stays due forever. After
//! `MOST_LOST` lost swaps in a row, maintenance raises the fence: writers
//! arriving wait at it, and it waits for those already writing to finish.
//! It then copies, repairs and swaps with no writer between, and lowers
//! the fence. No lock is held while it works or while writers wait; they
//! wait on a condition, as on a lock, and the fence is the graph's last
//! word only for as long as one repair takes.
//!
//! A swap also waits for no writer past the fence, so an insert links its
//! node's back edges in the epoch it was published in, and none is lost.

use std::sync::{Condvar, Mutex, PoisonError};

use crate::error::Result;
use crate::sync::lock;

/// \[DESIGN\] Lost swaps in a row before maintenance fences writers out.
pub(super) const MOST_LOST: u32 = 2;

/// Whether the fence is up, and the writers past it.
#[derive(Default)]
struct Gate {
    raised: bool,
    writing: usize,
}

/// The fence: its gate, and the condition both sides wait on.
#[derive(Default)]
pub(super) struct Fence {
    gate: Mutex<Gate>,
    changed: Condvar,
}

/// A writer past the fence: it lets the fence know as it is dropped.
pub(super) struct Writing<'a>(&'a Fence);

/// The fence raised, writers out: lowered as it is dropped.
pub(super) struct Raised<'a>(&'a Fence);

impl Fence {
    /// Waits while the fence is up, then counts a writer in.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the fence's lock is poisoned.
    pub(super) fn pass(&self) -> Result<Writing<'_>> {
        let mut gate = lock(&self.gate)?;
        while gate.raised {
            gate = self.changed.wait(gate).unwrap_or_else(PoisonError::into_inner);
        }
        gate.writing = gate.writing.saturating_add(1);
        Ok(Writing(self))
    }

    /// Whether no writer is past the fence.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the fence's lock is poisoned.
    pub(super) fn idle(&self) -> Result<bool> {
        Ok(lock(&self.gate)?.writing == 0)
    }

    /// Whether the fence is up.
    #[cfg(test)]
    pub(super) fn is_raised(&self) -> bool {
        lock(&self.gate).is_ok_and(|g| g.raised)
    }

    /// Raises the fence, then waits for the writers past it to finish.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the fence's lock is poisoned.
    pub(super) fn raise(&self) -> Result<Raised<'_>> {
        let mut gate = lock(&self.gate)?;
        while gate.raised {
            gate = self.changed.wait(gate).unwrap_or_else(PoisonError::into_inner);
        }
        gate.raised = true;
        while gate.writing > 0 {
            gate = self.changed.wait(gate).unwrap_or_else(PoisonError::into_inner);
        }
        Ok(Raised(self))
    }
}

impl Drop for Writing<'_> {
    fn drop(&mut self) {
        let mut gate = self.0.gate.lock().unwrap_or_else(PoisonError::into_inner);
        gate.writing = gate.writing.saturating_sub(1);
        drop(gate);
        self.0.changed.notify_all();
    }
}

impl Drop for Raised<'_> {
    fn drop(&mut self) {
        self.0.gate.lock().unwrap_or_else(PoisonError::into_inner).raised = false;
        self.0.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] a raised fence waits for the writers past it, and
    /// holds new ones out until it is lowered.
    #[test]
    fn a_raised_fence_waits_for_writers_and_holds_new_ones_out() {
        let fence = Fence::default();
        let writer = fence.pass().unwrap();
        std::thread::scope(|scope| {
            let raised = scope.spawn(|| {
                let up = fence.raise().unwrap();
                assert_eq!(lock(&fence.gate).unwrap().writing, 0);
                drop(up);
            });
            drop(writer);
            raised.join().unwrap();
        });
        let gate = lock(&fence.gate).unwrap();
        assert!(!gate.raised && gate.writing == 0);
        drop(gate);
        let up = fence.raise().unwrap();
        std::thread::scope(|scope| {
            let late = scope.spawn(|| {
                let _w = fence.pass().unwrap();
                assert!(!lock(&fence.gate).unwrap().raised);
            });
            drop(up);
            late.join().unwrap();
        });
    }
}

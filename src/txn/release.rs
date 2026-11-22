//! A transaction's end, as its locks see it: what it holds released, and
//! those waiting for it woken.

use std::mem;
use std::sync::atomic::Ordering;

use super::Txn;
use crate::sync::lock_builtin;

impl Txn {
    /// Releases every table lock it holds and wakes the transactions waiting
    /// for them, and forgets its snapshot. Called when it ends, and again
    /// when it is dropped, which finds nothing left to release.
    pub(super) fn release(&self) {
        // [DESIGN] RULES.md D452: its increments reserved end with it,
        // before the writers waiting for them go on.
        for escrow in mem::take(&mut lock_builtin(&self.state).escrowed) {
            escrow.end(self.id);
        }
        let held = mem::take(&mut lock_builtin(&self.state).held);
        for table in held {
            table.release(self.id);
        }
        // [DESIGN] RULES.md D442: the waiters for its rows go on, in turn.
        self.origin.shared.ends.end(self.id);
        let pinned = mem::take(&mut lock_builtin(&self.state).pinned);
        if self.open.swap(false, Ordering::AcqRel) {
            self.origin.shared.clock.end(self.id, self.snapshot());
            for at in pinned {
                self.origin.shared.clock.end(self.id, at);
            }
        }
    }
}

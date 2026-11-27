//! A transaction's end, as its locks see it: what it holds released, and
//! those waiting for it woken.
//!
//! \[DESIGN\] And its end when it is dropped without one (RULES.md
//! D1259). A transaction is ended by its commit or its rollback, which
//! clear `open` as they release. One dropped still open, as an error
//! before either leaves it, would leave its versions in the engines with
//! its locks gone, no engine told and no end record. So the drop ends it.
//! If it joined an engine it is rolled back, as `State::rollback` rolls
//! one back, under the gate. If that cannot be done, as while a panic
//! unwinds, with its state's lock poisoned, its commit under way or a
//! change that cannot be undone, the tables it holds are marked damaged
//! and the database stops taking changes, so no checkpoint captures a
//! version it left and nothing is committed over one.
//!
//! The rollback takes the gate, the transaction's own state, its engines
//! and the log, so it must run where the thread holds none of them but
//! the gate, which its holder takes again without waiting (`sync::Gate`).
//! A transaction is held by its session alone, as its own, as the one its
//! running call notes and in a routine's host, and by the database's list
//! of commits in doubt, which are not open. No executor, cursor or plan
//! holds one. So the last reference to an open transaction that joined
//! an engine is dropped in one of three places, the thread in no engine
//! at any of them.
//! A session dropped with no call to enter ends its transaction itself,
//! its cursors closed first, at the start of its drop
//! (`Session::end_unentered`). A call that ends a transaction drops it as
//! the end returns, when the end returned early with nothing undone
//! (`Session::rollback_in`, `Session::commit_in`). And a function that
//! begins a transaction of its own drops it as it returns. Those that
//! only hold locks joined nothing, and the others end theirs on every
//! path (`db/assertions.rs`).

use std::mem;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::Ordering;

use super::Txn;
use crate::error::{Code, Result, SqlError};
use crate::memory::Undoing;
use crate::sync::lock_builtin;

/// What a transaction dropped still open left to end.
enum Left {
    /// It joined no engine and holds no undo, as one that only holds
    /// locks does.
    Nothing,
    /// Changes or reads in the engines it joined.
    Joined,
    /// Undo entries and no participant, its commit having taken them.
    Committing,
}

/// The failure that stops the database: a transaction dropped before its
/// end could not be rolled back, `why`.
fn unended(why: &str) -> SqlError {
    let told = format!("[DESIGN] a transaction dropped before its end could not be rolled back, so the tables it held are damaged: {why}");
    SqlError::new(Code::Damaged, told)
}

impl Drop for Txn {
    fn drop(&mut self) {
        self.abandon();
        // Only one whose records were written before its end has a first
        // to forget.
        if self.forget_held() {
            lock_builtin(&self.origin.shared.firsts).remove(&self.id);
        }
        self.origin.shared.stops.remove(self.id);
        self.origin.shared.clock.dropped(self.id);
    }
}

impl Txn {
    /// \[DESIGN\] RULES.md D1259: ends it if it is still open, as its
    /// drop does, and releases what it holds: for a session dropped with
    /// no call to enter, which ends its transaction while it still holds
    /// it, whoever else does. One that has ended is left as it is.
    pub(crate) fn abandon(&self) {
        if self.open.load(Ordering::Acquire) {
            self.end_dropped();
        }
        self.release();
    }

    /// What it left to end, dropped still open. Its state is read whether
    /// or not its lock is poisoned.
    fn left(&self) -> Left {
        let st = lock_builtin(&self.state);
        match (st.joined.is_empty(), st.undo.is_empty()) {
            (true, true) => Left::Nothing,
            (true, false) => Left::Committing,
            (false, true) | (false, false) => Left::Joined,
        }
    }

    /// Ends it, dropped still open: see the module's documentation. Its
    /// locks are released by the drop after.
    fn end_dropped(&self) {
        let ended = match self.left() {
            Left::Nothing => return,
            Left::Committing => Err(unended("its commit was under way")),
            Left::Joined => self.rolled_back_dropped(),
        };
        if let Err(e) = ended {
            for table in &lock_builtin(&self.state).held {
                table.damaged(true);
            }
            self.origin.shared.stop_changes(&e);
        }
    }

    /// Rolls it back, dropped still open, unless that cannot be trusted:
    /// while a panic unwinds, when a second panic would end the process,
    /// or with its state's lock poisoned, its bookkeeping left half
    /// changed. A panic of the rollback itself is caught.
    ///
    /// # Errors
    ///
    /// SQLCODE -914 if it was not rolled back whole.
    fn rolled_back_dropped(&self) -> Result<()> {
        if std::thread::panicking() || self.state.is_poisoned() {
            return Err(unended("a panic left it"));
        }
        // Never refused by the memory budget (RULES.md D38).
        let _undoing = Undoing::begin();
        match catch_unwind(AssertUnwindSafe(|| self.undo_whole())) {
            Ok(Ok((failures, _))) => failures.first().map_or(Ok(()), |f| Err(unended(&f.error.described()))),
            Ok(Err(e)) => Err(unended(&e.described())),
            Err(_) => Err(unended("its rollback panicked")),
        }
    }

    /// Releases every table lock it holds and wakes the transactions waiting
    /// for them, and forgets its snapshot. Called when it ends, and again
    /// when it is dropped, which finds nothing left to release.
    pub(super) fn release(&self) {
        // What it holds, taken under its lock once.
        let (escrowed, held, pinned) = {
            let mut st = lock_builtin(&self.state);
            (mem::take(&mut st.escrowed), mem::take(&mut st.held), mem::take(&mut st.pinned))
        };
        // [DESIGN] RULES.md D452: its increments reserved end with it,
        // before the writers waiting for them go on.
        for escrow in escrowed {
            escrow.end(self.id);
        }
        for table in held {
            table.release(self.id);
        }
        // [DESIGN] RULES.md D442: the waiters for its rows go on, in turn.
        self.origin.shared.ends.end(self.id);
        if self.open.swap(false, Ordering::AcqRel) {
            self.origin.shared.clock.end(self.id, self.snapshot());
            for at in pinned {
                self.origin.shared.clock.end(self.id, at);
            }
        }
    }
}

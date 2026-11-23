//! \[DESIGN\] When a commit is durable, and what waits for it (RULES.md
//! D25, D441). A commit makes its rows committed and releases its locks
//! before its commit record is synced, so a row's next writer goes on at
//! once. Snapshots read at the durable timestamp (`clock.rs`), so no reader
//! sees a commit a crash could lose. A transaction learns of a commit not
//! yet durable only where it reads past its snapshot: a READ COMMITTED
//! write over the newest committed version, a uniqueness or referential
//! check of the newest state, or a refusal (SQLCODE -911) a newer commit
//! caused. Each marks the commit it read (`Txn::depend`), and the
//! transaction tells its session nothing until that commit is durable
//! (`Txn::replied`). A failed sync turns the answer into SQLCODE -922.

use std::sync::atomic::Ordering;

use super::{Timestamp, Transactions, Txn};
use crate::error::{Code, Result, SqlError, SqlState};

/// A commit under way, holding its timestamp: until it is dropped, no
/// snapshot counts commits at or after its timestamp as ended.
pub(crate) struct Committing<'a> {
    shared: &'a Transactions,
    /// Its commit timestamp.
    pub(crate) at: Timestamp,
    /// Whether its commit record is synced after it ends, which then marks
    /// it durable; else it is durable as it ends.
    syncing: bool,
}

impl Committing<'_> {
    /// Its commit record is synced once it has ended, or it is in doubt:
    /// it is not durable as it ends.
    pub(super) fn syncs(&mut self) {
        self.syncing = true;
    }
}

impl Drop for Committing<'_> {
    fn drop(&mut self) {
        self.shared.clock.end_commit(self.at);
        if !self.syncing {
            self.shared.clock.end_syncs(std::iter::once(self.at));
        }
    }
}

/// SQLCODE -922: what the session would be told rests on a commit a failed
/// sync may have lost.
fn in_doubt() -> SqlError {
    SqlError::new(
        Code::InDoubt,
        "[DESIGN] a sync of the log failed before a commit this rests on was durable, so whether it holds is known only once the database is opened again and recovered from its log",
    )
    .with_state(SqlState::TRANSACTION_RESOLUTION_UNKNOWN)
}

impl Transactions {
    /// A commit timestamp, later than every one given out before, held
    /// until the commit ends.
    pub(super) fn commit_timestamp(&self) -> Committing<'_> {
        Committing { shared: self, at: self.clock.take_commit(), syncing: false }
    }

    /// Returns once every commit up to `at` is durable, so a snapshot
    /// taken after counts it.
    ///
    /// # Errors
    ///
    /// SQLCODE -922 if a failed sync halted the durable timestamp first.
    pub(super) fn wait_durable(&self, at: Timestamp) -> Result<()> {
        if self.clock.wait_durable(at) { Ok(()) } else { Err(in_doubt()) }
    }
}

impl Txn {
    /// It read a version committed at `at` past its snapshot: nothing it
    /// tells its session goes out before that commit is durable.
    pub(crate) fn depend(&self, at: Timestamp) {
        if at > self.origin.shared.clock.durable() {
            self.depends.fetch_max(at, Ordering::SeqCst);
        }
    }

    /// `result`, told to its session once every commit it depends on is
    /// durable.
    ///
    /// # Errors
    ///
    /// `result`'s; SQLCODE -922 in its place if a failed sync halted the
    /// durable timestamp before it reached them.
    pub(crate) fn replied<T>(&self, result: Result<T>) -> Result<T> {
        let at = self.depends.load(Ordering::SeqCst);
        if at == 0 {
            return result;
        }
        // [DESIGN] RULES.md D520: shown waiting for I/O meanwhile.
        self.stop.activity().waits_for_io();
        let durable = self.origin.shared.wait_durable(at);
        self.stop.activity().woke();
        durable.and(result)
    }
}

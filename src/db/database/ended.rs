//! \[EXTENSION\] A database's transactions ended (RULES.md D334), how many
//! committed and how many rolled back since it was opened, of every
//! session, each counted as its session's diagnostics area counts it
//! [ISO/IEC 9075-2:2023 §23.1 GR2h, GR2i]; those in doubt, held until it
//! is dropped (RULES.md D25), counted neither. Opened again in place, it
//! counts on. A handle to the counts reads them as they grow, as
//! INFORMATION_SCHEMA.SERVER does.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, PoisonError};

use super::Database;
use crate::txn::Txn;

/// The counts of transactions ended.
#[derive(Default)]
pub(in crate::db) struct Ended {
    /// Those committed.
    committed: AtomicU64,
    /// Those rolled back.
    rolled_back: AtomicU64,
}

/// How many transactions a database ended since it was opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TransactionsEnded {
    /// Those committed.
    pub committed: u64,
    /// Those rolled back.
    pub rolled_back: u64,
}

impl Ended {
    /// The counts now.
    fn read(&self) -> TransactionsEnded {
        TransactionsEnded { committed: self.committed.load(Ordering::Relaxed), rolled_back: self.rolled_back.load(Ordering::Relaxed) }
    }
}

/// A database's counts of transactions ended, read as they grow, by a
/// handle that keeps no more of the database alive.
#[derive(Clone)]
pub struct EndedCounts(Arc<Ended>);

impl EndedCounts {
    /// How many transactions the database committed and rolled back since
    /// it was opened, of every session.
    pub fn now(&self) -> TransactionsEnded {
        self.0.read()
    }
}

impl Database {
    /// Counts a transaction ended, committed or not.
    pub(in crate::db) fn count_ended(&self, committed: bool) {
        let ended = &self.0.ended;
        let count = if committed { &ended.committed } else { &ended.rolled_back };
        // A count past u64 would take longer than any database runs; it
        // wraps rather than fails.
        count.fetch_add(1, Ordering::Relaxed);
    }

    /// How many transactions it committed and rolled back since it was
    /// opened, of every session.
    pub fn transactions_ended(&self) -> TransactionsEnded {
        self.0.ended.read()
    }

    /// Its counts of transactions ended, as a handle that reads them.
    pub fn ended_counts(&self) -> EndedCounts {
        EndedCounts(Arc::clone(&self.0.ended))
    }

    /// Holds `tx`, in doubt, until the database is dropped (RULES.md D25).
    pub(in crate::db) fn hold_in_doubt(&self, tx: Arc<Txn>) {
        // Entries are added whole, so a poisoned list is whole.
        self.0.in_doubt.lock().unwrap_or_else(PoisonError::into_inner).push(tx);
    }
}

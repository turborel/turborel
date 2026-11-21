//! \[DESIGN\] What a session does now, and what its statements have done
//! (RULES.md D520, D521), kept in its `Stop` because what reads it, a
//! server's INFORMATION_SCHEMA.CONNECTIONS or the database's
//! STATEMENT_STATISTICS, reaches a session only through it, and its
//! transaction and lock waits reach it there too.
//!
//! * The statement it runs now, or ran last: its text, made once for a
//!   prepared statement and once for each text run directly
//!   (`Activity::text_of`), and when it began.
//! * Its open transaction and when it began, as the transaction begins
//!   and ends (`Stops::insert`, `Stops::remove`).
//! * What it waits on: a lock and the transaction holding it, set as a
//!   wait begins and cleared as it ends (`txn/locks.rs`,
//!   `txn/row_waits.rs`), or the log's sync (`txn/logging.rs`).
//! * The rows its scans read and the bytes its work areas spilled, from
//!   its start: one count each, which a statement's tally and EXPLAIN
//!   ANALYZE both read as differences.
//! * The tally of its statements, by text and user (`Tallies`): only it
//!   writes it, so a statement's end takes a lock no other session takes
//!   but while STATEMENT_STATISTICS is read.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use super::TxId;
use crate::hash::murmur::{MurmurMap, murmur3_x64_128};
use crate::sync::lock_builtin;
use crate::system::Text;

mod tallies;

pub use tallies::{Tallied, Tallies, Tally};

/// \[DESIGN\] The texts run directly a session keeps the normalization of:
/// past them the oldest go, all at once, as a session running many texts
/// once each gains nothing from keeping them.
const TEXTS_KEPT: usize = 256;

/// What a session waits on, if anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Waiting {
    /// A lock of a table, or a row, `what`, which the transaction `holder`
    /// holds.
    Lock {
        /// The table or row waited for, as a refusal of the wait names it.
        what: String,
        /// The transaction holding it, the first if several do.
        holder: TxId,
    },
    /// The log's sync, of its commit or of a commit it read.
    Io,
}

/// The values of `Waiting` as one atomic holds them.
const NOT_WAITING: u8 = 0;
/// See [`NOT_WAITING`].
const WAITING_LOCK: u8 = 1;
/// See [`NOT_WAITING`].
const WAITING_IO: u8 = 2;

/// What a session does now, as `Activity::now` reads it.
#[derive(Debug, Clone)]
pub struct Now {
    /// The statement it runs or ran last, and when it began, as a
    /// timestamp's instant (`value::datetime::Now::utc`).
    pub statement: Option<(Arc<Text>, i64)>,
    /// Its open transaction, and when it began.
    pub transaction: Option<(TxId, i64)>,
    /// What it waits on.
    pub waiting: Option<Waiting>,
}

/// What a session does now, and what its statements did.
#[derive(Debug, Default)]
pub struct Activity {
    statement: Mutex<Option<(Arc<Text>, i64)>>,
    /// Its open transaction, 0 for none.
    transaction: AtomicU64,
    /// When it began.
    transaction_began: AtomicI64,
    waiting: AtomicU8,
    lock: Mutex<Option<(String, TxId)>>,
    read: AtomicU64,
    spilled: AtomicU64,
    tallies: Arc<Mutex<Tallies>>,
    /// Whether `tallies` is listed with the database's (`Statements`).
    listed: AtomicBool,
    texts: Mutex<MurmurMap<u64, Arc<Text>>>,
    /// The user its statements last ran as.
    user: tallies::User,
}

impl Activity {
    /// The text of `sql`, run directly: made once while it is kept.
    pub fn text_of(&self, sql: &str) -> Arc<Text> {
        let (hash, _) = murmur3_x64_128(sql.as_bytes(), 0);
        let mut texts = lock_builtin(&self.texts);
        if let Some(text) = texts.get(&hash).filter(|t| t.raw() == sql) {
            return Arc::clone(text);
        }
        if texts.len() >= TEXTS_KEPT {
            texts.clear();
        }
        let text = Arc::new(Text::of(sql));
        texts.insert(hash, Arc::clone(&text));
        text
    }

    /// The statement `text` began at `utc`.
    pub fn began(&self, text: &Arc<Text>, utc: i64) {
        let mut statement = lock_builtin(&self.statement);
        // The same text again takes no count of it, which other sessions
        // may share.
        if let Some((had, at)) = statement.as_mut()
            && Arc::ptr_eq(had, text)
        {
            *at = utc;
            return;
        }
        *statement = Some((Arc::clone(text), utc));
    }

    /// The transaction `tx` began, in the statement running.
    pub(crate) fn began_transaction(&self, tx: TxId) {
        let utc = lock_builtin(&self.statement).as_ref().map_or(0, |s| s.1);
        self.transaction_began.store(utc, Ordering::Relaxed);
        self.transaction.store(tx, Ordering::Release);
    }

    /// The transaction `tx` ended, if it is the open one.
    pub(crate) fn ended_transaction(&self, tx: TxId) {
        // Another may have begun since, of a session that ended this one
        // on another thread: it stays.
        let _ = self.transaction.compare_exchange(tx, 0, Ordering::AcqRel, Ordering::Relaxed);
    }

    /// A wait for `what`, which `holder` holds, begins.
    pub(crate) fn waits_for_lock(&self, what: String, holder: TxId) {
        *lock_builtin(&self.lock) = Some((what, holder));
        self.waiting.store(WAITING_LOCK, Ordering::Release);
    }

    /// A wait for the log's sync begins.
    pub(crate) fn waits_for_io(&self) {
        self.waiting.store(WAITING_IO, Ordering::Release);
    }

    /// The wait ended.
    pub(crate) fn woke(&self) {
        self.waiting.store(NOT_WAITING, Ordering::Release);
    }

    /// `rows` more rows read by its scans.
    pub(crate) fn read(&self, rows: u64) {
        self.read.fetch_add(rows, Ordering::Relaxed);
    }

    /// `bytes` more bytes put in a work area's spill area.
    pub(crate) fn spilled(&self, bytes: usize) {
        self.spilled.fetch_add(u64::try_from(bytes).unwrap_or(u64::MAX), Ordering::Relaxed);
    }

    /// The rows read and the bytes spilled from its start.
    pub fn counts(&self) -> (u64, u64) {
        (self.read.load(Ordering::Relaxed), self.spilled.load(Ordering::Relaxed))
    }

    /// What it does now.
    pub fn now(&self) -> Now {
        let statement = lock_builtin(&self.statement).clone();
        let tx = self.transaction.load(Ordering::Acquire);
        let transaction = (tx != 0).then(|| (tx, self.transaction_began.load(Ordering::Relaxed)));
        let waiting = match self.waiting.load(Ordering::Acquire) {
            WAITING_LOCK => lock_builtin(&self.lock).clone().map(|(what, holder)| Waiting::Lock { what, holder }),
            WAITING_IO => Some(Waiting::Io),
            _ => None,
        };
        Now { statement, transaction, waiting }
    }

    /// Its tallies, and whether this is the first time they are asked to
    /// be listed with the database's.
    pub fn tallies(&self) -> (&Arc<Mutex<Tallies>>, bool) {
        let first = if self.listed.load(Ordering::Relaxed) { false } else { !self.listed.swap(true, Ordering::Relaxed) };
        (&self.tallies, first)
    }
}

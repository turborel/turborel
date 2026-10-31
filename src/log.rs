//! \[DESIGN\] The log (RULES.md D25): an ordered, durable record of what
//! the database changed, the one commit point of every transaction. The
//! database owns the protocol and the records' format; where the records
//! go is an extension point, `Log`.
//!
//! Built in: `MemoryLog`, which keeps records in memory and is not durable
//! (the behaviour of a database whose engines keep no data between runs),
//! and `FileLog` (`log/file.rs`), segments of block files in a directory.
//!
//! To a `Log`, a record is bytes; what the database writes in them, row
//! changes, schema changes, settings, users, checkpoints and the ends of
//! transactions, is `record`; a record of a table with encodings is
//! sealed through them (`sealed`).

mod file;
pub mod record;
pub mod sealed;

use std::collections::VecDeque;
use std::sync::Mutex;

pub use file::{
    DEFAULT_DURABILITY, DEFAULT_SEGMENT_SIZE, FileLog, FileLogOptions, LogCounts, LogRecovery, MAX_SEGMENT_SIZE, MIN_SEGMENT_SIZE,
};

use crate::disk::io::Durability;
use crate::error::{Code, Result, SqlError};
use crate::sync::lock_builtin;

/// \[DESIGN\] A log position: where a record is. Positions only increase in
/// the order records are appended; the database orders commits, and engines
/// their row versions, by them (RULES.md D25, D29).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Lsn(pub u64);

/// What a log can promise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogCapabilities {
    /// Whether what `sync` returns for survives a crash.
    pub durable: bool,
}

/// The records of a log from a position, in order, each with its position.
pub type Records<'a> = Box<dyn Iterator<Item = Result<(Lsn, Vec<u8>)>> + 'a>;

/// \[DESIGN\] A log: an extension point (RULES.md D25, R-SAFE-3).
///
/// Contract (checked for the built-in logs by `test/integration/log.rs`,
/// the same tests for each):
/// * `append` returns a position higher than that of every record whose
///   append returned before it began; records whose appends overlap in time
///   may take their positions in either order.
/// * After `sync(p)` returns, the record at `p`, and every record whose
///   append returned before `sync` began, survive a crash, if the log is
///   durable.
/// * `read_from(p)` yields every record at or after `p`, each with the
///   position `append` returned and the bytes appended, in position order.
///   `p` is 0, a position `append` returned, or `end`.
/// * `read_at(p)` returns the record `append` returned `p` for, while
///   the log holds it, and none for a position it never gave.
/// * `truncate_before(p)` may remove records before `p`, never one at or
///   after it.
/// * Methods may be called from several threads at once (`Send + Sync`), and
///   never call back into the database (RULES.md R-ERR-4).
pub trait Log: Send + Sync {
    /// What the log promises now: a log whose durability changes while it
    /// is open (`set_durability`) tells the one it keeps now, which the
    /// Strict policy reads at each write (RULES.md D25).
    fn capabilities(&self) -> LogCapabilities;
    /// Makes its syncs from now on as durable as `durability` says, as far
    /// as it can: the database's LOG.DURABILITY, told as the database is
    /// given the log and after each change of it (RULES.md D25, D36). What
    /// it then promises, `capabilities` tells. Default: nothing changes, a
    /// log that keeps one durability keeping it.
    fn set_durability(&self, _durability: Durability) {}
    /// Appends `record`; its position.
    ///
    /// # Errors
    ///
    /// The log cannot store it (SQLCODE -915 for an I/O error).
    fn append(&self, record: &[u8]) -> Result<Lsn>;
    /// Makes the record at `through`, and every earlier one, durable.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if `through` is not a position of this log; SQLCODE -915
    /// if the sync fails, when nothing more is promised: the database then
    /// takes no change until it is opened again (RULES.md D25). A sync
    /// tried again after one failed may succeed without what the failed
    /// one lost, so the file log fails every later sync too.
    fn sync(&self, through: Lsn) -> Result<()>;
    /// The records at or after `from`, in order.
    fn read_from(&self, from: Lsn) -> Records<'_>;
    /// The record at `at`, if the log holds one there. Default: the first
    /// record `read_from(at)` yields, if it is at `at`.
    ///
    /// # Errors
    ///
    /// As `read_from`'s records.
    fn read_at(&self, at: Lsn) -> Result<Option<Vec<u8>>> {
        let first = self.read_from(at).next().transpose()?;
        Ok(first.filter(|(lsn, _)| *lsn == at).map(|(_, record)| record))
    }
    /// Removes records before `before`, as far as the log stores them in
    /// pieces it can remove whole.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if what is to be removed cannot be.
    fn truncate_before(&self, before: Lsn) -> Result<()>;
    /// The position after the last record appended.
    fn end(&self) -> Lsn;
}

/// \[DESIGN\] A log in memory: records survive only as long as the process,
/// whatever LOG.DURABILITY says (`Log::set_durability`, not overridden).
/// Positions are 1, 2, 3, ...; appends take turns under one short lock.
#[derive(Debug, Default)]
pub struct MemoryLog {
    state: Mutex<MemoryState>,
}

#[derive(Debug, Default)]
struct MemoryState {
    /// The records kept, oldest first.
    records: VecDeque<(Lsn, Vec<u8>)>,
    /// The last position given.
    last: u64,
}

impl MemoryLog {
    /// An empty log.
    pub fn new() -> MemoryLog {
        MemoryLog::default()
    }
}

impl Log for MemoryLog {
    fn capabilities(&self) -> LogCapabilities {
        LogCapabilities { durable: false }
    }
    fn append(&self, record: &[u8]) -> Result<Lsn> {
        let mut state = lock_builtin(&self.state);
        state.last = state.last.checked_add(1).ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] the log has no position left"))?;
        let lsn = Lsn(state.last);
        state.records.push_back((lsn, record.to_vec()));
        Ok(lsn)
    }
    fn sync(&self, through: Lsn) -> Result<()> {
        if through.0 > lock_builtin(&self.state).last {
            return Err(SqlError::new(Code::Api, format!("[DESIGN] the log has no position {} yet", through.0)));
        }
        Ok(())
    }
    fn read_from(&self, from: Lsn) -> Records<'_> {
        let state = lock_builtin(&self.state);
        let records: Vec<(Lsn, Vec<u8>)> = state.records.iter().filter(|(lsn, _)| *lsn >= from).cloned().collect();
        Box::new(records.into_iter().map(Ok))
    }
    /// Positions are consecutive, so a record is found by its distance from
    /// the first kept.
    fn read_at(&self, at: Lsn) -> Result<Option<Vec<u8>>> {
        let state = lock_builtin(&self.state);
        let first = state.records.front().map_or(0, |(lsn, _)| lsn.0);
        let found = at.0.checked_sub(first).and_then(|i| usize::try_from(i).ok()).and_then(|i| state.records.get(i));
        Ok(found.filter(|(lsn, _)| *lsn == at).map(|(_, record)| record.clone()))
    }
    fn truncate_before(&self, before: Lsn) -> Result<()> {
        let mut state = lock_builtin(&self.state);
        while state.records.front().is_some_and(|(lsn, _)| *lsn < before) {
            state.records.pop_front();
        }
        Ok(())
    }
    fn end(&self) -> Lsn {
        Lsn(lock_builtin(&self.state).last.saturating_add(1))
    }
}

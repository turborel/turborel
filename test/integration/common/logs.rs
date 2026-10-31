//! A log plug-in for the tests of the log's users: a memory log that
//! records its syncs and fails as told. Split from common.rs (RULES.md
//! R-SIZE-2).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use turborel::error::{Code, Result, SqlError};
use turborel::log::{Log, LogCapabilities, Lsn, MemoryLog, Records};

/// A memory log that records which positions were synced, and fails its
/// appends or syncs while told to.
#[derive(Default)]
pub struct Watched {
    pub inner: MemoryLog,
    pub synced: Mutex<Vec<Lsn>>,
    pub fail_appends: AtomicBool,
    pub fail_syncs: AtomicBool,
    /// Whether it says it is durable: by default, not.
    pub durable: bool,
    /// Appends that still succeed before the rest fail, if set.
    pub appends_left: Mutex<Option<usize>>,
}

impl Log for Watched {
    fn capabilities(&self) -> LogCapabilities {
        LogCapabilities { durable: self.durable }
    }
    fn append(&self, record: &[u8]) -> Result<Lsn> {
        let mut left = self.appends_left.lock().unwrap();
        if self.fail_appends.load(Ordering::SeqCst) || *left == Some(0) {
            return Err(SqlError::new(Code::Io, "injected: the log cannot append"));
        }
        if let Some(n) = left.as_mut() {
            *n -= 1;
        }
        drop(left);
        self.inner.append(record)
    }
    fn sync(&self, through: Lsn) -> Result<()> {
        if self.fail_syncs.load(Ordering::SeqCst) {
            return Err(SqlError::new(Code::Io, "injected: the log cannot sync"));
        }
        self.synced.lock().unwrap().push(through);
        self.inner.sync(through)
    }
    fn read_from(&self, from: Lsn) -> Records<'_> {
        self.inner.read_from(from)
    }
    fn truncate_before(&self, before: Lsn) -> Result<()> {
        self.inner.truncate_before(before)
    }
    fn end(&self) -> Lsn {
        self.inner.end()
    }
}

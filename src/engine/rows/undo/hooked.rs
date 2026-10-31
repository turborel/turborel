//! A log for the tests of the undo store's races, shared by those of the
//! store itself and of the passes reading it (`zones/tests.rs`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::error::{Code, Result, SqlError};
use crate::log::{Log, LogCapabilities, Lsn, MemoryLog, Records};
use crate::sync::lock_builtin;

/// What a log does once, at a point a race would fall on.
pub(in crate::engine::rows) type Hook = Mutex<Option<Box<dyn FnOnce() + Send>>>;

/// A log in memory, counting its records, whose appends fail while told
/// to; and which does what it is given once, as its end is next asked for
/// (`on_end`), once its next record is appended (`on_append`), or as a
/// record is next read by its position, before it is (`on_read`).
#[derive(Default)]
pub(in crate::engine::rows) struct Hooked {
    log: MemoryLog,
    pub(in crate::engine::rows) failing: AtomicBool,
    pub(in crate::engine::rows) on_end: Hook,
    pub(in crate::engine::rows) on_append: Hook,
    pub(in crate::engine::rows) on_read: Hook,
}

/// Does what `hook` holds, if anything, once.
fn fire(hook: &Hook) {
    let taken = lock_builtin(hook).take();
    if let Some(work) = taken {
        work();
    }
}

impl Log for Hooked {
    fn capabilities(&self) -> LogCapabilities {
        self.log.capabilities()
    }
    fn append(&self, record: &[u8]) -> Result<Lsn> {
        if self.failing.load(Ordering::Relaxed) {
            return Err(SqlError::new(Code::Io, "[DESIGN] told to fail"));
        }
        let at = self.log.append(record)?;
        fire(&self.on_append);
        Ok(at)
    }
    fn sync(&self, through: Lsn) -> Result<()> {
        self.log.sync(through)
    }
    fn read_from(&self, from: Lsn) -> Records<'_> {
        self.log.read_from(from)
    }
    fn read_at(&self, at: Lsn) -> Result<Option<Vec<u8>>> {
        fire(&self.on_read);
        self.log.read_at(at)
    }
    fn truncate_before(&self, before: Lsn) -> Result<()> {
        self.log.truncate_before(before)
    }
    fn end(&self) -> Lsn {
        fire(&self.on_end);
        self.log.end()
    }
}

impl Hooked {
    /// The records the log holds.
    pub(in crate::engine::rows) fn records(&self) -> usize {
        self.log.read_from(Lsn(0)).count()
    }

    /// The bytes of each record the log holds.
    pub(in crate::engine::rows) fn sizes(&self) -> Vec<usize> {
        self.log.read_from(Lsn(0)).map(|r| r.map_or(0, |(_, record)| record.len())).collect()
    }
}

/// `log`, as the undo store takes one.
pub(in crate::engine::rows) fn shared(log: &Arc<Hooked>) -> Arc<dyn Log> {
    let copy: Arc<dyn Log> = Arc::<Hooked>::clone(log);
    copy
}

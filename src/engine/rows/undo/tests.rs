//! Unit tests of the undo store's batches: versions read wherever their
//! batch is, the log truncated by batch, and a batch that cannot be
//! written; and a truncation racing an append. Split from engine/rows/undo.rs (RULES.md R-SIZE-2).

use std::sync::atomic::AtomicBool;

use super::*;
use crate::log::{LogCapabilities, MemoryLog, Records};

/// What a log does once, at a point a race would fall on.
type Hook = Mutex<Option<Box<dyn FnOnce() + Send>>>;

/// A log in memory, counting its records, whose appends fail while told
/// to; and which does what it is given once, as its end is next asked for
/// (`on_end`), or once its next record is appended (`on_append`).
#[derive(Default)]
struct Hooked {
    log: MemoryLog,
    failing: AtomicBool,
    on_end: Hook,
    on_append: Hook,
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
    fn records(&self) -> usize {
        self.log.read_from(Lsn(0)).count()
    }
}

/// `log`, as the undo store takes one.
fn shared(log: &Arc<Hooked>) -> Arc<dyn Log> {
    let copy: Arc<dyn Log> = Arc::<Hooked>::clone(log);
    copy
}

/// Version `n`: its number, ten bytes of it.
fn version(n: u32) -> Vec<u8> {
    n.to_le_bytes().repeat(10)
}

/// Verifies: [DESIGN] RULES.md D31, the undo store's batches: versions are
/// gathered into a record of the log once they hold the batch's bytes, so
/// a hundred versions of forty bytes make few records; each is read back,
/// from the batch being gathered or from the log, as it was appended; a
/// store of no batches writes each version alone.
#[test]
fn batched_versions_read_back_as_appended() {
    let log = Arc::new(Hooked::default());
    let store = UndoStore::batched(shared(&log), 400);
    let at: Vec<Lsn> = (0_u32..100_u32).map(|n| store.append(&version(n)).unwrap()).collect();
    assert_eq!(log.records(), 10, "ten versions of forty bytes to a batch of 400");
    for (n, pos) in (0_u32..100_u32).zip(&at) {
        assert_eq!(store.read(*pos).unwrap(), version(n), "{n}");
    }
    let gathering = store.append(&version(100)).unwrap();
    assert_eq!((log.records(), store.read(gathering).unwrap()), (10, version(100)), "read while gathered");
    let alone = UndoStore::new(shared(&log));
    let one = alone.append(&version(7)).unwrap();
    assert_eq!((log.records(), alone.read(one).unwrap()), (11, version(7)));
}

/// Verifies: [DESIGN] RULES.md D31: the log is truncated before the oldest
/// batch a pinned version is in, the batches before it forgotten, a
/// version of one read no more (SQLCODE -916); a store with nothing pinned
/// lets the whole log go.
#[test]
fn batches_go_once_nothing_in_them_is_pinned() {
    let log = Arc::new(Hooked::default());
    let store = UndoStore::batched(shared(&log), 400);
    let at: Vec<Lsn> = (0_u32..50_u32).map(|n| store.append(&version(n)).unwrap()).collect();
    for done in at.iter().take(25) {
        store.release(*done);
    }
    store.truncate().unwrap();
    assert_eq!(store.read(at[5]).err().map(|e| e.code), Some(Code::Corrupt), "its batch forgotten");
    assert_eq!(store.read(at[25]).unwrap(), version(25), "the oldest pinned");
    assert_eq!(store.pinned(), 25);
    at.iter().skip(25).for_each(|done| store.release(*done));
    store.truncate().unwrap();
    store.truncate().unwrap();
    assert_eq!((store.pinned(), store.read(at[49]).err().map(|e| e.code)), (0, Some(Code::Corrupt)));
}

/// Verifies: [DESIGN] RULES.md D31: a batch the log cannot write fails the
/// append that filled it, whose version is not kept for it; the batch's
/// other versions are read still, from memory, and the store goes on.
#[test]
fn a_batch_that_cannot_be_written_stays_readable() {
    let log = Arc::new(Hooked::default());
    let store = UndoStore::batched(shared(&log), 400);
    let earlier: Vec<Lsn> = (0_u32..9_u32).map(|n| store.append(&version(n)).unwrap()).collect();
    log.failing.store(true, Ordering::Relaxed);
    assert_eq!(store.append(&version(9)).err().map(|e| e.code), Some(Code::Io));
    assert_eq!(store.pinned(), 9, "the failed one's not kept");
    log.failing.store(false, Ordering::Relaxed);
    for (n, pos) in (0_u32..9_u32).zip(&earlier) {
        assert_eq!(store.read(*pos).unwrap(), version(n), "{n}");
    }
    let next = store.append(&version(10)).unwrap();
    assert_eq!(store.read(next).unwrap(), version(10));
}

/// Verifies: [DESIGN] RULES.md D31: a version appended to a store of no
/// batches while it is truncated, with nothing pinned, is kept: the log's
/// end the truncation goes to is read where no append can fall between it
/// and the pins.
///
/// Regression test (R-TEST-7): the end was read once the pins' lock was
/// let go, so a version appended and pinned in between was before it, and
/// removed; a rollback then could not find it (SQLCODE -916), as threads
/// moving money between accounts found under load.
#[test]
fn a_version_appended_while_the_store_is_truncated_is_kept() {
    let log = Arc::new(Hooked::default());
    let store = Arc::new(UndoStore::new(shared(&log)));
    let first = store.append(&version(0)).unwrap();
    store.release(first);
    let appended = Arc::new(Mutex::new(None));
    let (meddler, slot) = (Arc::clone(&store), Arc::clone(&appended));
    *lock_builtin(&log.on_end) = Some(Box::new(move || {
        // Where the race would fall: an append, if the pins are not held.
        let free = meddler.pinned.try_lock().is_ok();
        if free {
            *lock_builtin(&slot) = Some(meddler.append(&version(1)).unwrap());
        }
    }));
    store.truncate().unwrap();
    let taken = lock_builtin(&appended).take();
    let at = taken.unwrap_or_else(|| store.append(&version(1)).unwrap());
    assert_eq!(store.read(at).unwrap(), version(1), "pinned, so kept");
    assert_eq!(store.read(first).err().map(|e| e.code), Some(Code::Corrupt), "released, so gone");
}

/// Verifies: [DESIGN] RULES.md D31: a batch whose record is in the log,
/// not yet recorded as written, is kept by a truncation then, as a
/// version pinned in it may be read.
///
/// Regression test (R-TEST-7): the truncation went to the log's end, past
/// the record of a batch being written, and removed it.
#[test]
fn a_batch_being_written_while_the_store_is_truncated_is_kept() {
    let log = Arc::new(Hooked::default());
    let store = Arc::new(UndoStore::batched(shared(&log), 1));
    let first = store.append(&version(0)).unwrap();
    let meddler = Arc::clone(&store);
    *lock_builtin(&log.on_append) = Some(Box::new(move || {
        meddler.release(first);
        meddler.truncate().unwrap();
    }));
    let second = store.append(&version(1)).unwrap();
    assert_eq!(log.records(), 1, "the first batch removed, the second kept");
    assert_eq!(store.read(second).unwrap(), version(1));
    assert_eq!(store.read(first).err().map(|e| e.code), Some(Code::Corrupt));
}

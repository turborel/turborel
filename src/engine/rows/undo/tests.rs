//! Unit tests of the undo store's batches: versions read wherever their
//! batch is, the log truncated by batch, and a batch that cannot be
//! written; a truncation racing an append, alone or in batches; batches fitted to a page; and
//! batches written kept in memory within the budget's share, past it read
//! from the log, read on many threads; on the log of `hooked.rs`.

use super::hooked::{Hooked, shared};
use super::*;

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
        assert_eq!(store.read(*pos).unwrap().to_vec(), version(n), "{n}");
    }
    let gathering = store.append(&version(100)).unwrap();
    assert_eq!((log.records(), store.read(gathering).unwrap().to_vec()), (10, version(100)), "read while gathered");
    let alone = UndoStore::new(shared(&log));
    let one = alone.append(&version(7)).unwrap();
    assert_eq!((log.records(), alone.read(one).unwrap().to_vec()), (11, version(7)));
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
    assert_eq!(store.read(at[25]).unwrap().to_vec(), version(25), "the oldest pinned");
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
        assert_eq!(store.read(*pos).unwrap().to_vec(), version(n), "{n}");
    }
    let next = store.append(&version(10)).unwrap();
    assert_eq!(store.read(next).unwrap().to_vec(), version(10));
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
    assert_eq!(store.read(at).unwrap().to_vec(), version(1), "pinned, so kept");
    assert_eq!(store.read(first).err().map(|e| e.code), Some(Code::Corrupt), "released, so gone");
}

/// Verifies: [DESIGN] RULES.md D31: in a batched store, a batch appended
/// and written while the store is truncated, with nothing pinned, is
/// kept: the log's end the truncation goes to is read under the batches'
/// lock, where no batch can be taken and written between it and the pins.
///
/// Regression test (R-TEST-7): the end was read once that lock was let
/// go, so a batch written in between was before it, and removed; a reader
/// of a version pinned in it could not find it (SQLCODE -916), as readers
/// on many threads found on CI.
#[test]
fn a_batch_appended_while_the_batched_store_is_truncated_is_kept() {
    let log = Arc::new(Hooked::default());
    let store = Arc::new(UndoStore::batched(shared(&log), 1));
    let first = store.append(&version(0)).unwrap();
    store.release(first);
    let meddler = Arc::clone(&store);
    let (tell, told) = std::sync::mpsc::channel();
    let appender = Arc::new(Mutex::new(None));
    let slot = Arc::clone(&appender);
    *lock_builtin(&log.on_end) = Some(Box::new(move || {
        // Where the race would fall: an append on another thread, done
        // at once if the batches' lock is free, else once it is.
        let writer = std::thread::spawn(move || {
            let at = meddler.append(&version(1)).unwrap();
            let _ = tell.send(());
            at
        });
        let _ = told.recv_timeout(std::time::Duration::from_millis(300));
        *lock_builtin(&slot) = Some(writer);
    }));
    store.truncate().unwrap();
    let writer = lock_builtin(&appender).take().unwrap();
    let at = writer.join().unwrap();
    assert_eq!(store.read(at).unwrap().to_vec(), version(1), "pinned, so kept");
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
    assert_eq!(store.read(second).unwrap().to_vec(), version(1));
    assert_eq!(store.read(first).err().map(|e| e.code), Some(Code::Corrupt));
}

/// Verifies: [DESIGN] RULES.md D31, D49: a fitted store writes a batch
/// before the next version would carry its record past its bytes, so
/// every record fits them: ten versions of forty bytes, each with its
/// length, and the count make a record of 444 bytes, so a fit of 444
/// takes ten to a record and one of 443 nine; a version longer than the
/// fit is a batch alone. Each is read back as appended.
#[test]
fn fitted_batches_stay_within_their_bytes() {
    let log = Arc::new(Hooked::default());
    let store = UndoStore::fitted(shared(&log), 444);
    let at: Vec<Lsn> = (0_u32..21_u32).map(|n| store.append(&version(n)).unwrap()).collect();
    assert_eq!(log.records(), 2, "ten to a record, the 21st gathered");
    assert_eq!(log.sizes(), [444_usize, 444_usize]);
    for (n, pos) in (0_u32..21_u32).zip(&at) {
        assert_eq!(store.read(*pos).unwrap().to_vec(), version(n), "{n}");
    }
    let tighter = Arc::new(Hooked::default());
    let nine = UndoStore::fitted(shared(&tighter), 443);
    for n in 0_u32..10_u32 {
        nine.append(&version(n)).unwrap();
    }
    assert_eq!(tighter.records(), 1, "nine to a record, the tenth gathered");
    let lengthy = nine.append(&[7_u8; 1000]).unwrap();
    let after = nine.append(&version(11)).unwrap();
    assert_eq!(tighter.records(), 3, "the tenth's batch, then the long one alone");
    assert_eq!((nine.read(lengthy).unwrap().to_vec(), nine.read(after).unwrap().to_vec()), (vec![7_u8; 1000], version(11)));
}

/// Verifies: [DESIGN] RULES.md D31: the last `CACHED` batches read from
/// the log are kept, so versions of each are read again from memory, each
/// as it was appended, its bytes where its batch's record holds them; a
/// batch read past them forgets the one read from the log longest ago, read from the
/// log again.
#[test]
fn batches_read_are_kept_the_last_few() {
    let log = Arc::new(Hooked::default());
    let store = UndoStore::batched(shared(&log), 400);
    let at: Vec<Lsn> = (0_u32..120_u32).map(|n| store.append(&version(n)).unwrap()).collect();
    let batches = u32::try_from(CACHED).unwrap() + 1;
    let first_of = |batch: u32| batch * 10;
    for batch in 0..batches - 1 {
        assert_eq!(&*store.read(at[usize::try_from(first_of(batch)).unwrap()]).unwrap(), version(first_of(batch)));
    }
    assert_eq!(log.reads.load(Ordering::Relaxed), CACHED, "one read of each batch");
    for (n, pos) in (0_u32..).zip(&at).take(usize::try_from((batches - 1) * 10).unwrap()) {
        assert_eq!(&*store.read(*pos).unwrap(), version(n), "{n}, from memory");
    }
    assert_eq!(log.reads.load(Ordering::Relaxed), CACHED, "no read again");
    let past = usize::try_from(first_of(batches - 1)).unwrap();
    assert_eq!(&*store.read(at[past]).unwrap(), version(first_of(batches - 1)));
    assert_eq!(&*store.read(at[1]).unwrap(), version(1), "the first batch read again");
    assert_eq!(&*store.read(at[21]).unwrap(), version(21), "the third, still kept");
    assert_eq!(log.reads.load(Ordering::Relaxed), CACHED + 2);
}

/// The bytes a batch of ten versions of forty bytes holds kept: its record
/// of 444 bytes and ten spans.
const KEPT_BATCH: usize = 444 + 10 * 16;

/// A store of batches of 400 bytes over `log`, kept within a budget of
/// `limit` bytes, and the budget.
fn kept_in(log: &Arc<Hooked>, limit: usize) -> (UndoStore, Arc<MemoryBudget>) {
    let budget = MemoryBudget::new(limit);
    (UndoStore::batched(shared(log), 400).in_budget(&budget).unwrap(), budget)
}

/// Verifies: [DESIGN] RULES.md D746: a store in a budget keeps each batch it
/// writes in memory, so every version is read back without a read of the
/// log, each as appended; what it keeps is reserved of the budget, and given
/// back as a truncation forgets the batches.
#[test]
fn batches_written_are_kept_until_truncated() {
    let log = Arc::new(Hooked::default());
    let (store, budget) = kept_in(&log, 1 << 30);
    let at: Vec<Lsn> = (0_u32..100_u32).map(|n| store.append(&version(n)).unwrap()).collect();
    for (n, pos) in (0_u32..100_u32).zip(&at) {
        assert_eq!(&*store.read(*pos).unwrap(), version(n), "{n}");
    }
    assert_eq!((log.records(), log.reads.load(Ordering::Relaxed)), (10, 0), "every batch read from memory");
    assert_eq!((store.kept_bytes(), budget.reserved()), (10 * KEPT_BATCH, 10 * KEPT_BATCH));
    at.iter().take(55).for_each(|done| store.release(*done));
    store.truncate().unwrap();
    assert_eq!((store.kept_bytes(), budget.reserved()), (5 * KEPT_BATCH, 5 * KEPT_BATCH), "five batches forgotten");
    assert_eq!(store.read(at[3]).err().map(|e| e.code), Some(Code::Corrupt));
    assert_eq!(&*store.read(at[55]).unwrap(), version(55));
    at.iter().skip(55).for_each(|done| store.release(*done));
    store.truncate().unwrap();
    assert_eq!((store.kept_bytes(), budget.reserved()), (0, 0));
}

/// Verifies: [DESIGN] RULES.md D746, the bound on batches kept: past the
/// budget's share the oldest go first, and a version of one gone is read
/// from the log, as appended; the newest are read from memory still. MC/DC
/// of the bound's decision, its first condition: the share passed, the
/// budget not refusing.
#[test]
fn batches_past_the_bound_are_read_from_the_log() {
    let log = Arc::new(Hooked::default());
    let (store, budget) = kept_in(&log, 16 * 3 * KEPT_BATCH);
    let at: Vec<Lsn> = (0_u32..100_u32).map(|n| store.append(&version(n)).unwrap()).collect();
    assert_eq!((store.kept_bytes(), budget.reserved()), (3 * KEPT_BATCH, 3 * KEPT_BATCH), "the newest three");
    for (n, pos) in (0_u32..100_u32).zip(&at).skip(70) {
        assert_eq!(&*store.read(*pos).unwrap(), version(n), "{n}");
    }
    assert_eq!(log.reads.load(Ordering::Relaxed), 0, "the newest from memory");
    for (n, pos) in (0_u32..70_u32).zip(&at) {
        assert_eq!(&*store.read(*pos).unwrap(), version(n), "{n}");
    }
    assert_eq!(log.reads.load(Ordering::Relaxed), 7, "each batch gone read once");
}

/// Verifies: [DESIGN] RULES.md D746, D38: a budget that refuses what the
/// batches kept would reserve keeps none, within the share all the same,
/// and every version is read from the log. MC/DC of the bound's decision,
/// its second condition: the share not passed, the budget refusing.
#[test]
fn a_budget_refusing_keeps_no_batch() {
    let log = Arc::new(Hooked::default());
    let (store, budget) = kept_in(&log, 1 << 20);
    let _taken = budget.reserve((1 << 20) - 300, "another cache").unwrap();
    let at: Vec<Lsn> = (0_u32..30_u32).map(|n| store.append(&version(n)).unwrap()).collect();
    assert_eq!(store.kept_bytes(), 0);
    for (n, pos) in (0_u32..30_u32).zip(&at) {
        assert_eq!(&*store.read(*pos).unwrap(), version(n), "{n}");
    }
    assert_eq!(log.reads.load(Ordering::Relaxed), 3);
}

/// Verifies: [DESIGN] RULES.md D745: readers on many threads read versions
/// from memory while appends fill batches and truncations forget them, and
/// each reads every version it holds pinned as it was appended.
#[test]
fn readers_read_while_batches_are_written_and_forgotten() {
    let log = Arc::new(Hooked::default());
    let (store, _budget) = kept_in(&log, 16 * 4 * KEPT_BATCH);
    let store = Arc::new(store);
    let readers: Vec<_> = (0_u32..4_u32)
        .map(|t| {
            let store = Arc::clone(&store);
            std::thread::spawn(move || {
                for round in 0_u32..50_u32 {
                    let n = t * 1000 + round;
                    let at: Vec<Lsn> = (0..10).map(|k| store.append(&version(n * 10 + k)).unwrap()).collect();
                    for (k, pos) in (0_u32..).zip(&at) {
                        assert_eq!(&*store.read(*pos).unwrap(), version(n * 10 + k));
                    }
                    for done in &at {
                        store.release(*done);
                    }
                    store.truncate().unwrap();
                }
            })
        })
        .collect();
    for r in readers {
        r.join().unwrap();
    }
    assert_eq!(store.pinned(), 0);
}

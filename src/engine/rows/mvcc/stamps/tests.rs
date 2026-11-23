use std::sync::Arc;

use turborel_test_support::Scratch;

use crate::disk::buffer::{BufferPool, Clock};
use crate::disk::encoding::Pipeline;
use crate::disk::io::OsFileSystem;
use crate::disk::pager::Pager;
use crate::engine::rows::{RowTable, TableFiles, TableOptions, key};
use crate::hash::crc::{CRC32_ISCSI, Crc};
use crate::log::MemoryLog;
use crate::storage::{RowId, TableStorage};
use crate::txn::NO_TRANSACTION;
use crate::value::{DataType, Value};

use super::super::{Maker, Version};

/// A table of one `CHAR(5)` column, in files of its own, holding one row
/// committed before every snapshot.
pub(in crate::engine::rows::mvcc) fn table(scratch: &Scratch) -> (TableFiles, RowTable, RowId) {
    let pager =
        Pager::create(&OsFileSystem, &scratch.path("rows"), Arc::new(Crc::new(CRC32_ISCSI).unwrap()), Pipeline::none(), 4096).unwrap();
    let pool = Arc::new(BufferPool::new(16, Box::new(Clock::new(16))).unwrap());
    let file = pool.add_pager(Arc::new(pager));
    let files = TableFiles::new(pool, file, Arc::new(MemoryLog::new()));
    let table = RowTable::create(&files, &[DataType::character(5)], TableOptions::default()).unwrap();
    let id = table.insert(NO_TRANSACTION, vec![Value::Char("old  ".into())]).unwrap();
    (files, table, id)
}

fn value(table: &RowTable, tx: u64, id: RowId) -> Option<String> {
    table.get(tx, id).unwrap().map(|row| row[0].to_string())
}

fn maker(table: &RowTable, id: RowId) -> Maker {
    Version::parse(&table.tree.get(&key(id)).unwrap().unwrap()).unwrap().maker
}

/// Verifies: [DESIGN] RULES.md D29, D440: a commit is seen by snapshots at
/// or after its timestamp at once, through the status table, its version
/// still naming the transaction; settling stamps it; a rolled-back
/// transaction's version is never seen.
#[test]
fn a_commit_is_seen_before_its_stamp() {
    let scratch = Scratch::new("unit-stamps");
    let (_files, table, id) = table(&scratch);
    table.begin(10, 9).unwrap();
    table.update(10, id, vec![Value::Char("new  ".into())]).unwrap();
    table.commit(10, 10);
    assert_eq!(maker(&table, id), Maker::Open(10), "not stamped yet");
    table.begin(11, 10).unwrap();
    table.begin(12, 9).unwrap();
    assert_eq!(value(&table, 11, id).as_deref(), Some("new  "));
    assert_eq!(value(&table, 12, id).as_deref(), Some("old  "), "its snapshot is before the commit");
    table.settle(10);
    assert_eq!(maker(&table, id), Maker::Committed(10));
    table.update(12, id, vec![Value::Char("lost ".into())]).unwrap_err();
    table.rollback(12).unwrap();
    table.begin(13, 10).unwrap();
    table.update(13, id, vec![Value::Char("undo ".into())]).unwrap();
    table.rollback(13).unwrap();
    assert_eq!(value(&table, 11, id).as_deref(), Some("new  "));
}

/// Verifies: [DESIGN] RULES.md D440: a writer replacing a version committed
/// but not stamped appends it stamped, so a reader going back to it reads
/// it as committed once its transaction's status is forgotten.
#[test]
fn an_unstamped_version_is_appended_stamped() {
    let scratch = Scratch::new("unit-stamps-appended");
    let (files, table, id) = table(&scratch);
    table.begin(10, 9).unwrap();
    table.update(10, id, vec![Value::Char("first".into())]).unwrap();
    table.commit(10, 10);
    table.begin(11, 10).unwrap();
    table.update(11, id, vec![Value::Char("again".into())]).unwrap();
    table.commit(11, 12);
    files.statuses.abort(10);
    table.begin(12, 11).unwrap();
    assert_eq!(value(&table, 12, id).as_deref(), Some("first"));
}

/// Verifies: [DESIGN] RULES.md D843: a commit of many rows, their leaves
/// split by the transaction's own later inserts, stamps each in place from
/// where its write left it, an update, an insert and a deletion alike.
#[test]
fn a_commit_stamps_its_rows_where_their_writes_left_them() {
    let scratch = Scratch::new("unit-stamps-placed");
    let (_files, table, id) = table(&scratch);
    table.begin(10, 9).unwrap();
    table.update(10, id, vec![Value::Char("new  ".into())]).unwrap();
    let ids: Vec<RowId> = (0..600_u32).map(|_| table.insert(10, vec![Value::Char("many ".into())]).unwrap()).collect();
    table.delete(10, ids[3]).unwrap();
    table.commit(10, 10);
    table.settle(10);
    for &row in ids.iter().chain([&id]) {
        assert_eq!(maker(&table, row), Maker::Committed(10), "row {row}");
    }
    table.begin(11, 10).unwrap();
    assert_eq!(value(&table, 11, id).as_deref(), Some("new  "));
    assert_eq!(value(&table, 11, ids[3]), None, "the deletion committed");
    assert_eq!(value(&table, 11, ids[599]).as_deref(), Some("many "));
}

/// Whether `work`, run on a thread of its own, returns within five
/// seconds: not if it waits on a lock the test holds.
fn returns(work: impl FnOnce() + Send + 'static) -> bool {
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        work();
        done.send(()).unwrap();
    });
    finished.recv_timeout(std::time::Duration::from_secs(5)).is_ok()
}

/// Verifies: [DESIGN] RULES.md D870: a commit whose rows to stamp and
/// garbage are within the bytes held makes its lists with no work areas,
/// so it commits and settles though another holds the store's work areas.
#[test]
fn a_commit_within_the_bytes_held_takes_no_work_areas() {
    let scratch = Scratch::new("unit-stamps-no-areas");
    let (files, table, id) = table(&scratch);
    let table = Arc::new(table);
    table.begin(10, 9).unwrap();
    table.update(10, id, vec![Value::Char("new  ".into())]).unwrap();
    let extra = table.insert(10, vec![Value::Char("more ".into())]).unwrap();
    table.delete(10, extra).unwrap();
    let held = files.work.write().unwrap();
    let committing = Arc::clone(&table);
    let returned = returns(move || {
        committing.commit(10, 10);
        committing.settle(10);
    });
    drop(held);
    assert!(returned, "the commit waited on the work areas");
    assert_eq!(maker(&table, id), Maker::Committed(10));
    assert!(table.awaits_collection(), "its garbage is filed");
}

/// Verifies: [DESIGN] RULES.md D448, D870: a transaction that changed
/// nothing in a table and recorded no read there validates, commits and
/// settles though another holds the table's validated transactions and its
/// queue of commits to stamp, and is not kept among the validated. MC/DC
/// of keeping it: (no change, no read) not kept, against (change, no read)
/// and (no change, read), each kept.
#[test]
fn a_transaction_that_left_nothing_takes_no_lock_of_the_table() {
    let scratch = Scratch::new("unit-stamps-nothing-left");
    let (_files, table, id) = table(&scratch);
    let table = Arc::new(table);
    let unchecked = crate::txn::ReadMode { snapshot: 9, checked: false };
    table.begin(10, 9).unwrap();
    table.read_mode(10, unchecked).unwrap();
    assert_eq!(value(&table, 10, id).as_deref(), Some("old  "));
    let (validated, unstamped) = (table.versions.validated.lock().unwrap(), table.versions.unstamped.lock().unwrap());
    let ending = Arc::clone(&table);
    let returned = returns(move || {
        ending.validate(10, 10).unwrap();
        ending.commit(10, 10);
        ending.settle(10);
    });
    drop((validated, unstamped));
    assert!(returned, "its end waited on a lock of the table");
    assert!(table.versions.validated.lock().unwrap().is_empty(), "nothing read or changed is not kept");
    // A change with no read is kept.
    table.begin(11, 10).unwrap();
    table.read_mode(11, unchecked).unwrap();
    table.update(11, id, vec![Value::Char("new  ".into())]).unwrap();
    table.validate(11, 11).unwrap();
    assert!(!table.versions.validated.lock().unwrap().is_empty(), "a change is kept");
    table.commit(11, 11);
    assert_eq!(table.versions.queued.load(std::sync::atomic::Ordering::SeqCst), 1, "its commit is queued to stamp");
    table.settle(11);
    assert_eq!(table.versions.queued.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(maker(&table, id), Maker::Committed(11));
    table.gc(12, &mut |_, _, _| true).unwrap();
    assert!(table.versions.validated.lock().unwrap().is_empty(), "collected past the horizon");
    // A read with no change is kept.
    table.begin(12, 11).unwrap();
    assert_eq!(value(&table, 12, id).as_deref(), Some("new  "));
    table.validate(12, 12).unwrap();
    assert!(!table.versions.validated.lock().unwrap().is_empty(), "a read recorded is kept");
}

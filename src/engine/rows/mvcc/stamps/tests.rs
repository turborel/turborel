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
fn table(scratch: &Scratch) -> (TableFiles, RowTable, RowId) {
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

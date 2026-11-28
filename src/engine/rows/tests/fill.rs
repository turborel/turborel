//! Unit tests of an index filled from a table's versions beside its
//! writers, read outside any transaction (RULES.md R-TEST-1, R-TEST-7).

use std::sync::Arc;

use turborel_test_support::Scratch;

use crate::disk::buffer::{BufferPool, Clock};
use crate::disk::encoding::Pipeline;
use crate::disk::io::OsFileSystem;
use crate::disk::pager::Pager;
use crate::engine::rows::{NO_TRANSACTION, RowTable, TableFiles, TableOptions};
use crate::hash::crc::{CRC32_ISCSI, Crc};
use crate::log::MemoryLog;
use crate::storage::TableStorage;
use crate::txn::Timestamp;
use crate::value::{DataType, Value};

/// Verifies: [DESIGN] RULES.md D1322, D443, D27, an index filled beside
/// writers: the versions of a table read with a horizon below the one the
/// table was last collected at are those a reader may still read, the
/// newest alone once every earlier one is collected, and no version is
/// asked of the undo store that collection released.
///
/// Regression (RULES.md R-TEST-7): the fill read the clock's horizon as it
/// began, which is 0 while another thread is beginning a transaction
/// (`txn/clock.rs`, `Clock::begin`), after the build had collected the
/// table at a later one. It took every committed version for one a reader
/// may read, went back past them to a version collection had released,
/// and CREATE INDEX failed (SQLCODE -916; seen as a rare failure of
/// `kept_entries::an_index_built_beside_kept_updates_is_exact`).
#[test]
fn a_fill_below_the_collected_horizon_reads_no_version_released() {
    let scratch = Scratch::new("unit-fill");
    let pager =
        Pager::create(&OsFileSystem, &scratch.path("rows"), Arc::new(Crc::new(CRC32_ISCSI).unwrap()), Pipeline::none(), 4096).unwrap();
    let pool = Arc::new(BufferPool::new(16, Box::new(Clock::new(16))).unwrap());
    let file = pool.add_pager(Arc::new(pager));
    let files = TableFiles::new(pool, file, Arc::new(MemoryLog::new()));
    let table = RowTable::create(&files, &[DataType::character(5)], TableOptions::default()).unwrap();
    let row = |v: &str| vec![Value::Char(v.into())];
    let id = table.insert(NO_TRANSACTION, row("v0   ")).unwrap();
    for (tx, at) in [(10, 5), (11, 6)] {
        table.begin(tx, at - 1).unwrap();
        table.update(tx, id, row(&format!("v{at}   "))).unwrap();
        table.commit(tx, at);
    }
    table.gc(6, &mut |_, _, _| true).unwrap();
    assert_eq!(files.undo().pinned(), 0, "every version before the newest collected");
    let read: Vec<String> = Arc::new(table)
        .versions(0, Timestamp::MAX, &[0])
        .map(|found| found.map(|(_, values)| values[0].to_string()))
        .collect::<crate::error::Result<_>>()
        .unwrap();
    assert_eq!(read, ["v6   "]);
}

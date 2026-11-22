//! Unit tests of reads by row id: a read that finds no row tells the
//! commit of the deletion it found, and only of one committed (RULES.md
//! R-TEST-1, R-TEST-2).

use turborel_test_support::Scratch;

use super::super::*;
use crate::disk::buffer::Clock;
use crate::disk::encoding::Pipeline;
use crate::disk::io::OsFileSystem;
use crate::disk::pager::Pager;
use crate::hash::crc::{CRC32_ISCSI, Crc};
use crate::log::MemoryLog;
use crate::value::{DataType, Value};

/// Verifies: [DESIGN] RULES.md D837: a read that finds no row because its
/// newest version is a deletion committed at 2 tells 2 in its `Near`; a
/// read that finds the row, one before the deletion's commit, one while
/// the deletion is open and one of an id never given tell none.
#[test]
fn a_read_of_a_deleted_row_tells_its_deletion() {
    let scratch = Scratch::new("unit-gone");
    let pager =
        Pager::create(&OsFileSystem, &scratch.path("rows"), Arc::new(Crc::new(CRC32_ISCSI).unwrap()), Pipeline::none(), 4096).unwrap();
    let pool = Arc::new(BufferPool::new(16, Box::new(Clock::new(16))).unwrap());
    let file = pool.add_pager(Arc::new(pager));
    let files = TableFiles::new(pool, file, Arc::new(MemoryLog::new()));
    let table = RowTable::create(&files, &[DataType::character(5)], TableOptions::default()).unwrap();
    let row = || vec![Value::Char("x    ".into())];
    let (a, b) = (table.insert(NO_TRANSACTION, row()).unwrap(), table.insert(NO_TRANSACTION, row()).unwrap());
    let read = |tx: TxId, id: RowId| {
        let mut near = Near::default();
        let found = table.get_near(tx, id, None, &mut near).unwrap();
        (found.is_some(), near.gone)
    };
    table.begin(10, 1).unwrap();
    table.delete(10, a).unwrap();
    table.begin(11, 1).unwrap();
    assert_eq!(read(11, a), (true, None), "a deletion open");
    assert_eq!(read(10, a), (false, None), "its own deletion, not committed");
    table.validate(10, 2).unwrap();
    table.commit(10, 2);
    assert_eq!(read(11, a), (true, None), "a snapshot before the deletion's commit");
    table.begin(12, 2).unwrap();
    assert_eq!(read(12, a), (false, Some(2)), "a deletion committed at 2");
    assert_eq!(read(12, b), (true, None), "a row found");
    assert_eq!(read(12, 999), (false, None), "no row of the id");
}

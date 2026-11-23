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

/// Verifies: [DESIGN] RULES.md D862: a read of the columns listed gives
/// their values alone, in the order listed, appended to the row the
/// caller holds, for the version the reader sees; one read after another
/// through the same `Near` and row gives each row's; and a row the reader
/// does not see leaves the row as it was.
#[test]
fn a_read_of_listed_columns_gives_them_in_their_order() {
    let scratch = Scratch::new("unit-listed");
    let pager =
        Pager::create(&OsFileSystem, &scratch.path("rows"), Arc::new(Crc::new(CRC32_ISCSI).unwrap()), Pipeline::none(), 4096).unwrap();
    let pool = Arc::new(BufferPool::new(16, Box::new(Clock::new(16))).unwrap());
    let file = pool.add_pager(Arc::new(pager));
    let files = TableFiles::new(pool, file, Arc::new(MemoryLog::new()));
    let types = [DataType::character(3), DataType::varying(8), DataType::character(2)];
    let table = RowTable::create(&files, &types, TableOptions::default()).unwrap();
    let row = |n: u8| vec![Value::Char(format!("a{n} ").into()), Value::Char(format!("b{n}").into()), Value::Char(format!("c{n}").into())];
    let (a, b) = (table.insert(NO_TRANSACTION, row(1)).unwrap(), table.insert(NO_TRANSACTION, row(2)).unwrap());
    table.begin(10, 1).unwrap();
    table.update(10, a, row(3)).unwrap();
    table.begin(11, 1).unwrap();
    let (mut near, mut out) = (Near::default(), Vec::new());
    let mut read = |tx: TxId, id: RowId, columns: &[usize]| {
        out.clear();
        let found = table.get_listed(tx, id, columns, (&mut near, &mut out)).unwrap();
        (found, out.iter().map(ToString::to_string).collect::<Vec<_>>().join("|"))
    };
    assert_eq!(read(10, a, &[2, 0]), (true, "c3|a3 ".to_owned()), "its own change");
    assert_eq!(read(11, a, &[2, 0]), (true, "c1|a1 ".to_owned()), "the version before it");
    assert_eq!(read(11, b, &[1]), (true, "b2".to_owned()));
    assert_eq!(read(11, b, &[]), (true, String::new()), "no column");
    assert_eq!(read(11, 999, &[1]), (false, String::new()), "no row of the id");
    // Pinned for a statement (RULES.md D857), the table reads the same.
    let pinned = TableStorage::pin(&table);
    assert!(pinned.is_some());
    assert_eq!(read(11, b, &[0]), (true, "a2 ".to_owned()));
}

/// Verifies: [DESIGN] RULES.md D869: a row read where it lies and one
/// read from a copy give the same values, whole, by columns and listed:
/// a row whose long value is kept out of line is read from the copy when
/// that value is asked for, and where it lies when it is not.
#[test]
fn a_row_read_where_it_lies_is_the_row_read_from_a_copy() {
    let scratch = Scratch::new("unit-standing");
    let pager =
        Pager::create(&OsFileSystem, &scratch.path("rows"), Arc::new(Crc::new(CRC32_ISCSI).unwrap()), Pipeline::none(), 4096).unwrap();
    let pool = Arc::new(BufferPool::new(64, Box::new(Clock::new(64))).unwrap());
    let file = pool.add_pager(Arc::new(pager));
    let files = TableFiles::new(pool, file, Arc::new(MemoryLog::new()));
    let table = RowTable::create(&files, &[DataType::character(3), DataType::varying(9000)], TableOptions::default()).unwrap();
    let long = "x".repeat(6000);
    let short = table.insert(NO_TRANSACTION, vec![Value::Char("a  ".into()), Value::Char("b".into())]).unwrap();
    let spilled = table.insert(NO_TRANSACTION, vec![Value::Char("c  ".into()), Value::Char(long.clone().into())]).unwrap();
    let shown = |row: Option<Row>| row.map(|r| r.iter().map(ToString::to_string).collect::<Vec<_>>().join("|"));
    assert_eq!(shown(table.get(NO_TRANSACTION, short).unwrap()), Some("a  |b".to_owned()));
    assert_eq!(shown(table.get(NO_TRANSACTION, spilled).unwrap()), Some(format!("c  |{long}")), "from a copy");
    assert_eq!(shown(table.get_columns(NO_TRANSACTION, spilled, &[0]).unwrap()).map(|s| s.starts_with("c  |")), Some(true));
    let (mut near, mut out) = (Near::default(), Vec::new());
    assert!(table.get_listed(NO_TRANSACTION, spilled, &[1, 0], (&mut near, &mut out)).unwrap());
    assert_eq!(out.iter().map(ToString::to_string).collect::<Vec<_>>(), [long.clone(), "c  ".to_owned()]);
    out.clear();
    assert!(table.get_listed(NO_TRANSACTION, spilled, &[0], (&mut near, &mut out)).unwrap());
    assert_eq!(out.len(), 1, "where it lies: the long value not asked for");
}

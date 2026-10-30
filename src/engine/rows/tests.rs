//! Unit tests of the row store: a long value's page checked against its
//! row and column, a meta page against its format, and an undone change's
//! older version kept for open readers (RULES.md R-TEST-1, R-TEST-2,
//! R-TEST-7). Split from rows.rs (RULES.md R-SIZE-2).

use turborel_test_support::Scratch;

use super::meta::parse_meta;
use super::mvcc::Reader;
use super::*;
use crate::disk::buffer::Clock;
use crate::disk::encoding::Pipeline;
use crate::disk::io::OsFileSystem;
use crate::disk::pager::Pager;
use crate::engine::record::Reference;
use crate::hash::crc::{CRC32_ISCSI, Crc};
use crate::log::MemoryLog;

/// Verifies: [DESIGN] RULES.md D31, values out of line: a reference
/// read for another row or another column than its page holds is
/// refused with SQLCODE -916, never answered with another row's value;
/// a meta page of another length or format is not one.
#[test]
fn stale_references_are_refused() {
    let scratch = Scratch::new("unit-rows");
    let pager =
        Pager::create(&OsFileSystem, &scratch.path("rows"), Arc::new(Crc::new(CRC32_ISCSI).unwrap()), Pipeline::none(), 4096).unwrap();
    let pool = Arc::new(BufferPool::new(16, Box::new(Clock::new(16))).unwrap());
    let file = pool.add_pager(Arc::new(pager));
    let files = TableFiles::new(pool, file, Arc::new(MemoryLog::new()));
    let table =
        RowTable::create(&files, &[DataType::Character { length: 9 }], TableOptions { out_of_line: Some(2), ..TableOptions::default() })
            .unwrap();
    let page = table.put_value(7, 0, b"long").unwrap();
    let reference = Reference { column: 0, at: page.0, length: 4 };
    assert_eq!(table.get_value(7, reference).unwrap(), b"long");
    assert_eq!(table.get_value(8, reference).err().map(|e| e.code), Some(Code::Corrupt), "another row");
    let other_column = Reference { column: 1, ..reference };
    assert_eq!(table.get_value(7, other_column).err().map(|e| e.code), Some(Code::Corrupt), "another column");
    assert_eq!(parse_meta(&meta_content(PageId(3), 9, 11)), Some((PageId(3), 9, 11)));
    assert_eq!(parse_meta(&[TABLE_FORMAT; 3]), None);
    let mut other_format = meta_content(PageId(3), 9, 11);
    other_format[0] = 3;
    assert_eq!(parse_meta(&other_format), None);
}

/// Verifies: [DESIGN] RULES.md D29, D31, garbage collection: what an
/// undone change appended to the undo store stays readable by a
/// transaction open when it was undone, and is released once that
/// transaction has ended.
///
/// Regression (RULES.md R-TEST-7): a transaction's change, undone, put
/// the old entry back in the tree and released at once the committed
/// version it had appended to the undo store, so a reader that had read
/// the undone version, and was going back from it, found it gone
/// (SQLCODE -916; seen as a rare failure of
/// `mvcc::transfers_on_threads_keep_the_total`). Now the version is
/// kept until every transaction open then has ended.
#[test]
fn an_undone_change_keeps_its_older_version_for_open_readers() {
    let scratch = Scratch::new("unit-undone");
    let pager =
        Pager::create(&OsFileSystem, &scratch.path("rows"), Arc::new(Crc::new(CRC32_ISCSI).unwrap()), Pipeline::none(), 4096).unwrap();
    let pool = Arc::new(BufferPool::new(16, Box::new(Clock::new(16))).unwrap());
    let file = pool.add_pager(Arc::new(pager));
    let files = TableFiles::new(pool, file, Arc::new(MemoryLog::new()));
    let table = RowTable::create(&files, &[DataType::Character { length: 5 }], TableOptions::default()).unwrap();
    let id = table.insert(crate::txn::NO_TRANSACTION, vec![crate::value::Value::Char("old  ".into())]).unwrap();
    let (writer, reader) = (10, 11);
    table.begin(writer, 9).unwrap();
    table.begin(reader, 9).unwrap();
    table.update(writer, id, vec![crate::value::Value::Char("new  ".into())]).unwrap();
    let seen = table.tree.get(&key(id)).unwrap().unwrap();
    table.rollback(writer).unwrap();
    table.gc(9, &mut |_, _| {}).unwrap();
    let found = table.visible_from(seen, Reader::At { tx: reader, snapshot: 9 }).unwrap().unwrap();
    assert_eq!(table.decode(id, &found, None).unwrap()[0].to_string(), "old  ");
    table.commit(reader, 9);
    table.gc(12, &mut |_, _| {}).unwrap();
    assert_eq!(files.undo().pinned(), 0, "released once the reader ended");
}

/// Verifies: [DESIGN] RULES.md D29, D31, writes to deleted rows: a
/// transaction cannot update or delete a row it deleted itself, nor one a
/// committed transaction it sees deleted (SQLCODE -910, no such row); one
/// whose snapshot is before that deletion is refused as a conflict
/// (SQLCODE -911), the row changed after it began.
#[test]
fn deleted_rows_cannot_be_written() {
    let scratch = Scratch::new("unit-deleted");
    let pager =
        Pager::create(&OsFileSystem, &scratch.path("rows"), Arc::new(Crc::new(CRC32_ISCSI).unwrap()), Pipeline::none(), 4096).unwrap();
    let pool = Arc::new(BufferPool::new(16, Box::new(Clock::new(16))).unwrap());
    let file = pool.add_pager(Arc::new(pager));
    let files = TableFiles::new(pool, file, Arc::new(MemoryLog::new()));
    let table = RowTable::create(&files, &[DataType::Character { length: 5 }], TableOptions::default()).unwrap();
    let row = || vec![crate::value::Value::Char("x    ".into())];
    let (a, b) = (table.insert(crate::txn::NO_TRANSACTION, row()).unwrap(), table.insert(crate::txn::NO_TRANSACTION, row()).unwrap());
    let code = |r: Result<()>| r.err().map(|e| e.code);
    table.begin(10, 1).unwrap();
    table.delete(10, a).unwrap();
    assert_eq!(code(table.update(10, a, row())), Some(Code::Api), "its own deletion");
    assert_eq!(code(table.delete(10, a)), Some(Code::Api));
    table.begin(11, 1).unwrap();
    table.delete(11, b).unwrap();
    table.validate(11, 2).unwrap();
    table.commit(11, 2);
    table.begin(12, 2).unwrap();
    assert_eq!(code(table.update(12, b, row())), Some(Code::Api), "a deletion it sees");
    assert_eq!(code(table.update(10, b, row())), Some(Code::Conflict), "a deletion after its snapshot");
}

/// Verifies: [DESIGN] `TableStorage::versions`, what an index build counts:
/// of the versions committed by `as_of`, each one a reader at or after
/// `horizon` may read, and each one collection will take back that none
/// can read; not a version committed after `as_of`, nor one not yet
/// committed, whose writers count them (`Database::create_index`).
#[test]
fn versions_are_those_committed_by_a_time() {
    let scratch = Scratch::new("unit-versions");
    let pager =
        Pager::create(&OsFileSystem, &scratch.path("rows"), Arc::new(Crc::new(CRC32_ISCSI).unwrap()), Pipeline::none(), 4096).unwrap();
    let pool = Arc::new(BufferPool::new(16, Box::new(Clock::new(16))).unwrap());
    let file = pool.add_pager(Arc::new(pager));
    let files = TableFiles::new(pool, file, Arc::new(MemoryLog::new()));
    let table = Arc::new(RowTable::create(&files, &[DataType::Character { length: 1 }], TableOptions::default()).unwrap());
    let value = |v: &str| vec![crate::value::Value::Char(v.into())];
    let id = table.insert(crate::txn::NO_TRANSACTION, value("a")).unwrap();
    table.begin(10, 9).unwrap();
    table.update(10, id, value("b")).unwrap();
    table.commit(10, 12);
    table.begin(20, 12).unwrap();
    table.update(20, id, value("c")).unwrap();
    let versions = |horizon, as_of| {
        let mut found: Vec<String> = Arc::clone(&table).versions(horizon, as_of, &[0]).map(|r| r.unwrap().1[0].to_string()).collect();
        found.sort();
        found
    };
    assert_eq!(versions(9, 11), ["a"], "b is committed after 11, c not at all");
    assert_eq!(versions(9, 12), ["a", "b"]);
    assert_eq!(versions(13, 12), ["a", "b"], "a, no longer read, is collection's to take back");
    table.gc(13, &mut |_, _| {}).unwrap();
    assert_eq!(versions(13, 12), ["b"], "taken back");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.18 GR2], RULES.md D33: a
/// dropped table frees every page it took, its rows', the values they keep
/// out of line, its older versions' and its tree's, when the last
/// reference to it goes, not when it is discarded; a table not discarded
/// keeps them; a new table reuses them.
#[test]
fn a_discarded_table_frees_its_pages_when_it_goes() {
    let fs = crate::disk::memory::MemoryFileSystem::new();
    let pager = Arc::new(
        Pager::create(&fs, std::path::Path::new("/rows"), Arc::new(Crc::new(CRC32_ISCSI).unwrap()), Pipeline::none(), 4096).unwrap(),
    );
    let pool = Arc::new(BufferPool::new(64, Box::new(Clock::new(64))).unwrap());
    let file = pool.add_pager(Arc::clone(&pager));
    let files = TableFiles::new(pool, file, Arc::new(MemoryLog::new()));
    let types = [DataType::Character { length: 900 }];
    let long = |n: u64| vec![crate::value::Value::Char(format!("{n:>900}"))];
    let written = |table: &RowTable| {
        let ids: Vec<RowId> = (0..300).map(|n| table.insert(crate::txn::NO_TRANSACTION, long(n)).unwrap()).collect();
        table.begin(10, 1).unwrap();
        for &id in ids.iter().take(50) {
            table.update(10, id, long(id + 1000)).unwrap();
        }
        table.validate(10, 2).unwrap();
        table.commit(10, 2);
    };
    let in_use = || pager.page_count() - pager.free_pages();
    let before = in_use();
    let kept = RowTable::create(&files, &types, TableOptions::default()).unwrap();
    written(&kept);
    let with_kept = in_use();
    drop(kept);
    assert_eq!(in_use(), with_kept, "not discarded: kept");
    let dropped = Arc::new(RowTable::create(&files, &types, TableOptions::default()).unwrap());
    written(&dropped);
    let (size, with_both) = (pager.page_count(), in_use());
    assert!(with_both > with_kept, "it took pages");
    let reader = Arc::clone(&dropped);
    dropped.discard();
    drop(dropped);
    assert_eq!(in_use(), with_both, "a reader still holds it");
    drop(reader);
    assert_eq!(in_use(), with_kept, "every page it took freed");
    let again = RowTable::create(&files, &types, TableOptions::default()).unwrap();
    written(&again);
    assert_eq!(pager.page_count(), size, "its pages reused, the file not grown");
    assert!(with_kept > before);
}

/// Verifies: [DESIGN] RULES.md D25, D29, `TableStorage::garbage`: the
/// versions awaiting collection are read as they are pulled, only those
/// superseded by commits at or before the timestamp asked for, the
/// superseded rows as they were.
#[test]
fn garbage_is_read_as_of_a_commit() {
    let scratch = Scratch::new("unit-garbage");
    let pager =
        Pager::create(&OsFileSystem, &scratch.path("rows"), Arc::new(Crc::new(CRC32_ISCSI).unwrap()), Pipeline::none(), 4096).unwrap();
    let pool = Arc::new(BufferPool::new(16, Box::new(Clock::new(16))).unwrap());
    let file = pool.add_pager(Arc::new(pager));
    let files = TableFiles::new(pool, file, Arc::new(MemoryLog::new()));
    let table = Arc::new(RowTable::create(&files, &[DataType::Character { length: 5 }], TableOptions::default()).unwrap());
    let char = |s: &str| vec![crate::value::Value::Char(s.into())];
    let id = table.insert(crate::txn::NO_TRANSACTION, char("first")).unwrap();
    for (tx, at, value) in [(10, 20, "secnd"), (11, 30, "third")] {
        table.begin(tx, at - 5).unwrap();
        table.update(tx, id, char(value)).unwrap();
        table.validate(tx, at).unwrap();
        table.commit(tx, at);
    }
    let read = |as_of| Arc::clone(&table).garbage(as_of).map(|r| r.unwrap().1[0].to_string()).collect::<Vec<_>>();
    assert_eq!(read(10), Vec::<String>::new());
    assert_eq!(read(25), ["first"]);
    assert_eq!(read(30), ["first", "secnd"]);
}

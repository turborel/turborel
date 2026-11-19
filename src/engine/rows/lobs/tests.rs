//! Unit tests of a table's large objects (RULES.md D79, R-TEST-1): kept
//! in chunks and read back; shared by the versions of one row, copied for
//! another; what a change drops freed once collected, what a rollback
//! undoes freed at once, what a failed attempt made freed at commit; the
//! pages of a dropped table's objects freed with it; and recovery's
//! objects kept as the log has them, the unnamed swept.

use std::sync::Arc;

use crate::disk::buffer::{BufferPool, Clock};
use crate::disk::encoding::Pipeline;
use crate::disk::pager::Pager;
use crate::engine::rows::{RowTable, TableFiles, TableOptions};
use crate::hash::crc::{CRC32_ISCSI, Crc};
use crate::log::MemoryLog;
use crate::storage::{Row, TableStorage};
use crate::txn::NO_TRANSACTION;
use crate::value::lob::{Lob, LobKind};
use crate::value::{DataType, Decimal, Value};

/// A table of an INTEGER and a CLOB, in a pager in memory, and the pager.
fn table() -> (RowTable, Arc<Pager>) {
    let fs = crate::disk::memory::MemoryFileSystem::new();
    let pager = Arc::new(
        Pager::create(&fs, std::path::Path::new("/rows"), Arc::new(Crc::new(CRC32_ISCSI).unwrap()), Pipeline::none(), 4096).unwrap(),
    );
    let pool = Arc::new(BufferPool::new(64, Box::new(Clock::new(64))).unwrap());
    let file = pool.add_pager(Arc::clone(&pager));
    let files = TableFiles::new(pool, file, Arc::new(MemoryLog::new()));
    let types = [
        DataType::Exact { precision: 10, scale: 0, spelled: crate::value::Spelled::Integer },
        DataType::Lob { kind: LobKind::Character, length: crate::value::lob::MAX_LOB_LENGTH, units: crate::value::CharUnits::Characters },
    ];
    (RowTable::create(&files, &types, TableOptions::default()).unwrap(), pager)
}

/// Row `k` holding `text`.
fn row(k: i128, text: &str) -> Row {
    vec![Value::Exact(Decimal::new(k, 0)), Value::Lob(Lob::of_text(text))]
}

/// A text of `n` characters, chunks' ends falling within some.
fn long(n: usize, seed: char) -> String {
    (0..n).map(|i| if i % 7 == 0 { '€' } else { seed }).collect()
}

/// Keeps `row`'s large objects for `tx`, the objects logged counted.
fn kept(table: &RowTable, tx: u64, replacing: Option<u64>, mut row: Row, logged: &mut usize) -> Row {
    let mut log = |_: u64, _: &[u8], put: &mut dyn FnMut() -> crate::error::Result<()>| {
        *logged = logged.saturating_add(1);
        put()
    };
    TableStorage::keep_lobs(table, tx, replacing, &mut row, &mut log).unwrap();
    row
}

/// The objects the table keeps.
fn objects(table: &RowTable) -> usize {
    table
        .lobs
        .tree
        .get()
        .map_or(0, |t| t.scan(std::ops::Bound::Unbounded, std::ops::Bound::Unbounded, crate::engine::tree::Direction::Forward).count())
}

/// The text of row `id`'s CLOB as `tx` sees it.
fn text(table: &RowTable, tx: u64, id: u64) -> String {
    let row = table.get(tx, id).unwrap().unwrap();
    let Value::Lob(l) = &row[1] else { panic!("{row:?}") };
    String::from_utf8(l.reader().read(0, l.octets()).unwrap()).unwrap()
}

/// Verifies: [DESIGN] RULES.md D79: a long value is kept as a directory
/// and a chunk's segments, each logged once, read back whole; a short
/// one is held in the record and makes no object; a version writing back
/// the value it replaces shares its objects, and one taking another row's
/// copies it; a change dropping a value frees its objects once the commit
/// is collected, not before, while a reader of the older version may read
/// them.
#[test]
fn versions_of_a_row_share_its_objects_and_free_what_they_drop() {
    let (table, _) = table();
    let first = long(600_000, 'a');
    let mut logged = 0;
    table.begin(10, 1).unwrap();
    let r = kept(&table, 10, None, row(1, &first), &mut logged);
    let id = table.insert(10, r).unwrap();
    let short = kept(&table, 10, None, row(2, "short"), &mut logged);
    let other = table.insert(10, short).unwrap();
    table.validate(10, 2).unwrap();
    table.commit(10, 2);
    let segments = first.len().div_ceil(crate::value::lob::CHUNK_OCTETS);
    assert_eq!((logged, objects(&table)), (segments + 1, segments + 1), "its segments and directory, the short one none");
    assert_eq!(text(&table, NO_TRANSACTION, id), first);
    table.begin(11, 2).unwrap();
    let mut same = table.get(11, id).unwrap().unwrap();
    same[0] = Value::Exact(Decimal::new(5, 0));
    let same = kept(&table, 11, Some(id), same, &mut logged);
    table.update(11, id, same).unwrap();
    assert_eq!(logged, segments + 1, "written back, shared");
    let copied = table.get(11, id).unwrap().unwrap();
    let copied = kept(&table, 11, Some(other), copied, &mut logged);
    table.update(11, other, copied).unwrap();
    assert_eq!(objects(&table), 2 * (segments + 1), "another row's value copied");
    table.validate(11, 3).unwrap();
    table.commit(11, 3);
    table.begin(20, 3).unwrap();
    table.begin(12, 3).unwrap();
    let replaced = kept(&table, 12, Some(id), row(1, "gone"), &mut logged);
    table.update(12, id, replaced).unwrap();
    table.validate(12, 4).unwrap();
    table.commit(12, 4);
    table.gc(3, &mut |_, _| true).unwrap();
    assert_eq!(objects(&table), 2 * (segments + 1), "a reader at 3 may read them");
    assert_eq!(text(&table, 20, id), first, "the older version read whole");
    table.commit(20, 3);
    table.gc(4, &mut |_, _| true).unwrap();
    assert_eq!(objects(&table), segments + 1, "freed once collected");
    assert_eq!(text(&table, NO_TRANSACTION, other), first);
}

/// Verifies: [DESIGN] RULES.md D79: a rollback frees the objects its
/// transaction made, of rows it wrote or of none; a commit frees those it made for no
/// version it wrote, as for an attempt that failed; a version naming an
/// object of another row is refused (SQLCODE -910).
#[test]
fn undone_and_unwritten_objects_are_freed() {
    let (table, _) = table();
    let mut logged = 0;
    table.begin(10, 1).unwrap();
    let r = kept(&table, 10, None, row(1, &long(300_000, 'b')), &mut logged);
    table.insert(10, r).unwrap();
    let _never_written = kept(&table, 10, None, row(2, &long(300_000, 'g')), &mut logged);
    assert!(objects(&table) > 0);
    table.rollback(10).unwrap();
    assert_eq!(objects(&table), 0, "freed by the rollback, those of no row too");
    table.begin(11, 1).unwrap();
    let kept_row = kept(&table, 11, None, row(1, &long(300_000, 'c')), &mut logged);
    let _unwritten = kept(&table, 11, None, row(2, &long(300_000, 'd')), &mut logged);
    let id = table.insert(11, kept_row.clone()).unwrap();
    let refused = table.insert(11, kept_row).err().map(|e| e.code);
    assert_eq!(refused, Some(crate::error::Code::Api), "another row naming its objects");
    table.validate(11, 2).unwrap();
    table.commit(11, 2);
    assert_eq!(objects(&table), 300_000_usize.div_ceil(crate::value::lob::CHUNK_OCTETS) + 1, "the unwritten value's freed");
    assert_eq!(text(&table, NO_TRANSACTION, id), long(300_000, 'c'));
}

/// Verifies: [DESIGN] RULES.md D25, D79: recovery keeps each object the
/// log holds once, however often it is read; and its sweep frees those no
/// row's newest version names, of a transaction that never committed or a
/// version replaced, keeping those named.
#[test]
fn recovery_keeps_logged_objects_and_sweeps_the_unnamed() {
    let (table, _) = table();
    let mut log = Vec::new();
    table.begin(10, 1).unwrap();
    let mut keep = |id: u64, body: &[u8], put: &mut dyn FnMut() -> crate::error::Result<()>| {
        log.push((id, body.to_vec()));
        put()
    };
    let mut named = row(1, &long(300_000, 'e'));
    TableStorage::keep_lobs(&table, 10, None, &mut named, &mut keep).unwrap();
    let mut unnamed = row(2, &long(300_000, 'f'));
    TableStorage::keep_lobs(&table, 10, None, &mut unnamed, &mut keep).unwrap();
    table.rollback(10).unwrap();
    assert_eq!(objects(&table), 0);
    for (id, body) in log.iter().chain(&log) {
        TableStorage::apply_lob(&table, *id, body).unwrap();
    }
    assert_eq!(objects(&table), log.len(), "each kept once");
    let reference = |r: &Row| {
        let Value::Lob(l) = &r[1] else { panic!() };
        let id = table.lobs.kept_here(l).unwrap();
        vec![r[0].clone(), Value::Lob(Lob::of_reference(LobKind::Character, id, (l.octets(), l.chars())))]
    };
    TableStorage::apply(&table, 7, Some(reference(&named))).unwrap();
    TableStorage::recovered(&table).unwrap();
    assert_eq!(objects(&table), log.len().div_euclid(2), "the unnamed value's swept");
    assert_eq!(text(&table, NO_TRANSACTION, 7), long(300_000, 'e'));
}

/// Verifies: [DESIGN] RULES.md D79: a dropped table's objects are freed
/// when the last value reading them goes, not before.
#[test]
fn a_dropped_tables_objects_go_with_the_last_reader() {
    let (table, pager) = table();
    let in_use = || pager.page_count() - pager.free_pages();
    let before = in_use();
    let mut logged = 0;
    let r = kept(&table, NO_TRANSACTION, None, row(1, &long(600_000, 'g')), &mut logged);
    let id = table.insert(NO_TRANSACTION, r).unwrap();
    let held = table.get(NO_TRANSACTION, id).unwrap().unwrap();
    assert!(in_use() > before);
    table.discard();
    let tree_pages = in_use();
    drop(table);
    let Value::Lob(l) = &held[1] else { panic!() };
    assert_eq!(l.reader().read(0, 3).unwrap(), b"\xE2\x82\xAC", "still read");
    assert!(in_use() <= tree_pages);
    drop(held);
    assert!(in_use() < tree_pages, "its objects' pages freed");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §4.2.3] [DESIGN] RULES.md D79,
/// D106: an XML value held as its sequence is kept as its stored form: a
/// long one as the table's objects, each chunk logged once, read back the
/// same value; a short one in its record, making no object.
#[test]
fn xml_values_are_kept_as_large_objects() {
    use crate::value::xml::{XmlType, XmlValue};
    use crate::xml::{Sequence, Syntax, Whitespace, parse};
    let fs = crate::disk::memory::MemoryFileSystem::new();
    let pager = Arc::new(
        Pager::create(&fs, std::path::Path::new("/rows"), Arc::new(Crc::new(CRC32_ISCSI).unwrap()), Pipeline::none(), 4096).unwrap(),
    );
    let pool = Arc::new(BufferPool::new(64, Box::new(Clock::new(64))).unwrap());
    let file = pool.add_pager(Arc::clone(&pager));
    let files = TableFiles::new(pool, file, Arc::new(MemoryLog::new()));
    let table = RowTable::create(&files, &[DataType::Xml(XmlType::Sequence)], TableOptions::default()).unwrap();
    let document = |text: &str| Value::Xml(XmlValue::of(Sequence::document(parse(text, Syntax::Document, Whitespace::Preserve).unwrap())));
    let long_text = format!("<a>{}</a>", long(400_000, 'x'));
    let mut logged = 0;
    table.begin(10, 1).unwrap();
    let long_row = kept(&table, 10, None, vec![document(&long_text)], &mut logged);
    let Value::Xml(x) = &long_row[0] else { panic!("{long_row:?}") };
    assert!(x.stored_lob().is_some_and(|l| l.inline().is_none()), "kept as objects, not in the record");
    assert!(logged > 1, "a directory and its segments, each logged");
    let id = table.insert(10, long_row).unwrap();
    let before = (logged, objects(&table));
    let short_row = kept(&table, 10, None, vec![document("<b/>")], &mut logged);
    table.insert(10, short_row).unwrap();
    assert_eq!((logged, objects(&table)), before, "a short one is held in its record");
    table.validate(10, 2).unwrap();
    table.commit(10, 2);
    let read = table.get(NO_TRANSACTION, id).unwrap().unwrap();
    let Value::Xml(back) = &read[0] else { panic!("{read:?}") };
    assert_eq!(back.text().unwrap(), long_text);
}

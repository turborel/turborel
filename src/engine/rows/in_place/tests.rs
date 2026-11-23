//! Unit tests of a row table's batches read in place (`in_place.rs`), and
//! the fixtures the row count's tests share (`count.rs`).

use std::sync::Arc;

use crate::disk::buffer::{BufferPool, Clock};
use crate::disk::encoding::Pipeline;
use crate::disk::memory::MemoryFileSystem;
use crate::disk::pager::Pager;
use crate::engine::rows::{RowTable, TableFiles, TableOptions, key};
use crate::error::Code;
use crate::hash::crc::{CRC32_ISCSI, Crc};
use crate::log::MemoryLog;
use crate::storage::{Ramp, Row, RowId, TableStorage};
use crate::txn::{NO_TRANSACTION, TxId};
use crate::value::{DataType, Decimal, Spelled, Value};

/// A number, and a text that goes out of line when long.
pub(in crate::engine::rows) const TYPES: [DataType; 2] =
    [DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer }, DataType::varying(400)];

/// Row `n`: its text long, out of line, every fifth row.
pub(in crate::engine::rows) fn row(n: i128) -> Row {
    let text = if n % 5 == 0 { format!("long {n} ").repeat(30) } else { format!("text {n}") };
    vec![Value::Exact(Decimal::new(n, 0)), Value::Char(text.into())]
}

/// The files of tables of the row store in memory.
pub(in crate::engine::rows) fn files() -> TableFiles {
    let fs = MemoryFileSystem::new();
    let crc = Arc::new(Crc::new(CRC32_ISCSI).unwrap());
    let pager = Arc::new(Pager::create(&fs, std::path::Path::new("/rows"), crc, Pipeline::none(), 4096).unwrap());
    let pool = Arc::new(BufferPool::new(512, Box::new(Clock::new(512))).unwrap());
    let file = pool.add_pager(pager);
    TableFiles::new(pool, file, Arc::new(MemoryLog::new()))
}

/// A table of the row store in `files`, its values longer than 64 bytes
/// out of line.
pub(in crate::engine::rows) fn table_in(files: &TableFiles) -> Arc<RowTable> {
    let options = TableOptions { out_of_line: Some(64), ..TableOptions::default() };
    Arc::new(RowTable::create(files, &TYPES, options).unwrap())
}

/// Rows `0..n` of `table`, committed: their ids.
pub(in crate::engine::rows) fn filled(table: &RowTable, n: i128) -> Vec<RowId> {
    (0..n).map(|i| table.insert(NO_TRANSACTION, row(i)).unwrap()).collect()
}

/// The rows of `table` `tx` sees, of `columns`, as a scan of copies gives
/// them, each as text.
pub(in crate::engine::rows) fn scanned(table: &Arc<RowTable>, tx: TxId, columns: &[usize]) -> Vec<Vec<String>> {
    let rows = Arc::clone(table).scan_rows(tx, columns, None).map(|r| r.unwrap().1);
    rows.map(|row| columns.iter().map(|&c| row[c].to_string()).collect()).collect()
}

/// The same, read in place by batch.
fn in_place(table: &Arc<RowTable>, tx: TxId, columns: &[usize]) -> Vec<Vec<String>> {
    let reader = table.reader(tx).unwrap();
    let mut read = Vec::new();
    for batch in Arc::clone(table).gathered_in((reader, Ramp::default()), columns, vec![(0, None)]) {
        let batch = batch.unwrap();
        read.extend((0..batch.rows).map(|r| (0..columns.len()).map(|c| batch.cell(c, r).to_value().to_string()).collect()));
    }
    read
}

/// Verifies: [DESIGN] RULES.md D350, D29: batches read in place hold the
/// rows each reader sees, as a scan of copies gives them, of any columns
/// or none: a transaction's own inserts, updates and deletions; the rows
/// another's uncommitted changes and a commit after its snapshot leave it,
/// from their older versions; outside a transaction the newest committed;
/// values out of line read alike, in batches past a leaf.
#[test]
fn batches_read_in_place_are_the_rows_each_reader_sees() {
    let files = files();
    let table = table_in(&files);
    let ids = filled(&table, 3_000);
    let (own, other, late, before, after) = (10, 11, 12, 13, 14);
    for tx in [own, other, late, before] {
        table.begin(tx, 1).unwrap();
    }
    for &id in &ids[5..50] {
        table.delete(own, id).unwrap();
    }
    for &id in &ids[100..150] {
        table.update(own, id, row(-1)).unwrap();
    }
    for n in 0..20 {
        table.insert(own, row(5_000 + n)).unwrap();
    }
    for &id in &ids[200..210] {
        table.update(late, id, row(-2)).unwrap();
    }
    table.validate(late, 2).unwrap();
    table.commit(late, 2);
    table.begin(after, 3).unwrap();
    for tx in [own, other, before, after, NO_TRANSACTION] {
        for columns in [&[][..], &[0], &[1], &[1, 0]] {
            assert_eq!(in_place(&table, tx, columns), scanned(&table, tx, columns), "reader {tx}, columns {columns:?}");
        }
    }
    assert_eq!(in_place(&table, own, &[]).len(), 3_000 - 45 + 20);
}

/// Verifies: [DESIGN] RULES.md D350, robustness (R-TEST-2): a row whose
/// newest version is malformed fails the scan with SQLCODE -916.
#[test]
fn a_malformed_version_fails_a_scan_in_place() {
    let files = files();
    let table = table_in(&files);
    let ids = filled(&table, 40);
    table.tree.replace(&key(ids[30]), &[0xff]).unwrap();
    let reader = table.reader(NO_TRANSACTION).unwrap();
    let failed = Arc::clone(&table).gathered_in((reader, Ramp::default()), &[0], vec![(0, None)]).find_map(Result::err);
    assert_eq!(failed.map(|e| e.code), Some(Code::Corrupt));
}

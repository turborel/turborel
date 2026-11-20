//! Unit tests of a row table's rows counted from its zones (`count.rs`).

use std::sync::Arc;

use crate::engine::rows::in_place::tests::{TYPES, files, filled, row, scanned, table_in};
use crate::engine::rows::options::Columns;
use crate::engine::rows::{RowTable, TableOptions};
use crate::error::{Code, Result};
use crate::storage::TableStorage;
use crate::txn::{NO_TRANSACTION, TxId};

/// The count of the rows `tx` sees of `table`, its counts summed.
fn counted(table: &Arc<RowTable>, tx: TxId) -> u64 {
    Arc::clone(table).count(tx).unwrap().sum::<Result<u64>>().unwrap()
}

/// Whether `table`'s count is the scan's for each of `readers`.
fn counts_as_scanned(table: &Arc<RowTable>, readers: &[TxId]) {
    for &tx in readers {
        let scanned = u64::try_from(scanned(table, tx, &[]).len()).unwrap();
        assert_eq!(counted(table, tx), scanned, "reader {tx}");
    }
}

/// Has `table` sum up and settle its zones, of rows a reader at or after
/// `horizon` may read, until none is due.
fn settled(table: &RowTable, horizon: u64) {
    while table.maintain(horizon).unwrap() {}
}

/// Verifies: [DESIGN] RULES.md D351, D29: the count is the scan's for
/// every reader, of zones settled and not: committed rows, from settled
/// zones once summed up; a transaction's own uncommitted inserts and
/// deletions, its zones no longer settled; another's uncommitted rows,
/// counted by no other reader; a commit after an open reader's snapshot,
/// its zone left unsettled for as long as the reader is open, and
/// settled once it ends; deletions summed up.
#[test]
fn a_count_is_the_scans_for_every_reader() {
    let files = files();
    let table = table_in(&files);
    let ids = filled(&table, 5_000);
    counts_as_scanned(&table, &[NO_TRANSACTION]);
    settled(&table, 1);
    assert_eq!(table.zones.settled().0, 5_000, "every zone settled");
    counts_as_scanned(&table, &[NO_TRANSACTION]);
    let (own, other, old) = (10, 11, 12);
    for tx in [own, other, old] {
        table.begin(tx, 1).unwrap();
    }
    for &id in &ids[10..60] {
        table.delete(own, id).unwrap();
    }
    for n in 0..30 {
        table.insert(own, row(9_000 + n)).unwrap();
    }
    table.insert(other, row(-1)).unwrap();
    counts_as_scanned(&table, &[own, other, old, NO_TRANSACTION]);
    assert_eq!(counted(&table, own), 5_000 - 50 + 30);
    table.rollback(own).unwrap();
    table.validate(other, 2).unwrap();
    table.commit(other, 2);
    settled(&table, 1);
    counts_as_scanned(&table, &[old, NO_TRANSACTION]);
    assert_eq!(counted(&table, old), 5_000, "a commit after its snapshot is not counted");
    table.rollback(old).unwrap();
    settled(&table, 3);
    assert_eq!(table.zones.settled().0, 5_001, "settled once no reader is older");
    table.begin(13, 3).unwrap();
    for &id in &ids[100..400] {
        table.delete(13, id).unwrap();
    }
    table.validate(13, 4).unwrap();
    table.commit(13, 4);
    settled(&table, 5);
    counts_as_scanned(&table, &[NO_TRANSACTION]);
    assert_eq!(counted(&table, NO_TRANSACTION), 4_701);
}

/// Verifies: [DESIGN] RULES.md D351, D31: a table opened has every zone
/// unsettled, so its rows are counted by reading them, then from its
/// zones once settled; a table that freezes rows gives no count.
#[test]
fn a_count_of_a_table_opened_reads_until_settled() {
    let files = files();
    let meta = {
        let table = table_in(&files);
        filled(&table, 3_000);
        table.meta()
    };
    let options = TableOptions { out_of_line: Some(64), ..TableOptions::default() };
    let table = Arc::new(RowTable::open(&files, meta, &TYPES, options).unwrap());
    assert_eq!(table.zones.settled().0, 0, "every zone unsettled");
    assert_eq!(counted(&table, NO_TRANSACTION), 3_000);
    settled(&table, 1);
    assert_eq!((table.zones.settled().0, counted(&table, NO_TRANSACTION)), (3_000, 3_000));
    let columns = TableOptions { columns: Some(Columns::default()), ..TableOptions::default() };
    let frozen = Arc::new(RowTable::create(&files, &TYPES, columns).unwrap());
    assert!(frozen.count(NO_TRANSACTION).is_none(), "a table that freezes rows is scanned");
}

/// Verifies: [DESIGN] RULES.md D351, D29, serializable reads: a count read
/// counts as reading the whole table, so a transaction that counted and
/// writes is refused (SQLCODE -911) once another committed an insert
/// meanwhile; a count never read notes no read, and refuses nothing.
#[test]
fn a_count_read_conflicts_with_an_insert_as_a_scan_does() {
    let files = files();
    let table = table_in(&files);
    let ids = filled(&table, 100);
    settled(&table, 1);
    let (reader, unread, writer) = (10, 11, 12);
    for tx in [reader, unread, writer] {
        table.begin(tx, 1).unwrap();
    }
    assert_eq!(counted(&table, reader), 100);
    drop(Arc::clone(&table).count(unread));
    table.insert(writer, row(-1)).unwrap();
    table.validate(writer, 2).unwrap();
    table.commit(writer, 2);
    table.update(reader, ids[0], row(-2)).unwrap();
    table.update(unread, ids[1], row(-3)).unwrap();
    assert_eq!(table.validate(reader, 3).err().map(|e| e.code), Some(Code::Conflict));
    table.validate(unread, 4).unwrap();
}

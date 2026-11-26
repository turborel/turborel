//! Unit tests of a table that holds groups counted from its directory
//! (`frozen/counts.rs`, RULES.md D1180).

use super::*;
use crate::error::Result;
use crate::txn::TxId;

/// The count of the rows `tx` sees of `table`, its counts summed.
fn counted(table: &Arc<RowTable>, tx: TxId) -> u64 {
    Arc::clone(table).count(tx).unwrap().sum::<Result<u64>>().unwrap()
}

/// The rows a scan by batch of no column gives `tx` of `table`.
fn batched(table: &Arc<RowTable>, tx: TxId) -> u64 {
    let rows: usize = Arc::clone(table).scan_batches(tx, &[], None, &[]).map(|b| b.unwrap().rows).sum();
    u64::try_from(rows).unwrap()
}

/// Whether `table`'s count is the scan's for each of `readers`, the shadow
/// bits trusted and not.
fn counts_as_scanned(table: &Arc<RowTable>, readers: &[TxId]) {
    for trusted in [true, false] {
        table.trust_shadow_bits(trusted);
        for &tx in readers {
            assert_eq!(counted(table, tx), batched(table, tx), "reader {tx}, bits trusted {trusted}");
        }
    }
    table.trust_shadow_bits(true);
}

/// Verifies: [DESIGN] RULES.md D1180, D29: a table that holds groups is
/// counted as its scan reads it, for every reader: its frozen rows; a
/// transaction's own updates and deletions of frozen rows, and its
/// inserts; another's uncommitted ones, counted by no other reader; a
/// commit after an open reader's snapshot, not counted by it; groups
/// sorted by a column, whose row ids are not in order; and the rows of
/// the tree past the groups.
#[test]
fn a_grouped_count_is_the_scans_for_every_reader() {
    for sorted in [false, true] {
        let (_, files) = files();
        let mut options = options(0);
        if sorted {
            options.columns.as_mut().unwrap().order = Places::of(&[1]);
        }
        let table = Arc::new(RowTable::create(&files, &TYPES, options).unwrap());
        let ids: Vec<RowId> = (0..5000).map(|n| table.insert(NO_TRANSACTION, row(n)).unwrap()).collect();
        ended(&table, 1);
        assert_eq!(table.frozen.directory().unwrap().groups.len(), 2);
        counts_as_scanned(&table, &[NO_TRANSACTION]);
        assert_eq!(counted(&table, NO_TRANSACTION), 5000);
        let (own, other, old) = (10, 11, 12);
        for tx in [own, other, old] {
            table.begin(tx, 1).unwrap();
        }
        for &id in &ids[10..60] {
            table.delete(own, id).unwrap();
        }
        for &id in &ids[100..120] {
            table.update(own, id, row(-1)).unwrap();
        }
        for n in 0..30 {
            table.insert(own, row(9_000 + n)).unwrap();
        }
        table.delete(other, ids[4500]).unwrap();
        table.insert(other, row(-2)).unwrap();
        counts_as_scanned(&table, &[own, other, old, NO_TRANSACTION]);
        assert_eq!(counted(&table, own), 5000 - 50 + 30);
        table.rollback(own).unwrap();
        table.validate(other, 2).unwrap();
        table.commit(other, 2);
        counts_as_scanned(&table, &[old, NO_TRANSACTION]);
        assert_eq!((counted(&table, old), counted(&table, NO_TRANSACTION)), (5000, 5000), "a commit after its snapshot");
        table.rollback(old).unwrap();
    }
}

/// Verifies: [DESIGN] RULES.md D1180, D31: a count begun before a freeze
/// counts each row once, of the directory it took, the frozen rows'
/// entries kept for it; and a table that no longer freezes rows but
/// still holds groups is counted from them too.
#[test]
fn a_grouped_count_misses_no_row_frozen_as_it_counts() {
    let (_, files) = files();
    let (table, _) = table(&files, 0, 5000);
    let mut counts = Arc::clone(&table).count(NO_TRANSACTION).unwrap();
    let first = counts.next().unwrap().unwrap();
    ended(&table, 1);
    assert_eq!(table.frozen.directory().unwrap().groups.len(), 2, "frozen as it counts");
    assert_eq!(first.checked_add(counts.sum::<Result<u64>>().unwrap()), Some(5000));
    table.retire_unread().unwrap();
    assert_eq!(counted(&table, NO_TRANSACTION), 5000);
    table.frozen.reconfigure(None);
    assert_eq!(counted(&table, NO_TRANSACTION), 5000, "groups held, no longer freezing");
}

/// Verifies: [DESIGN] RULES.md D1180, D29, serializable reads: a count of
/// a table that holds groups counts as reading the whole table, so a
/// transaction that counted and writes is refused (SQLCODE -911) once
/// another committed an insert meanwhile; a count never read notes no
/// read.
#[test]
fn a_grouped_count_conflicts_with_an_insert_as_a_scan_does() {
    let (_, files) = files();
    let (table, ids) = table(&files, 0, 5000);
    ended(&table, 1);
    let (reader, unread, writer) = (10, 11, 12);
    for tx in [reader, unread, writer] {
        table.begin(tx, 1).unwrap();
    }
    assert_eq!(counted(&table, reader), 5000);
    drop(Arc::clone(&table).count(unread));
    table.insert(writer, row(-1)).unwrap();
    table.validate(writer, 2).unwrap();
    table.commit(writer, 2);
    table.update(reader, ids[0], row(-2)).unwrap();
    table.update(unread, ids[1], row(-3)).unwrap();
    assert_eq!(table.validate(reader, 3).err().map(|e| e.code), Some(Code::Conflict));
    table.validate(unread, 4).unwrap();
}

/// Of `table`'s pieces for `tx` of column 0: the rows given whole, with
/// the bounds of each piece given whole, and the rows given by batch.
fn pieces(table: &Arc<RowTable>, tx: TxId) -> (u64, Vec<String>, usize) {
    let (mut whole, mut bounds, mut batched) = (0_u64, Vec::new(), 0_usize);
    for piece in Arc::clone(table).summarized(tx, &[0]).unwrap() {
        match piece.unwrap() {
            crate::storage::Piece::Whole { rows, columns } => {
                whole = whole.checked_add(rows).unwrap();
                let (nulls, b) = &columns[0];
                bounds.push(format!("{nulls} {}", b.as_ref().map_or("none".to_owned(), |(l, g)| format!("{l}..{g}"))));
            }
            crate::storage::Piece::Batch(batch) => batched = batched.checked_add(batch.rows).unwrap(),
        }
    }
    (whole, bounds, batched)
}

/// Verifies: [DESIGN] RULES.md D1181: a group the tree has no entry over
/// is given whole, its rows and its column's nulls and bounds; one a
/// deletion shadows is given by batch, as the scan reads it, and so are
/// the rows past the groups, so every row is given once; a table of the
/// row store summarizes nothing.
#[test]
fn a_group_no_entry_shadows_is_given_whole() {
    let (_, files) = files();
    let (table, ids) = table(&files, 0, 5000);
    let plain = Arc::new(RowTable::create(&files, &TYPES, TableOptions::default()).unwrap());
    assert!(plain.summarized(NO_TRANSACTION, &[0]).is_none(), "of the row store");
    ended(&table, 1);
    assert_eq!(pieces(&table, NO_TRANSACTION), (4096, vec!["0 0..2047".to_owned(), "0 2048..4095".to_owned()], 904));
    changed(&table, (&[], &[ids[3]]), 10);
    assert_eq!(pieces(&table, NO_TRANSACTION), (2048, vec!["0 2048..4095".to_owned()], 2047 + 904), "the first shadowed");
    table.trust_shadow_bits(false);
    assert_eq!(pieces(&table, NO_TRANSACTION), (0, Vec::new(), 4999), "every group read, the bits not trusted");
}

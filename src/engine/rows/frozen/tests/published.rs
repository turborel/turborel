//! Unit tests of a group read between its place in the directory and the
//! marking of its shadow bits (`freeze.rs`, `shadow_bits.rs`, RULES.md
//! D1100), by a freeze driven step by step, with reads between its steps.

use super::*;
use crate::storage::Batch;

/// The keys of the rows of `batches`, sorted.
fn keys(batches: impl Iterator<Item = crate::error::Result<Batch>>) -> Vec<String> {
    let mut keys = Vec::new();
    for batch in batches {
        let mut batch = batch.unwrap();
        batch.resolve(None, &crate::kernels::Portable).unwrap();
        keys.extend((0..batch.rows).map(|r| batch.cell(0, r).to_value().to_string()));
    }
    keys.sort();
    keys
}

/// The keys `table` gives by batch, by the batches of the morsels of a
/// split of 500 rows each, and by row, each sorted.
fn read_each_way(table: &Arc<RowTable>) -> [Vec<String>; 3] {
    let by_batch = keys(Arc::clone(table).scan_batches(NO_TRANSACTION, &[0], None, &[]));
    let split = Arc::clone(table).split(NO_TRANSACTION, &[0], None, 500).unwrap();
    let by_morsel = keys((0..split.morsels()).flat_map(|n| split.morsel_batches(n, &[]).unwrap()));
    let mut by_row = scanned(table, NO_TRANSACTION);
    by_row.sort();
    [by_batch, by_morsel, by_row]
}

/// Verifies: [DESIGN] RULES.md D1100, a group read before its shadow bits
/// are marked. A freeze leaves out five rows of its group's range, their
/// commit not yet stamped on their entries as it reads them, and a row of
/// the group is updated and another deleted before the group is in the
/// directory. A reader that takes the group there before its bits are
/// marked reads the tree over its range whole, so it reads the five rows,
/// the row as updated, and not the row deleted, by batch, by morsel and by
/// row alike. Once the bits are marked and settled it reads the same, by
/// the bits. A regression test (R-TEST-7) that fails if a group's bits are
/// trusted before they are settled, the scan by batch then missing the
/// five rows, reading the row as frozen, and reading the row deleted.
#[test]
fn a_group_read_before_its_bits_are_marked_misses_no_row() {
    let (_, files) = files();
    let (table, ids) = table(&files, 0, 1000);
    table.begin(10, 1).unwrap();
    let late: Vec<RowId> = (1000..1005).map(|n| table.insert(10, row(n)).unwrap()).collect();
    for n in 1005..3000 {
        table.insert(NO_TRANSACTION, row(n)).unwrap();
    }
    let columns = table.frozen.columns().unwrap();
    // As a freeze notes itself, so a deletion of a row it may freeze is kept.
    let freezing = super::super::freeze::Freezing::from(&table.frozen.freezing_from, 0);
    let entries = table.eligible(None, 1, 2048).unwrap();
    assert_eq!(entries.len(), 2048);
    assert!(entries.iter().all(|(id, _)| !late.contains(id)), "left out: their commit is not stamped");
    table.validate(10, 2).unwrap();
    table.commit(10, 2);
    table.begin(12, 11).unwrap();
    table.update(12, ids[7], row(-7)).unwrap();
    table.delete(12, ids[8]).unwrap();
    table.validate(12, 12).unwrap();
    table.commit(12, 12);
    table.gc(12, &mut |_, _, _| true).unwrap();
    let made = table.made(table.written(&entries, &columns).unwrap()).unwrap();
    let put = table.put((None, Some(made))).unwrap();
    drop(freezing);
    let entry = Arc::clone(&table.frozen.directory().unwrap().groups[0]);
    assert!(late.iter().all(|&id| entry.summary.first < id && id < entry.summary.last), "within its range");
    let mut expected: Vec<String> = (0_i32..3000_i32).filter(|n| ![7_i32, 8_i32].contains(n)).map(|n| n.to_string()).collect();
    expected.push("-7".to_owned());
    expected.sort();
    let [by_batch, by_morsel, by_row] = read_each_way(&table);
    assert_eq!(by_batch.len(), 2999, "by batch, before its bits are marked");
    assert_eq!((by_batch, by_morsel, by_row), (expected.clone(), expected.clone(), expected.clone()));
    table.marked(put, entries).unwrap();
    assert!(late.iter().all(|&id| entry.shadow.may_hold(id)), "the rows left out, marked");
    assert!(entry.shadow.may_hold(ids[7]) && entry.shadow.may_hold(ids[8]) && !entry.shadow.may_hold(ids[9]), "settled");
    assert_eq!(read_each_way(&table), [expected.clone(), expected.clone(), expected], "by its bits");
}

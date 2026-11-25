//! Unit tests of a column store's morsels read by batch, their columns
//! asked for late given late (RULES.md D366).

use super::*;
use crate::storage::{Batch, Column};

/// The keys and texts of the rows of every morsel of `table`'s split of
/// those whose key is below 100, fewer than one in eight of a run, read by batch, the text asked for late
/// if `late`, each batch resolved; and whether any batch held it late.
fn morsels_of(table: &Arc<RowTable>, late: &[usize]) -> (Vec<String>, bool) {
    let below = Conjunction::of(Condition {
        columns: vec![0],
        probe: Probe::Compare { op: CompOp::Lt, value: Value::Exact(Decimal::new(100, 0)) },
    });
    let split = Arc::clone(table).split(NO_TRANSACTION, &[0, 1], Some(&below), 5000).unwrap();
    let (mut rows, mut held_late) = (Vec::new(), false);
    for n in 0..split.morsels() {
        for batch in split.morsel_batches(n, late).unwrap() {
            let mut batch: Batch = batch.unwrap();
            held_late |= matches!(batch.columns.get(1), Some(Column::Late(_)));
            batch.resolve(None, &crate::kernels::Portable).unwrap();
            rows.extend((0..batch.rows).map(|r| format!("{} {}", batch.cell(0, r).to_value(), batch.cell(1, r).to_value())));
        }
    }
    rows.sort();
    (rows, held_late)
}

/// Verifies: [DESIGN] RULES.md D366 a morsel of a column store's split
/// told a column late gives it as its rows' places, decoded once resolved
/// of the rows kept: the same rows as a morsel told none, each its text.
#[test]
fn morsels_give_late_columns_as_their_places() {
    let (_, files) = files();
    let (table, _) = table(&files, 0, 5000);
    ended(&table, 1);
    let (early, none_late) = morsels_of(&table, &[]);
    let (late, held_late) = morsels_of(&table, &[1]);
    assert!(!none_late && held_late, "late only as told");
    assert_eq!(late, early);
    assert!(early.iter().any(|r| r == "17 v17"), "{early:?}");
}

/// Verifies: [DESIGN] RULES.md D1052 a column store's split judges its
/// table's zones once, as it is made, and each morsel reads of its tree's
/// rows those of the zones its condition may match within its own row
/// ids alone: of 9000 rows not yet frozen, in zones of 1024 row ids, a
/// condition on the last thousand keys reads the last two zones' rows,
/// whatever the morsels they fall in, and no morsel reads another's.
#[test]
fn a_split_judges_its_zones_once_for_every_morsel() {
    let (_, files) = files();
    let (table, _) = table(&files, 3600, 9000);
    let last = Conjunction::of(Condition {
        columns: vec![0],
        probe: Probe::Compare { op: CompOp::Ge, value: Value::Exact(Decimal::new(8000, 0)) },
    });
    let read = |rows: usize| {
        let split = Arc::clone(&table).split(NO_TRANSACTION, &[0], Some(&last), rows).unwrap();
        let morsels = split.morsels();
        let given: usize = (0..morsels).flat_map(|n| split.morsel_batches(n, &[]).unwrap()).map(|b| b.unwrap().rows).sum();
        (morsels, given)
    };
    // The zones are kept from the first scan that asks them.
    let _ = read(9000);
    assert_eq!(read(9000), (1, 9000 - 7 * 1024), "one morsel, the zones of ids from 7168");
    let (morsels, given) = read(500);
    assert!(morsels > 9, "{morsels} morsels");
    assert_eq!(given, 9000 - 7 * 1024, "the same rows by many morsels");
}

/// Verifies: [DESIGN] RULES.md D1095 a scan that keeps few rows of each
/// run gives the vectors it decoded to judge a run back once it holds the
/// rows kept, and the next run is decoded into them: of two groups each
/// keeping a few rows, the second's judged column is held by the first's
/// vector, and the rows given are those the condition keeps.
#[test]
fn a_selective_scan_decodes_each_run_into_the_vectors_of_the_last() {
    use crate::engine::rows::frozen::Batched;
    use crate::storage::Conjunct;
    let (_, files) = files();
    let (table, _) = table(&files, 0, 4096);
    ended(&table, 1);
    let bound =
        |op, n| Conjunct::Condition(Condition { columns: vec![0], probe: Probe::Compare { op, value: Value::Exact(Decimal::new(n, 0)) } });
    let few = Conjunction(vec![bound(CompOp::Ge, 2040), bound(CompOp::Lt, 2060)]);
    let reader = table.reader(NO_TRANSACTION).unwrap();
    let directory = table.directory_for(reader).unwrap();
    assert_eq!(directory.groups.len(), 2, "two groups frozen");
    let mut scan = Batched::new(Arc::clone(&table), (reader, directory), vec![0, 1], Some(few));
    let mut keys = Vec::new();
    for batch in &mut scan {
        let batch = batch.unwrap();
        keys.extend((0..batch.rows).map(|r| batch.cell(0, r).to_value().to_string()));
    }
    keys.sort();
    assert_eq!(keys, (2040..2060).map(|n| n.to_string()).collect::<Vec<_>>());
    assert_eq!(scan.reused(), 1, "the second group's column in the first's vector");
}

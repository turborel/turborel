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

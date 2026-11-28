//! Unit tests of a column store's morsels read by batch, their columns
//! asked for late given late (RULES.md D366), under a sort's bound too
//! (RULES.md D1241).

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

/// Of the batches of `table`'s one morsel under `condition`, the text
/// asked for late: how many rows they give, and how many of those hold
/// the text decoded, not as their places.
fn decoded_of(table: &Arc<RowTable>, condition: &Conjunction) -> (usize, usize) {
    let split = Arc::clone(table).split(NO_TRANSACTION, &[0, 1], Some(condition), 5000).unwrap();
    let (mut rows, mut decoded) = (0_usize, 0_usize);
    for batch in (0..split.morsels()).flat_map(|n| split.morsel_batches(n, &[1]).unwrap()) {
        let batch: Batch = batch.unwrap();
        rows = rows.saturating_add(batch.rows);
        if !matches!(batch.columns.get(1), Some(Column::Late(_))) {
            decoded = decoded.saturating_add(batch.rows);
        }
    }
    (rows, decoded)
}

/// Verifies: [DESIGN] RULES.md D1241, D366 a run the scan judged and kept
/// most rows of is given whole. Judged by a comparison, its late column
/// is decoded with it, of every row. Judged by a sort's bound that keeps
/// as many, its late column stays late, no value of a frozen run decoded,
/// as the sort keeps few of the rows it is given; and so does a run under
/// a bound that holds no value yet.
#[test]
fn a_run_given_whole_under_a_bound_keeps_its_late_column_late() {
    use crate::storage::{Conjunct, Threshold};
    let (_, files) = files();
    let (table, _) = table(&files, 0, 5000);
    ended(&table, 1);
    let most = Value::Exact(Decimal::new(4000, 0));
    let compared = Conjunction::of(Condition { columns: vec![0], probe: Probe::Compare { op: CompOp::Le, value: most.clone() } });
    let (rows, decoded) = decoded_of(&table, &compared);
    assert!(rows > 4000 && decoded == rows, "a comparison keeping most rows decodes the late column: {decoded} of {rows}");
    // The rows not yet frozen are read from the tree, decoded as they are
    // read, whatever is asked late: those a scan told nothing decodes.
    let (all, of_tree) = decoded_of(&table, &Conjunction(Vec::new()));
    assert!(all == 5000 && of_tree < 1024, "{of_tree} of {all} from the tree");
    let threshold = Arc::new(Threshold::new(false));
    let bounded = Conjunction(vec![Conjunct::Dynamic { column: 0, threshold: Arc::clone(&threshold) }]);
    assert_eq!(decoded_of(&table, &bounded), (5000, of_tree), "a bound not yet set");
    threshold.raise(&most);
    let (under, decoded_under) = decoded_of(&table, &bounded);
    // The tree's rows are all past the bound, their zone passed over.
    assert!(under > 4000 && decoded_under == 0, "a bound keeping most rows decodes none of a frozen run: {decoded_under} of {under}");
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

/// Verifies: [DESIGN] RULES.md D1267 a morsel of a column store asked for
/// a VARCHAR column measured gives it late with the octet length of each
/// row's value, of a run given whole and of rows gathered at their
/// places under a condition; `Batch::resolve` leaves it late; its values,
/// read after all, are the texts the scan would have decoded, each of the
/// length held. A column of integers asked for measured is given
/// decoded, as it has no octet length to take. MC/DC of the column
/// given: one that can be measured (late, lengths) against one that
/// cannot (decoded).
#[test]
fn a_morsel_gives_a_column_asked_measured_late_with_its_lengths() {
    let (_, files) = files();
    let (table, _) = table(&files, 0, 5000);
    ended(&table, 1);
    let below = Conjunction::of(Condition {
        columns: vec![0],
        probe: Probe::Compare { op: CompOp::Lt, value: Value::Exact(Decimal::new(100, 0)) },
    });
    for condition in [None, Some(&below)] {
        let split = Arc::clone(&table).split(NO_TRANSACTION, &[0, 1], condition, 5000).unwrap();
        let (mut rows, mut measured) = (0_usize, 0_usize);
        for batch in (0..split.morsels()).flat_map(|n| split.morsel_measured(n, (&[], &[0, 1])).unwrap()) {
            let mut batch: Batch = batch.unwrap();
            rows = rows.saturating_add(batch.rows);
            assert!(!matches!(batch.columns.first(), Some(Column::Late(_))), "integers are given decoded");
            let Some(Column::Late(late)) = batch.columns.get(1).cloned() else { continue };
            let lengths = late.lengths().expect("a late column asked measured holds its lengths");
            batch.resolve(None, &crate::kernels::Portable).unwrap();
            assert!(matches!(batch.columns.get(1), Some(Column::Late(_))), "left late by resolve");
            for r in 0..batch.rows {
                let value = batch.cell(1, r).to_value();
                if let Value::Char(text) = &value {
                    assert_eq!((lengths.units[r], lengths.is_null(r)), (i64::try_from(text.len()).unwrap(), false), "{value}");
                } else {
                    assert!(value.is_null() && lengths.is_null(r), "{value}");
                }
            }
            measured = measured.saturating_add(batch.rows);
        }
        assert!(measured >= 100 && measured <= rows, "frozen rows are given measured: {measured} of {rows}");
        assert_eq!(condition.is_none(), rows == 5000);
    }
}

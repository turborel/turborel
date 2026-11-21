//! Unit tests of statistics read after the table changed (RULES.md
//! R-TEST-1, R-TEST-2).

use super::super::StatsBuilder;
use super::*;
use crate::value::Decimal;

fn number(n: u32) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}

/// The statistics of a table read whole: a column of 1 to 1,000, a key,
/// and one of ten values.
fn table() -> TableStats {
    let mut builder = StatsBuilder::new(2);
    for n in 1..=1_000_u32 {
        builder.add(&vec![number(n), number(n % 10)], 9);
    }
    builder.finish()
}

/// Verifies: [DESIGN] RULES.md D381, bounds after change: of a table read
/// whole and unchanged, a value past the greatest is matched by no row;
/// once rows were written since, a range past it keeps as many as an
/// equality would, not none; of a column whose greatest value rose at
/// each of the last two passes, the share of the rows written since.
/// MC/DC of the growth: rose twice and changed (the share written), rose
/// once (an equality's), unchanged (none).
#[test]
fn a_range_past_the_greatest_value_keeps_rows_written_since() {
    let stats = table();
    let key = &stats.columns[0];
    let past = number(5_000);
    assert_eq!(key.selectivity_since(CompOp::Gt, &past, Since::NONE), 0.0_f64);
    let since = Since { changes: 100.0, rows: 1_000.0, live: 1_100.0 };
    let changed = key.selectivity_since(CompOp::Gt, &past, since);
    assert!(changed > 0.0_f64 && changed < 0.01_f64, "as an equality: {changed}");
    assert!(key.selectivity_since(CompOp::Eq, &past, since) > 0.0_f64, "an equality past it, once changed");
    let mut growing = key.clone();
    growing.rises = GROWING;
    assert!(growing.grows() && !key.grows());
    let grown = growing.selectivity_since(CompOp::Ge, &past, since);
    assert!((grown - 0.1_f64).abs() < 1e-9_f64, "the share written since: {grown}");
    assert_eq!(growing.selectivity_since(CompOp::Gt, &past, Since::NONE), 0.0_f64, "unchanged");
    let mut once = key.clone();
    once.rises = 1;
    assert!((once.selectivity_since(CompOp::Gt, &past, since) - changed).abs() < 1e-12_f64, "rose once");
    let before = key.selectivity_since(CompOp::Lt, &number(0), since);
    assert!(before > 0.0_f64 && before < 0.01_f64, "below the least: {before}");
}

/// Verifies: [DESIGN] RULES.md D381: a key's distinct values grow with
/// the rows the table holds now, any other column's do not; statistics
/// read unchanged are the pass's own, and read after change answer by
/// the change. MC/DC of the key test: distinct near the rows (scaled)
/// against far from them (not).
#[test]
fn a_keys_distinct_values_grow_with_the_rows() {
    let stats = Arc::new(table());
    let since = Since { changes: 1_000.0, rows: 1_000.0, live: 2_000.0 };
    assert!((stats.columns[0].distinct_now(since) / 2_000.0_f64 - 1.0_f64).abs() < 0.05_f64);
    assert!((stats.columns[1].distinct_now(since) - 10.0_f64).abs() < 0.5_f64);
    let same = Changed::of(Arc::clone(&stats), 0, 1_000);
    assert_eq!(same.distinct(0), stats.distinct(0));
    let changed = Changed::of(Arc::clone(&stats), 1_000, 2_000);
    assert!(changed.distinct(0).unwrap() > 1_900.0_f64);
    assert!(changed.selectivity(0, CompOp::Gt, &number(5_000)).unwrap() > 0.0_f64);
    assert_eq!(changed.rows(), Some(1_000.0_f64));
    assert!(changed.sample().is_some() && changed.common(1).is_some());
}

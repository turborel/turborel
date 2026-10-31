//! Unit tests of the optimizer's statistics: the sketch's accuracy, the
//! histogram's fractions, the selectivities they give, a deterministic
//! sample, and the parts built a column at a time (RULES.md R-TEST-1,
//! R-TEST-2). Split from stats.rs (RULES.md R-SIZE-4).

use super::parts::{Histogram, HyperLogLog, Reservoir, SAMPLE_VALUES, SAMPLED_BITS, SAMPLED_CHARACTERS};
use super::*;
use crate::value::Decimal;

fn number(n: u32) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}

/// Whether `got` is within `by` of `want`.
fn near(got: f64, want: f64, by: f64) -> bool {
    (got - want).abs() < by
}

/// Verifies: [DESIGN] RULES.md D31, distinct values: a HyperLogLog sketch
/// of 4,096 registers estimates 1,000 and 100,000 distinct values within
/// 5%, though each is added ten times; NULL is not counted; an empty
/// sketch estimates none.
#[test]
fn distinct_values_are_estimated() {
    assert_eq!(HyperLogLog::new().estimate(), 0.0_f64);
    for n in [1_000_u32, 100_000] {
        let mut sketch = HyperLogLog::new();
        for _ in 0..10_u32 {
            for i in 0..n {
                sketch.add(&number(i));
            }
        }
        sketch.add(&Value::Null);
        let want = f64::from(n);
        assert!(near(sketch.estimate() / want, 1.0, 0.05), "{n} distinct values estimated as {}", sketch.estimate());
    }
}

/// Verifies: [DESIGN] RULES.md D31, distinct values: values SQL compares
/// equal ('a' and 'a  ', 1 and 1.0) count once; sketches of parts of a
/// column merge into the sketch of the whole.
#[test]
fn sketches_count_equal_values_once_and_merge() {
    let mut equal = HyperLogLog::new();
    for value in [Value::Char("a".into()), Value::Char("a  ".into()), number(1), Value::Exact(Decimal::new(10, 1))] {
        equal.add(&value);
    }
    assert!(near(equal.estimate(), 2.0, 0.1), "two values: {}", equal.estimate());
    let (mut low, mut high, mut whole) = (HyperLogLog::new(), HyperLogLog::new(), HyperLogLog::new());
    for i in 0..5_000 {
        if i < 3_000 {
            low.add(&number(i));
        } else {
            high.add(&number(i));
        }
        whole.add(&number(i));
    }
    low.merge(&high);
    assert_eq!(low, whole);
}

/// The statistics of a column of `n` rows holding 0 to `n - 1`, every
/// tenth NULL.
fn numbers(n: u32) -> TableStats {
    let mut builder = StatsBuilder::new(1);
    for i in 0..n {
        let value = if i % 10 == 0 { Value::Null } else { number(i) };
        builder.add(&vec![value], 9);
    }
    builder.finish()
}

/// Verifies: [DESIGN] RULES.md D31, statistics: a table's rows and average
/// width; a column's NULL fraction, least and greatest values, and
/// distinct values; for a table of more rows than a sample holds as for
/// one of fewer.
#[test]
fn columns_are_described() {
    for n in [1_000_u32, 50_000] {
        let stats = numbers(n);
        let column = &stats.columns[0];
        assert_eq!((stats.rows, stats.width), (u64::from(n), 9.0_f64));
        assert!(near(column.nulls, 0.1, 1e-9));
        assert_eq!(format!("{:?}", column.min), format!("{:?}", Some(number(1))));
        assert_eq!(format!("{:?}", column.max), format!("{:?}", Some(number(n - 1))));
        assert!(near(column.distinct / (f64::from(n) * 0.9), 1.0, 0.05), "{} distinct", column.distinct);
    }
}

/// Verifies: [DESIGN] RULES.md D31, selectivity: for each operator, from
/// the histogram and distinct values, a tenth of NULLs kept out of each;
/// none for NULL [X3.135-1986 §5.11 GR2], nor for equality outside the
/// values' range; every non-null row below a value above the greatest.
#[test]
fn selectivities_follow_the_values() {
    for n in [1_000_u32, 50_000] {
        let column = numbers(n).columns.swap_remove(0);
        let (half, quarter) = (number(n >> 1_u32), number(n >> 2_u32));
        let close = |got: f64, want: f64| near(got, want, 0.03);
        assert!(close(column.selectivity(CompOp::Lt, &half), 0.45_f64), "{}", column.selectivity(CompOp::Lt, &half));
        assert!(close(column.selectivity(CompOp::Ge, &half), 0.45_f64));
        assert!(close(column.selectivity(CompOp::Gt, &quarter), 0.675_f64));
        assert!(close(column.selectivity(CompOp::Ne, &half), 0.9_f64));
        assert!(column.selectivity(CompOp::Eq, &half) < 0.01_f64);
        assert!(column.selectivity(CompOp::Le, &half) >= column.selectivity(CompOp::Lt, &half));
        assert!(close(column.selectivity(CompOp::Lt, &number(n * 2)), 0.9_f64), "above the greatest");
        assert_eq!(column.selectivity(CompOp::Eq, &number(0)), 0.0_f64, "below the least");
        assert_eq!(column.selectivity(CompOp::Eq, &Value::Null), 0.0_f64, "a comparison with NULL");
    }
}

/// Verifies: [DESIGN] RULES.md D31, statistics: the sample is the same for
/// the same rows, so plans do not change between runs; strings are placed
/// in the histogram's buckets, half of a bucket for one inside it; an
/// empty table has no rows, width or values, and an empty histogram knows
/// nothing.
#[test]
fn statistics_are_deterministic() {
    let strings = || {
        let mut builder = StatsBuilder::new(1);
        for i in 0..30_000_u32 {
            builder.add(&vec![Value::Char(format!("{i:06}"))], 6);
        }
        builder.finish()
    };
    let (a, b) = (strings(), strings());
    let probe = Value::Char("015000".into());
    let below = a.columns[0].selectivity(CompOp::Lt, &probe);
    assert_eq!(below, b.columns[0].selectivity(CompOp::Lt, &probe));
    assert!(near(below, 0.5, 0.05), "{below}");
    let empty = StatsBuilder::new(2).finish();
    assert_eq!((empty.rows, empty.width, empty.columns[0].min.is_none()), (0, 0.0_f64, true));
    assert_eq!(empty.columns[0].selectivity(CompOp::Eq, &number(1)), 0.0_f64);
    assert_eq!(Histogram::default().below(&number(1)), None);
}

/// Verifies: [DESIGN] RULES.md D31, samples of long values: a character
/// string is sampled as its first `SAMPLED_CHARACTERS` characters, counted
/// as characters, not bytes, and a bit string as its first `SAMPLED_BITS`
/// bits, short ones whole; so a sample of strings of 10,000 characters
/// takes no more than one of short ones, and its histogram still places a
/// value by its beginning, the least and greatest kept whole.
#[test]
fn long_values_are_sampled_by_their_prefixes() {
    let long = |i: u32| Value::Char(format!("{i:05}{}", "\u{e9}".repeat(10_000)));
    let mut builder = StatsBuilder::new(1);
    let mut sample = Reservoir::keeping(100);
    for i in 0..1_000_u32 {
        builder.add(&vec![long(i)], 20_005);
        sample.add(&long(i));
    }
    let kept = sample.into_sorted();
    let lengths: Vec<usize> = kept.iter().map(|v| v.to_string().chars().count()).collect();
    assert_eq!(lengths, vec![SAMPLED_CHARACTERS; 100], "prefixes kept");
    let stats = builder.finish();
    let column = &stats.columns[0];
    assert!(near(column.selectivity(CompOp::Lt, &long(500)), 0.5, 0.05), "{}", column.selectivity(CompOp::Lt, &long(500)));
    assert_eq!(format!("{:?}", column.max), format!("{:?}", Some(long(999))), "the greatest whole");
    let short = Value::Char("short".into());
    let bits = |n: u32| Value::Bits(crate::value::Bits::of_bits(&"10".repeat(usize::try_from(n >> 1_u32).unwrap())).unwrap());
    for (value, length) in [(short, 5_u32), (bits(8), 8), (bits(SAMPLED_BITS), SAMPLED_BITS), (bits(5_000), SAMPLED_BITS)] {
        let mut one = Reservoir::keeping(1);
        one.add(&value);
        let got = one.into_sorted();
        let sampled_length = match got.first() {
            Some(Value::Char(s)) => u32::try_from(s.chars().count()).unwrap(),
            Some(Value::Bits(b)) => b.len(),
            other => panic!("{other:?}"),
        };
        assert_eq!(sampled_length, length);
    }
}

/// Rows of two columns: 700 values repeated, and a third NULL.
fn two_columns() -> Vec<Row> {
    (0..20_000).map(|i| vec![number(i % 700), if i % 3 == 0 { Value::Null } else { number(i) }]).collect()
}

/// Verifies: [DESIGN] RULES.md D31, extensibility: a column's statistics
/// built a column at a time, as a columnar engine would, are those built a
/// row at a time; a sample keeps at most `SAMPLE_VALUES`, NULL never.
#[test]
fn statistics_are_built_a_column_at_a_time() {
    let rows = two_columns();
    let mut by_rows = StatsBuilder::new(2);
    for row in &rows {
        by_rows.add(row, 10);
    }
    let by_rows = by_rows.finish();
    for (c, stats) in by_rows.columns.iter().enumerate() {
        let mut column = ColumnBuilder::new();
        for row in &rows {
            column.add(&row[c]);
        }
        let alone = column.finish();
        assert_eq!((alone.distinct, alone.nulls), (stats.distinct, stats.nulls));
        assert_eq!(format!("{:?} {:?}", alone.min, alone.max), format!("{:?} {:?}", stats.min, stats.max));
        assert_eq!(alone.selectivity(CompOp::Lt, &number(350)), stats.selectivity(CompOp::Lt, &number(350)));
    }
    let mut sample = Reservoir::new();
    for i in 0..30_000 {
        sample.add(&number(i));
        sample.add(&Value::Null);
    }
    let kept = sample.into_sorted();
    assert_eq!(kept.len(), SAMPLE_VALUES);
    assert!(kept.iter().all(|v| !v.is_null()));
}

/// Statistics that know nothing, as an engine might give.
#[derive(Debug)]
struct Unknown;

impl Statistics for Unknown {}

/// Verifies: [DESIGN] RULES.md D31, extensibility: the planner asks through
/// `Statistics`; `TableStats` answers for its columns and not for others;
/// an engine knowing nothing answers nothing.
#[test]
fn statistics_answer_through_the_trait() {
    let mut builder = StatsBuilder::new(2);
    for row in &two_columns() {
        builder.add(row, 10);
    }
    let built = builder.finish();
    let table: &dyn Statistics = &built;
    assert_eq!((table.rows(), table.width(&[1])), (Some(20_000.0_f64), Some(10.0_f64)));
    assert!(table.distinct(0).is_some_and(|d| near(d, 700.0, 35.0)));
    assert!(table.nulls(1).is_some_and(|n| near(n, 1.0 / 3.0, 1e-3)));
    assert!(table.selectivity(0, CompOp::Eq, &number(5)).is_some());
    assert_eq!((table.distinct(2), table.nulls(2), table.selectivity(2, CompOp::Eq, &number(5))), (None, None, None));
    let unknown: &dyn Statistics = &Unknown;
    assert_eq!((unknown.rows(), unknown.width(&[]), unknown.distinct(0), unknown.nulls(0)), (None, None, None, None));
    assert_eq!(unknown.selectivity(0, CompOp::Eq, &number(1)), None);
}

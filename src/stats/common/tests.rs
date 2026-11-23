//! Unit tests of a column's most common values (RULES.md R-TEST-1,
//! R-TEST-2).

use super::super::parts::xorshift;
use super::super::{Statistics, StatsBuilder};
use super::*;
use crate::value::{CompOp, Decimal};

fn number(n: u64) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}

/// The rows of a skewed column: value k, for k of 1 to 1,000, held by
/// 20,000 / k rows, a Zipf distribution, in an order shuffled from a
/// fixed seed; and each value's rows.
fn zipf() -> (Vec<u64>, Vec<u32>) {
    let held: Vec<u32> = (1..=1_000_u32).map(|k| 20_000_u32.div_euclid(k)).collect();
    let mut rows: Vec<u64> = Vec::new();
    for (k, &n) in (1_u64..).zip(&held) {
        rows.extend(std::iter::repeat_n(k, usize::try_from(n).unwrap()));
    }
    let mut random = 7_u64;
    for i in (1..rows.len()).rev() {
        let j = usize::try_from(xorshift(&mut random).checked_rem(u64::try_from(i).unwrap().saturating_add(1)).unwrap()).unwrap();
        rows.swap(i, j);
    }
    (rows, held)
}

/// `n` as a float.
fn float(n: usize) -> f64 {
    f64::from(u32::try_from(n).unwrap())
}

/// Verifies: [DESIGN] RULES.md D381: of a skewed column, a Zipf
/// distribution sampled, each of the five most frequent values is
/// estimated within 1.2 times its true share by its own common share,
/// where the share every distinct value would hold is tens of times
/// off; a value not common is estimated by the rest shared among the
/// others, near its own; and an order comparison counts the common values
/// below its value.
#[test]
fn a_skewed_columns_common_values_are_estimated_by_their_shares() {
    let (rows, held) = zipf();
    let mut builder = StatsBuilder::new(1);
    for &v in &rows {
        builder.add(&vec![number(v)], 9);
    }
    let stats = builder.finish();
    let all = float(rows.len());
    assert!(!stats.columns[0].common.is_empty() && stats.columns[0].common.len() <= COMMON_VALUES);
    for k in 1..=5_u64 {
        let truth = f64::from(held[usize::try_from(k).unwrap() - 1]) / all;
        let got = stats.selectivity(0, CompOp::Eq, &number(k)).unwrap();
        assert!(got / truth < 1.2_f64 && truth / got < 1.2_f64, "value {k}: {got} for {truth}");
        let even = 1.0_f64 / stats.columns[0].distinct;
        assert!(truth / even > 20.0_f64, "the even share is far off: {even} for {truth}");
    }
    let (rare, truth) = (number(400), f64::from(held[399]) / all);
    let rare_got = stats.selectivity(0, CompOp::Eq, &rare).unwrap();
    assert!(rare_got / truth < 10.0_f64 && truth / rare_got < 10.0_f64, "a rare value: {rare_got} for {truth}");
    let below_three = f64::from(held[0] + held[1]) / all;
    let below = stats.selectivity(0, CompOp::Lt, &number(3)).unwrap();
    assert!(below / below_three < 1.2_f64 && below_three / below < 1.2_f64, "below 3: {below} for {below_three}");
}

/// Verifies: [DESIGN] RULES.md D381: a column of evenly spread values
/// keeps no common value, its histogram the whole answer; a value seen
/// once is never common; a string the sample cuts is never common, one
/// it keeps whole is; the sample without the common values keeps the
/// others in order.
#[test]
fn only_values_far_more_frequent_than_others_are_common() {
    let even: Vec<Value> = (0..10_000_u64).map(|i| number(i % 100)).collect();
    let mut sorted = even.clone();
    sorted.sort_by(|a, b| compare(a, b).unwrap());
    assert!(common_of(sorted, 1.0, 100.0).0.is_empty(), "evenly spread");
    assert!(common_of(vec![number(1), number(2)], 1.0, 1.0).0.is_empty(), "each seen once");
    let long = Value::Char("x".repeat(SAMPLED_CHARACTERS).into());
    let short = Value::Char("short".into());
    let values = vec![long.clone(), long.clone(), long, number(5), short.clone(), short.clone(), short.clone()];
    let (common, rest) = common_of(values, 0.5, 4.0);
    assert_eq!(common.len(), 1);
    assert_eq!(compare(&common[0].0, &short), Some(Ordering::Equal));
    assert!((common[0].1 - 0.5 * 3.0 / 7.0).abs() < 1e-9_f64, "its share of all rows: {}", common[0].1);
    assert_eq!(rest.len(), 4, "the others kept");
    assert_eq!(share_below(&[(number(1), 0.25_f64), (number(9), 0.5_f64)], &number(5)), 0.25_f64);
    assert_eq!(share_all(&[(number(1), 0.25_f64), (number(9), 0.5_f64)]), 0.75_f64);
}

//! Unit tests of a row table's sizes: a sum near what a count holds (RULES.md D1165, R-TEST-1).

use super::within;

/// Verifies: [DESIGN] RULES.md D1165, a sum of bytes up to the largest a
/// `BIGINT` holds kept, one past it or overflowed none.
#[test]
fn a_sum_past_a_count_is_none() {
    let largest = u64::try_from(i64::MAX).expect("a largest BIGINT is a u64");
    assert_eq!(within(Some(largest)), Some(largest));
    assert_eq!(within((largest - 10).checked_add(10)), Some(largest));
    assert_eq!(within(largest.checked_add(1)), None);
    assert_eq!(within(u64::MAX.checked_add(1)), None);
}

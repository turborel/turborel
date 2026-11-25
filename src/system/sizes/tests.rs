//! Unit tests of the sizes the system views show: a count up to the
//! largest a column of counts holds shown, one past it the null value
//! (RULES.md D1165, R-TEST-1).

use super::maybe;

/// Verifies: [DESIGN] RULES.md D1165, a count the views show: up to the
/// largest a `BIGINT` holds as itself, one past it, and none, the null
/// value, never a number no column of counts holds.
#[test]
fn a_count_past_a_bigint_is_shown_as_null() {
    let largest = u64::try_from(i64::MAX).expect("a largest BIGINT is a u64");
    assert_eq!(maybe(Some(largest)).to_string(), i64::MAX.to_string());
    assert!(maybe(Some(largest + 1)).is_null());
    assert!(maybe(Some(u64::MAX)).is_null());
    assert!(maybe(None).is_null());
}

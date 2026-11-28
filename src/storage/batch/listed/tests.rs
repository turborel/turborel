//! Unit tests of an IN list of exact constants folded to a scale
//! (`listed.rs`, RULES.md D1295).

use std::cmp::Ordering;

use super::*;
use crate::value::{Decimal, compare};

fn exact(units: i128, scale: u32) -> Value {
    Value::Exact(Decimal::new(units, scale))
}

/// [X3.135-1986 §5.13 GR2, §5.18] Whether `v IN (values)`, or NOT IN if
/// `not`, is true, each constant compared with `v` in turn as values:
/// unknown, so not true, of NOT IN with a null constant and no equal one.
fn by_values(v: &Value, values: &[Value], not: bool) -> bool {
    let equal = values.iter().any(|c| compare(v, c) == Some(Ordering::Equal));
    if not { !equal && !values.iter().any(Value::is_null) } else { equal }
}

/// Asserts the list of `values` at `scale`, IN and NOT IN, is true of
/// each of `units` as comparing its value with each constant says.
fn as_values(scale: u32, values: &[Value], units: &[i64], what: &str) {
    for not in [false, true] {
        let listed = Listed::of(scale, values, not).unwrap_or_else(|| panic!("{what}: not folded"));
        for &u in units {
            assert_eq!(listed.holds(u), by_values(&exact(i128::from(u), scale), values, not), "{what}: {u} not={not}");
        }
    }
}

/// The units from `from` to `to`, and the ends of an `i64`.
fn around(from: i64, to: i64) -> Vec<i64> {
    (from..=to).chain([i64::MIN, i64::MIN.saturating_add(1), i64::MAX.saturating_sub(1), i64::MAX]).collect()
}

/// Verifies: [DESIGN] RULES.md D1295 [X3.135-1986 §5.13 GR2, §5.18]
/// (MC/DC): a list folded to a scale is true of the units, and of those
/// alone, whose value equals a constant, and NOT IN of the others unless
/// a constant is null, when it is true of none: of constants out of
/// order and twice over; of a null among them; of constants that span
/// fewer than 64 units, exactly 63, exactly 64, and more, so found by a
/// bit or by halving; of negative ones; of the least and the greatest an
/// integer holds, whose span no integer holds; of one constant; and of
/// none but a null.
#[test]
fn a_list_finds_the_units_equal_to_a_constant() {
    let n = |units: i128| exact(units, 0);
    as_values(0, &[n(49), n(14), n(23), n(45), n(19), n(3), n(36), n(9)], &around(-5, 70), "eight sizes");
    as_values(0, &[n(9), n(3), n(9), n(3), n(3)], &around(-5, 20), "twice over");
    as_values(0, &[n(9), Value::Null, n(3)], &around(-5, 20), "a null among them");
    as_values(0, &[n(-10), n(53)], &around(-80, 80), "a span of 63");
    as_values(0, &[n(-10), n(54)], &around(-80, 80), "a span of 64");
    as_values(0, &[n(-10), n(55), n(7)], &around(-80, 80), "a span of 65");
    as_values(0, &[n(-1000), n(0), n(1000), n(-3)], &around(-1010, 1010), "far apart");
    as_values(0, &[n(i128::from(i64::MIN)), n(i128::from(i64::MAX)), n(0)], &around(-3, 3), "the ends of an integer");
    as_values(0, &[n(i128::from(i64::MAX)), n(i128::from(i64::MAX.saturating_sub(5)))], &around(-3, 3), "near the greatest");
    as_values(0, &[n(7)], &around(0, 10), "one constant");
    as_values(0, &[Value::Null], &around(0, 3), "a null alone");
    as_values(0, &[], &around(0, 3), "none");
}

/// Verifies: [DESIGN] RULES.md D1295 [X3.135-1986 §5.11 GR4]: a constant
/// folds to the units of the vector's scale as an equality does. One of
/// fewer fractional digits is scaled up; one of more equals the unit the
/// scale divides it to, and no unit if it does not divide; one past 64
/// bits equals none; and each such constant is left out without the
/// others being lost. A list with a constant not exact, or too large to
/// scale, is not folded.
#[test]
fn constants_fold_to_the_scale_or_equal_nothing() {
    as_values(0, &[exact(15, 1), exact(20, 1), exact(3000, 3)], &around(0, 4), "1.5, 2.0 and 3.000 of integers");
    as_values(2, &[exact(3, 0), exact(15, 1), exact(1505, 3), exact(1510, 3), exact(-2, 0)], &around(-310, 310), "of hundredths");
    as_values(0, &[exact(100_000_000_000_000_000_000, 0), exact(5, 0)], &around(0, 9), "past 64 bits");
    as_values(2, &[exact(i128::from(i64::MAX), 0), exact(1, 0)], &around(90, 110), "past 64 bits once scaled");
    assert!(Listed::of(0, &[exact(5, 0), Value::Approx(5.0)], false).is_none(), "an approximate constant");
    assert!(Listed::of(0, &[Value::Char("5".into())], true).is_none(), "a string");
    assert!(Listed::of(30, &[exact(i128::MAX, 0)], false).is_none(), "too large to scale");
}

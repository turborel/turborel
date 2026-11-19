//! Tests of decimals of any number of digits.

use super::*;

fn d(text: &str) -> Decimal {
    Decimal::parse(text).unwrap()
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [Schema2 §3.2.3.1]
/// [XQueryFO §17.1.2] a decimal read from its lexical forms has one form,
/// zeros at either end dropped, written as casting it to a string writes
/// it; no form is none.
#[test]
fn decimals_have_one_form() {
    for (text, expected) in [("+012.50", "12.5"), ("-0.0", "0"), (".5", "0.5"), ("100.", "100"), ("-007", "-7"), ("0.000", "0")] {
        assert_eq!(d(text).text(), expected, "{text}");
    }
    for text in ["", ".", "1.2.3", "1e3", "--1", "1 "] {
        assert!(Decimal::parse(text).is_none(), "{text}");
    }
    assert!(d("-0").is_zero() && !d("-0").is_negative());
    assert!(d("5").is_integer() && !d("5.5").is_integer());
    assert_eq!(Decimal::integer(-42).text(), "-42");
    assert_eq!(d("12").to_i128(), Some(12));
    assert_eq!(d("1.5").to_i128(), None);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §6.2.1 to
/// §6.2.3] sums, differences and products are exact, of any sign and
/// scale, carries and borrows across every digit.
#[test]
fn sums_differences_and_products_are_exact() {
    assert_eq!(d("0.1").add(&d("0.2")).text(), "0.3");
    assert_eq!(d("999.99").add(&d("0.01")).text(), "1000");
    assert_eq!(d("-5").add(&d("3.25")).text(), "-1.75");
    assert_eq!(d("3.25").add(&d("-5")).text(), "-1.75");
    assert_eq!(d("2").subtract(&d("2.000")).text(), "0");
    assert_eq!(d("1000").subtract(&d("0.001")).text(), "999.999");
    assert_eq!(d("-1.5").multiply(&d("-1.5")).text(), "2.25");
    assert_eq!(d("123456789").multiply(&d("987654321")).text(), "121932631112635269");
    assert_eq!(d("0.5").multiply(&d("-0")).text(), "0");
    assert_eq!(d("2.5").negated().text(), "-2.5");
    assert_eq!(d("-2.5").abs().text(), "2.5");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §6.2.4 to
/// §6.2.6] quotients exact when they end, else truncated at the scale
/// asked; a remainder of the dividend's sign; none of a zero divisor.
#[test]
fn quotients_and_remainders() {
    assert_eq!(d("1").quotient(&d("4")).unwrap().text(), "0.25");
    assert_eq!(d("1").quotient(&d("3")).unwrap().text(), "0.333333333333333333");
    assert_eq!(d("-2").quotient(&d("3")).unwrap().text(), "-0.666666666666666666");
    assert_eq!(d("10").divide(&d("3"), 0).unwrap().text(), "3");
    assert_eq!(d("-3.5").divide(&d("3"), 0).unwrap().text(), "-1");
    assert_eq!(d("1").divide(&d("0.001"), 0).unwrap().text(), "1000");
    assert_eq!(d("4.5").remainder(&d("1.2")).unwrap().text(), "0.9");
    assert_eq!(d("-10").remainder(&d("3")).unwrap().text(), "-1");
    assert_eq!(d("10").remainder(&d("-3")).unwrap().text(), "1");
    assert!(d("1").quotient(&d("0.0")).is_none());
    assert!(d("1").remainder(&d("0")).is_none());
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [Schema2 §3.2.3] the
/// order of decimals is total, a negative less than a positive, scales
/// compared by value.
#[test]
fn decimals_are_ordered() {
    use std::cmp::Ordering::{Equal, Greater, Less};
    let cases = [("1.5", "1.50", Equal), ("-1", "0.5", Less), ("-1", "-2", Greater), ("10", "9.99", Greater), ("0.01", "0.1", Less)];
    for (a, b, expected) in cases {
        assert_eq!(d(a).compare(&d(b)), expected, "{a} against {b}");
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §6.4.2 to
/// §6.4.5] rounding to a precision: floor and ceiling of either sign,
/// halves toward positive infinity or to even, and to tens of a negative
/// precision.
#[test]
fn rounding_by_each_mode() {
    let cases = [
        ("10.5", 0, Rounding::Ceiling, "11"),
        ("-10.5", 0, Rounding::Ceiling, "-10"),
        ("10.5", 0, Rounding::Floor, "10"),
        ("-10.5", 0, Rounding::Floor, "-11"),
        ("2.5", 0, Rounding::HalfUp, "3"),
        ("2.4999", 0, Rounding::HalfUp, "2"),
        ("-2.5", 0, Rounding::HalfUp, "-2"),
        ("-2.51", 0, Rounding::HalfUp, "-3"),
        ("0.5", 0, Rounding::HalfEven, "0"),
        ("1.5", 0, Rounding::HalfEven, "2"),
        ("2.5", 0, Rounding::HalfEven, "2"),
        ("35612.25", -2, Rounding::HalfEven, "35600"),
        ("3567.812", 2, Rounding::HalfEven, "3567.81"),
        ("0.05", 0, Rounding::HalfEven, "0"),
        ("6", -1, Rounding::HalfEven, "10"),
        ("6", -40, Rounding::HalfEven, "0"),
        ("9.99", 1, Rounding::HalfUp, "10"),
        ("1.25", 5, Rounding::HalfEven, "1.25"),
        ("-7.9", 0, Rounding::Truncate, "-7"),
    ];
    for (value, precision, mode, expected) in cases {
        assert_eq!(d(value).round(precision, mode).text(), expected, "{value} at {precision} by {mode:?}");
    }
    assert!(d("5").round(-5000, Rounding::Ceiling).is_too_long());
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §6.2] a
/// decimal past `MAX_DIGITS` is known to be too long, as an operator
/// reports it.
#[test]
fn length_is_bounded() {
    let long = "9".repeat(MAX_DIGITS);
    assert!(!d(&long).is_too_long());
    assert!(d(&long).add(&d("1")).is_too_long());
    assert!(d(&format!("0.{long}1")).is_too_long());
}

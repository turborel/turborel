//! Tests of the value spaces: equality, order, canonical forms, lengths
//! and digits.

use std::cmp::Ordering;

use super::*;

/// An atomic value of `kind` written `lexical`.
fn of(kind: AtomicType, lexical: &str) -> Atomic {
    Atomic { kind, lexical: lexical.into() }
}

/// The order of the values of `kind` written `a` and `b`.
fn order(kind: AtomicType, a: &str, b: &str) -> Option<Ordering> {
    compare(&of(kind, a), &of(kind, b))
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §3.1.3.2] [Schema2 §2.3.1,
/// §3.2.3.2, §3.2.4.2, §3.2.7.2, §3.2.8.2, §3.3.13.2, §3.3.14.2] the
/// canonical representation of a value: a decimal's period, an integer's
/// none, the -0 of `xs:nonPositiveInteger`, a float's mantissa and
/// exponent, a date and time or time in UTC with Z; the string otherwise.
#[test]
fn canonical_representations() {
    let cases = [
        (AtomicType::Decimal, "12.50", "12.5"),
        (AtomicType::Decimal, "12", "12.0"),
        (AtomicType::Decimal, "-.5", "-0.5"),
        (AtomicType::Integer, "+012", "12"),
        (AtomicType::NonPositiveInteger, "0", "-0"),
        (AtomicType::NonPositiveInteger, "-3", "-3"),
        (AtomicType::Float, "100", "1.0E2"),
        (AtomicType::Double, "0", "0.0E0"),
        (AtomicType::Double, "-0", "-0.0E0"),
        (AtomicType::Double, "NaN", "NaN"),
        (AtomicType::Double, "1.25E-7", "1.25E-7"),
        (AtomicType::DateTime, "2020-01-02T03:04:05+05:00", "2020-01-01T22:04:05Z"),
        (AtomicType::DateTime, "2020-01-02T03:04:05", "2020-01-02T03:04:05"),
        (AtomicType::Time, "01:00:00+02:00", "23:00:00Z"),
        (AtomicType::Date, "2020-01-02+05:00", "2020-01-02+05:00"),
        (AtomicType::Base64Binary, "AQID", "AQID"),
        (AtomicType::Duration, "P1Y", "P1Y"),
        (AtomicType::String, " a ", " a "),
        (AtomicType::Decimal, "x", "x"),
    ];
    for (kind, text, expected) in cases {
        assert_eq!(canonical(&of(kind, text)), expected, "{text} as {}", kind.name());
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §3.1.3.45] [Schema2 §3.2.3,
/// §3.2.4, §4.2.1, §4.2.2] equality and the total orders: decimals and
/// integers one value space, ordered by sign and size; floats with -0
/// below 0 and NaN equal to itself above INF; values of different
/// primitive types, or of a type with no order, never ordered, equal
/// only when the same.
#[test]
fn equality_and_the_total_orders() {
    assert_eq!(compare(&of(AtomicType::Decimal, "1.50"), &of(AtomicType::Integer, "2")), Some(Ordering::Less));
    assert!(equal(&of(AtomicType::Decimal, "2.0"), &of(AtomicType::Byte, "2")));
    assert_eq!(order(AtomicType::Decimal, "-3", "2"), Some(Ordering::Less));
    assert_eq!(order(AtomicType::Decimal, "-3", "-2.5"), Some(Ordering::Less));
    assert_eq!(order(AtomicType::Decimal, "0.51", "0.6"), Some(Ordering::Less));
    assert_eq!(order(AtomicType::Decimal, "10", "9.99"), Some(Ordering::Greater));
    assert_eq!(order(AtomicType::Decimal, "-0", "0"), Some(Ordering::Equal));
    assert_eq!(compare(&of(AtomicType::Float, "1"), &of(AtomicType::Double, "1")), None);
    assert_eq!(order(AtomicType::Double, "NaN", "NaN"), Some(Ordering::Equal));
    assert_eq!(order(AtomicType::Double, "NaN", "INF"), Some(Ordering::Greater));
    assert_eq!(order(AtomicType::Double, "-INF", "NaN"), Some(Ordering::Less));
    assert_eq!(order(AtomicType::Double, "-0", "0"), Some(Ordering::Less));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §3.1.3.45] [Schema2 §4.2.1,
/// §4.2.2] the types with no order: a value equal to the same value
/// alone, of the same primitive type; never ordered.
#[test]
fn equality_without_order() {
    assert_eq!(order(AtomicType::String, "a", "a"), Some(Ordering::Equal));
    assert_eq!(order(AtomicType::String, "a", "b"), None);
    assert_eq!(compare(&of(AtomicType::String, "a"), &of(AtomicType::AnyUri, "a")), None);
    assert_eq!(compare(&of(AtomicType::String, "a"), &of(AtomicType::Token, "a")), Some(Ordering::Equal));
    assert_eq!(compare(&of(AtomicType::String, "a"), &of(AtomicType::UntypedAtomic, "a")), None);
    assert!(equal(&of(AtomicType::Boolean, "1"), &of(AtomicType::Boolean, "true")));
    assert!(equal(&of(AtomicType::HexBinary, "0fb7"), &of(AtomicType::HexBinary, "0FB7")));
    assert!(!equal(&of(AtomicType::HexBinary, "0FB7"), &of(AtomicType::Base64Binary, "D7c=")));
    assert_eq!(order(AtomicType::Decimal, "x", "1"), None);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §3.1.3.45] [Schema2 §3.2.7.3,
/// §3.2.8, §3.2.9, §3.2.10 to §3.2.14] the partial order of dates and
/// times: by instants in UTC when both or neither have a time zone; else
/// determinate only beyond fourteen hours, the examples of §3.2.7.3; the
/// Gregorian types by their starting instants.
#[test]
fn the_partial_order_of_dates_and_times() {
    let dt = |a, b| order(AtomicType::DateTime, a, b);
    assert_eq!(dt("2000-01-15T00:00:00", "2000-02-15T00:00:00"), Some(Ordering::Less));
    assert_eq!(dt("2000-01-15T12:00:00", "2000-01-16T12:00:00Z"), Some(Ordering::Less));
    assert_eq!(dt("2000-01-16T12:00:00Z", "2000-01-15T12:00:00"), Some(Ordering::Greater));
    assert_eq!(dt("2000-01-01T12:00:00", "1999-12-31T23:00:00Z"), None);
    assert_eq!(dt("2000-01-16T12:00:00", "2000-01-16T12:00:00Z"), None);
    assert_eq!(dt("2000-01-16T00:00:00", "2000-01-16T12:00:00Z"), None);
    assert_eq!(dt("2000-03-04T23:00:00+03:00", "2000-03-04T20:00:00Z"), Some(Ordering::Equal));
    assert_eq!(order(AtomicType::Time, "12:00:00+01:00", "11:00:00Z"), Some(Ordering::Equal));
    assert_eq!(order(AtomicType::Time, "23:00:00Z", "01:00:00+02:00"), Some(Ordering::Greater));
    assert_eq!(order(AtomicType::Date, "2000-01-02+05:00", "2000-01-02Z"), Some(Ordering::Less));
    assert_eq!(order(AtomicType::GYear, "2000", "2001"), Some(Ordering::Less));
    assert_eq!(order(AtomicType::GMonthDay, "--02-29", "--03-01"), Some(Ordering::Less));
    assert_eq!(order(AtomicType::GDay, "---05Z", "---05"), None);
    assert_eq!(compare(&of(AtomicType::Date, "2000-01-01"), &of(AtomicType::DateTime, "2000-01-01T00:00:00")), None);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §3.1.3.45] [Schema2 §3.2.6.2,
/// Appendix E] [XQueryFO §10.4.7] the partial order of durations, by
/// adding each to four dateTimes: the table of §3.2.6.2, a year against
/// days and months against days; less at two of them and greater at the
/// others, indeterminate; equal of the same months and seconds.
#[test]
fn the_partial_order_of_durations() {
    let d = |a, b| order(AtomicType::Duration, a, b);
    let table = [
        ("P1Y", "P364D", Some(Ordering::Greater)),
        ("P1Y", "P365D", None),
        ("P1Y", "P366D", None),
        ("P1Y", "P367D", Some(Ordering::Less)),
        ("P1M", "P27D", Some(Ordering::Greater)),
        ("P1M", "P28D", None),
        ("P1M", "P31D", None),
        ("P1M", "P32D", Some(Ordering::Less)),
        ("P5M", "P149D", Some(Ordering::Greater)),
        ("P5M", "P150D", None),
        ("P5M", "P153D", None),
        ("P5M", "P154D", Some(Ordering::Less)),
        ("PT24H", "P1D", Some(Ordering::Equal)),
        ("P1Y", "P12M", Some(Ordering::Equal)),
        ("-P1D", "PT1S", Some(Ordering::Less)),
        ("P1Y1D", "P1Y", Some(Ordering::Greater)),
        ("P1M", "P30DT12H", None),
    ];
    for (a, b, expected) in table {
        assert_eq!(d(a, b), expected, "{a} against {b}");
    }
    assert_eq!(compare(&of(AtomicType::YearMonthDuration, "P1Y"), &of(AtomicType::DayTimeDuration, "P400D")), Some(Ordering::Less));
    assert_eq!(order(AtomicType::DayTimeDuration, "PT1S", "PT0.5S"), Some(Ordering::Greater));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §9.5 GR8a, GR8b, GR8c] [Schema2
/// §4.3.1, §4.3.11.3, §4.3.12.3] the units the facets count: characters
/// of a string or URI, octets of binary data, none of a QName; the total
/// and fraction digits of a decimal.
#[test]
fn lengths_and_digits() {
    assert_eq!(length(&of(AtomicType::String, "abç")), Some(3));
    assert_eq!(length(&of(AtomicType::AnyUri, "http://x")), Some(8));
    assert_eq!(length(&of(AtomicType::HexBinary, "0FB7")), Some(2));
    assert_eq!(length(&of(AtomicType::Base64Binary, "AQID")), Some(3));
    assert_eq!(length(&of(AtomicType::QName, "a:b")), None);
    assert_eq!(length(&of(AtomicType::Decimal, "1")), None);
    assert_eq!(digits(&of(AtomicType::Decimal, "0.00123")), Some((5, 5)));
    assert_eq!(digits(&of(AtomicType::Decimal, "120")), Some((3, 0)));
    assert_eq!(digits(&of(AtomicType::Decimal, "-12.50")), Some((3, 1)));
    assert_eq!(digits(&of(AtomicType::Integer, "0")), Some((1, 0)));
    assert_eq!(digits(&of(AtomicType::String, "1")), None);
}

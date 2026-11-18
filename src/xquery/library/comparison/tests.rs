//! Tests of the value comparison of atomic values.

use super::*;

fn at(kind: AtomicType, text: &str) -> Atomic {
    Atomic::new(kind, text)
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQuery §3.5.1]
/// [XQueryFO §15.1.3, §15.1.6] `eq` of values of comparable types: numbers
/// promoted, strings by the collation, an untyped value as a string, a URI
/// as one, durations of any kind for equality, dates in the implicit time
/// zone; none of types `eq` is not defined for.
#[test]
fn equality_of_values() {
    let c = Collator::Codepoint;
    let eq = |a: &Atomic, b: &Atomic| value_equal(a, b, &c, 0).unwrap();
    assert_eq!(eq(&at(AtomicType::Integer, "1"), &at(AtomicType::Double, "1")), Some(true));
    assert_eq!(eq(&at(AtomicType::Double, "NaN"), &at(AtomicType::Double, "NaN")), Some(false));
    assert_eq!(eq(&at(AtomicType::String, "a"), &at(AtomicType::UntypedAtomic, "a")), Some(true));
    assert_eq!(eq(&at(AtomicType::AnyUri, "u"), &at(AtomicType::String, "u")), Some(true));
    assert_eq!(eq(&at(AtomicType::String, "1"), &at(AtomicType::Integer, "1")), None);
    assert_eq!(eq(&at(AtomicType::YearMonthDuration, "P12M"), &at(AtomicType::Duration, "P1Y")), Some(true));
    assert_eq!(eq(&at(AtomicType::Date, "2000-01-01Z"), &at(AtomicType::Date, "2000-01-01")), Some(true));
    assert_eq!(eq(&at(AtomicType::GDay, "---01"), &at(AtomicType::GDay, "---01Z")), Some(true));
    assert_eq!(eq(&at(AtomicType::Boolean, "true"), &at(AtomicType::Boolean, "1")), Some(true));
    assert_eq!(eq(&at(AtomicType::HexBinary, "0A"), &at(AtomicType::HexBinary, "0a")), Some(true));
    assert_eq!(eq(&at(AtomicType::Base64Binary, "AQ=="), &at(AtomicType::Base64Binary, "AQ==")), Some(true));
    assert_eq!(eq(&at(AtomicType::HexBinary, "01"), &at(AtomicType::Base64Binary, "AQ==")), None);
    assert_eq!(eq(&Atomic::qname(Some("u"), "a:x"), &Atomic::qname(Some("u"), "x")), Some(true));
    let insensitive = Collator::named("CASE_INSENSITIVE").unwrap();
    assert_eq!(value_equal(&at(AtomicType::String, "A"), &at(AtomicType::String, "a"), &insensitive, 0).unwrap(), Some(true));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQuery §3.5.1]
/// [XQueryFO §15.4.3] `lt` and `gt` of values of ordered types, false less
/// than true; none of NaN, of types of no order and of different types.
#[test]
fn order_of_values() {
    let c = Collator::Codepoint;
    let order = |a: &Atomic, b: &Atomic| value_order(a, b, &c, 0).unwrap();
    assert_eq!(order(&at(AtomicType::Integer, "2"), &at(AtomicType::Decimal, "1.5")), Some(Ordering::Greater));
    assert_eq!(order(&at(AtomicType::Double, "NaN"), &at(AtomicType::Double, "1")), None);
    assert_eq!(order(&at(AtomicType::String, "a"), &at(AtomicType::String, "b")), Some(Ordering::Less));
    assert_eq!(order(&at(AtomicType::Boolean, "false"), &at(AtomicType::Boolean, "true")), Some(Ordering::Less));
    assert_eq!(order(&at(AtomicType::Boolean, "true"), &at(AtomicType::Boolean, "false")), Some(Ordering::Greater));
    assert_eq!(order(&at(AtomicType::Boolean, "true"), &at(AtomicType::Boolean, "true")), Some(Ordering::Equal));
    assert_eq!(order(&at(AtomicType::DayTimeDuration, "P1D"), &at(AtomicType::DayTimeDuration, "PT1H")), Some(Ordering::Greater));
    assert_eq!(order(&at(AtomicType::Duration, "P1D"), &at(AtomicType::Duration, "PT1H")), None);
    assert_eq!(order(&at(AtomicType::Time, "01:00:00"), &at(AtomicType::Time, "02:00:00")), Some(Ordering::Less));
    assert_eq!(order(&at(AtomicType::GYear, "2000"), &at(AtomicType::GYear, "2001")), None);
    assert_eq!(order(&at(AtomicType::HexBinary, "01"), &at(AtomicType::HexBinary, "02")), None);
    assert_eq!(order(&at(AtomicType::String, "1"), &at(AtomicType::Integer, "1")), None);
}

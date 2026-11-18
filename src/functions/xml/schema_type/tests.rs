//! Tests of the mapping of SQL data types to XML Schema data types.

use super::*;
use crate::value::lob::LobKind;
use crate::value::{DOUBLE_PRECISION, REAL_PRECISION};

/// The type `t` maps to.
fn mapped(t: &DataType) -> SimpleType {
    xml_schema_restriction(t, BinaryEncoding::Hex).unwrap().unwrap()
}

/// The patterns of the type `t` maps to.
fn pattern(t: &DataType) -> String {
    let facets = mapped(t).facets().patterns.clone();
    facets.iter().flatten().map(|p| p.text.to_string()).collect::<Vec<_>>().join(" ")
}

/// Whether `literal` is a value of the type `t` maps to.
fn takes(t: &DataType, literal: &str) -> bool {
    mapped(t).validate(literal).is_ok()
}

/// An interval type of the fields `start` to `end`, of `leading` digits
/// and `fraction` digits of seconds.
fn interval_of(start: Field, end: Field, leading: u32, fraction: u32) -> DataType {
    DataType::Interval(Qualifier::new(start, end, leading, fraction))
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §9.5 GR8a to GR8f] strings,
/// binary strings and numbers map to built-in types restricted by
/// facets: CHARACTER's length, or maxLength of a length in octets;
/// CHARACTER VARYING's and CLOB's maxLength; a binary string's
/// `xs:hexBinary` of maxLength; NUMERIC's totalDigits and fractionDigits;
/// INTEGER's bounds, all the values its precision holds; an approximate
/// number's and BOOLEAN's built-in type; DECFLOAT none.
#[test]
fn strings_and_numbers_9_5() {
    let character = DataType::Character { length: 5, units: CharUnits::Characters };
    assert!(takes(&character, "abc  ") && !takes(&character, "abc"));
    assert_eq!(mapped(&character).name(), character.to_string());
    let octets = DataType::Character { length: 5, units: CharUnits::Octets };
    assert!(takes(&octets, "abc") && !takes(&octets, "abcdef"));
    let varying = DataType::Varying { length: 3, units: CharUnits::Characters };
    assert!(takes(&varying, "abc") && !takes(&varying, "abcd"));
    let clob = DataType::Lob { kind: LobKind::Character, length: 4, units: CharUnits::Characters };
    assert!(takes(&clob, "abcd") && !takes(&clob, "abcde"));
    for binary in [
        DataType::Binary { length: 2 },
        DataType::VarBinary { length: 2 },
        DataType::Lob { kind: LobKind::Binary, length: 2, units: CharUnits::Characters },
    ] {
        assert_eq!(mapped(&binary).atomic(), Some(AtomicType::HexBinary));
        assert!(takes(&binary, "0FB7") && !takes(&binary, "0FB7AA"));
    }
    let numeric = DataType::Exact { precision: 5, scale: 2, spelled: Spelled::Numeric };
    assert!(takes(&numeric, "123.45") && takes(&numeric, "1234.5") && !takes(&numeric, "1.234") && !takes(&numeric, "123456"));
    let integer = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };
    assert_eq!(mapped(&integer).atomic(), Some(AtomicType::Integer));
    assert!(takes(&integer, "-9999999999") && !takes(&integer, "10000000000") && !takes(&integer, "1.5"));
    let real = DataType::Approximate { precision: REAL_PRECISION, spelled: Spelled::Real };
    let double = DataType::Approximate { precision: DOUBLE_PRECISION, spelled: Spelled::DoublePrecision };
    assert_eq!((mapped(&real).atomic(), mapped(&double).atomic()), (Some(AtomicType::Float), Some(AtomicType::Double)));
    assert_eq!(mapped(&DataType::Boolean).atomic(), Some(AtomicType::Boolean));
    assert!(xml_schema_restriction(&DataType::DecFloat { precision: 16 }, BinaryEncoding::Hex).unwrap().is_none());
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §9.5 GR8g to GR8k] datetimes map
/// to `xs:date`, `xs:time` and `xs:dateTime` restricted by their
/// patterns: the date's; the time's, a fraction of S digits if S is more
/// than 0, TZ of a type WITH TIME ZONE, its sign read as `\+`.
#[test]
fn datetimes_9_5() {
    let date = "\\p{Nd}{4}-\\p{Nd}{2}-\\p{Nd}{2}";
    let time = "\\p{Nd}{2}:\\p{Nd}{2}:\\p{Nd}{2}";
    let tz = "(\\+|-)\\p{Nd}{2}:\\p{Nd}{2}";
    assert_eq!(pattern(&DataType::Date), date);
    assert_eq!(pattern(&DataType::Time { precision: 0, zoned: false }), time);
    assert_eq!(pattern(&DataType::Time { precision: 3, zoned: true }), format!("{time}.\\p{{Nd}}{{3}}{tz}"));
    assert_eq!(pattern(&DataType::Timestamp { precision: 6, zoned: false }), format!("{date}T{time}.\\p{{Nd}}{{6}}"));
    assert_eq!(pattern(&DataType::Timestamp { precision: 0, zoned: true }), format!("{date}T{time}{tz}"));
    assert!(takes(&DataType::Time { precision: 0, zoned: true }, "12:00:00+05:30"));
    assert!(!takes(&DataType::Time { precision: 0, zoned: true }, "12:00:00Z"));
    assert_eq!(mapped(&DataType::Timestamp { precision: 0, zoned: false }).atomic(), Some(AtomicType::DateTime));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §9.5 GR8l] an interval maps to
/// its duration type restricted by its pattern: `-?P`, T before its
/// first time field, its first field of its leading precision's digits,
/// the others of two, the seconds SECS of its fractional precision's.
#[test]
fn intervals_9_5() {
    let cases = [
        (interval_of(Field::Year, Field::Year, 4, 0), "-?P\\p{Nd}{4}Y"),
        (interval_of(Field::Year, Field::Month, 2, 0), "-?P\\p{Nd}{2}Y\\p{Nd}{2}M"),
        (interval_of(Field::Month, Field::Month, 3, 0), "-?P\\p{Nd}{3}M"),
        (interval_of(Field::Day, Field::Hour, 2, 0), "-?P\\p{Nd}{2}DT\\p{Nd}{2}H"),
        (interval_of(Field::Day, Field::Second, 2, 6), "-?P\\p{Nd}{2}DT\\p{Nd}{2}H\\p{Nd}{2}M\\p{Nd}{2}.\\p{Nd}{6}S"),
        (interval_of(Field::Hour, Field::Minute, 3, 0), "-?PT\\p{Nd}{3}H\\p{Nd}{2}M"),
        (interval_of(Field::Minute, Field::Second, 2, 1), "-?PT\\p{Nd}{2}M\\p{Nd}{2}.\\p{Nd}{1}S"),
        (interval_of(Field::Second, Field::Second, 4, 3), "-?PT\\p{Nd}{2}.\\p{Nd}{3}S"),
    ];
    for (t, expected) in cases {
        assert_eq!(pattern(&t), expected, "{t}");
    }
    let year_month = interval_of(Field::Year, Field::Month, 2, 0);
    assert_eq!(mapped(&year_month).atomic(), Some(AtomicType::YearMonthDuration));
    assert!(takes(&year_month, "-P01Y02M") && !takes(&year_month, "P1Y2M"));
    assert_eq!(mapped(&interval_of(Field::Day, Field::Day, 2, 0)).atomic(), Some(AtomicType::DayTimeDuration));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §9.5 GR8b ii] a binary string
/// maps to `xs:base64Binary` of BASE64, of its maxLength, as to
/// `xs:hexBinary` of HEX.
#[test]
fn binary_strings_in_base64_9_5() {
    let varbinary = DataType::VarBinary { length: 2 };
    let based = xml_schema_restriction(&varbinary, BinaryEncoding::Base64).unwrap().unwrap();
    assert_eq!(based.atomic(), Some(AtomicType::Base64Binary));
    assert!(based.validate("D7c=").is_ok() && based.validate("AAAA").is_err());
    assert_eq!(mapped(&varbinary).atomic(), Some(AtomicType::HexBinary));
}

//! Tests of `datetime()`: each of the five types a string is tried as,
//! and the strings and items it refuses.

use super::*;
use crate::json::path::eval::tests::{error, gives, run, state, value};

/// The datetime `datetime()` gives of the JSON string `text`.
fn read(text: &str) -> Datetime {
    let got = value("lax $.datetime()", &format!("\"{text}\""));
    let Value::Datetime(d) = got else { panic!("a datetime: {got:?}") };
    d
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C VII 1 c
/// i to v] [§5.3 SR33 to SR35] a string is read as a date, a time or a
/// timestamp, with time zone if it writes a `<time zone interval>`, of the
/// precision of its `<seconds fraction>`.
#[test]
fn datetime_reads_each_kind_of_string() {
    let date = read("2024-02-29");
    assert_eq!((date.moment, date.zoned, date.to_string()), (Moment::Date, false, "2024-02-29".to_owned()));
    let time = read("12:34:56.78");
    assert_eq!((time.moment, time.zoned, time.precision), (Moment::Time, false, 2_u8));
    assert_eq!(time.to_string(), "12:34:56.78");
    let zoned = read("12:34:56+05:30");
    assert_eq!((zoned.moment, zoned.zoned, zoned.precision, zoned.zone), (Moment::Time, true, 0_u8, 330_i16));
    let stamp = read("2024-01-02 03:04:05.123456");
    assert_eq!((stamp.moment, stamp.zoned, stamp.precision), (Moment::Timestamp, false, 6_u8));
    let zoned_stamp = read("2024-01-02 03:04:05-08:00");
    assert_eq!((zoned_stamp.moment, zoned_stamp.zoned, zoned_stamp.zone), (Moment::Timestamp, true, -480_i16));
    gives(
        "lax $.datetime().type()",
        r#"["2024-01-02", "01:02:03", "01:02:03+01:00", "2024-01-02 01:02:03", "2024-01-02 01:02:03+01:00"]"#,
        r#"["date", "time without time zone", "time with time zone", "timestamp without time zone", "timestamp with time zone"]"#,
    );
    gives("strict $[*].datetime().type()", r#"["2024-01-02"]"#, r#"["date"]"#);
    gives("lax $.none.datetime()", "{}", "[]");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C VII 1 a,
/// VII 1 c vi] what is no character string, or a string of no valid
/// datetime of the five types, is SQLSTATE 22031: a string with spaces, of
/// a day not in its month, of a timestamp whose time zone takes it past
/// 9999 in UTC; strict: an array, not unwrapped.
#[test]
fn datetime_of_what_is_no_datetime_string_is_an_error() {
    for context in ["1", "true", "null", "{}", "[[\"2024-01-01\"]]"] {
        assert_eq!(state("lax $.datetime()", context), "22031", "{context}");
    }
    let refused = error("strict $.datetime()", "[\"2024-01-01\"]");
    assert_eq!(refused.rule.as_deref(), Some("ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C VII 1 a"));
    for text in [" 2024-01-01", "2023-02-29", "24:00:00", "2024-01-02T03:04:05", "9999-12-31 23:00:00-05:00", "today", ""] {
        let e = error("lax $.datetime()", &format!("\"{text}\""));
        assert_eq!(e.sqlstate().as_str(), "22031", "{text}");
        assert_eq!(e.rule.as_deref(), Some("ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C VII 1 c vi"), "{text}");
    }
}

/// Verifies: [DESIGN] RULES.md D18: a string of more digits of the
/// second's fraction than 6, turborel's maximum, is SQLCODE -900.
#[test]
fn datetime_of_too_fine_a_fraction_is_beyond_limits() {
    assert_eq!(run("lax $.datetime()", "\"12:00:00.1234567\"").unwrap_err().sqlcode(), -900_i32);
    assert_eq!(SESSION_ZONE, 0_i16);
}

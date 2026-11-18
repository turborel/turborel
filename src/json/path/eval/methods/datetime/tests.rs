//! Tests of `datetime()`: each of the five types a string is tried as,
//! the strings and items it refuses, and strings read by a template.

use super::*;
use crate::json::path::eval::tests::{error, gives, run, state, value};
use crate::json::path::parse_path;
use crate::json::{Keys, parse};
use crate::value::datetime::Clocked;

/// The datetime `datetime()` gives of the JSON string `text`.
fn read(text: &str) -> Datetime {
    let got = value("lax $.datetime()", &format!("\"{text}\""));
    let Value::Datetime(d) = got else { panic!("a datetime: {got:?}") };
    d
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C VII 1 c
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

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C VII 1 a,
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
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C VII 1 a, VII 1 b]
/// [§9.51 GR5 to GR12] `datetime()` of a template reads each string by
/// §9.51 as a value of the template's implicit datetime data type, of an
/// array's elements in lax mode; what is no character string is SQLSTATE
/// 22031, and so is a string the template does not read, in place of
/// §9.51's exception.
#[test]
fn datetime_of_a_template_reads_by_it() {
    let Value::Datetime(date) = value("lax $.datetime(\"DD.MM.YYYY\")", "\"29.02.2024\"") else { panic!("a date") };
    assert_eq!((date.moment, date.zoned, date.to_string()), (Moment::Date, false, "2024-02-29".to_owned()));
    let stamp = value("lax $.datetime(\"YYYY-MM-DD HH24:MI:SS.FF3 TZH:TZM\")", "\"2024-01-02 03:04:05.678 -08:00\"");
    let Value::Datetime(stamp) = stamp else { panic!("a timestamp") };
    assert_eq!((stamp.moment, stamp.zoned, stamp.precision, stamp.zone), (Moment::Timestamp, true, 3_u8, -480_i16));
    gives(
        "lax $.datetime(\"HH12:MI A.M.\").type()",
        r#"["09:30 A.M.", "11:59 P.M."]"#,
        r#"["time without time zone", "time without time zone"]"#,
    );
    let not_a_string = error("lax $.datetime(\"YYYY-MM-DD\")", "1");
    assert_eq!(not_a_string.sqlstate().as_str(), "22031");
    assert_eq!(not_a_string.rule.as_deref(), Some("ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C VII 1 a"));
    for text in ["2024/01/02", "2023-02-29", "2024-01-02 ", "12:00"] {
        let unread = error("lax $.datetime(\"YYYY-MM-DD\")", &format!("\"{text}\""));
        assert_eq!(unread.sqlstate().as_str(), "22031", "{text}");
        assert_eq!(unread.rule.as_deref(), Some("ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C VII 1 b"), "{text}");
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C VII 1 b, VII 1 c v]
/// [ISO/IEC 9075-2:2023 §9.51 GR] a datetime without time zone is taken in
/// the SQL-session's displacement as the path was parsed, with a template
/// and without one alike, so the two compare equal. Regression: without a
/// template one was taken in UTC before the fix.
#[test]
fn datetime_takes_the_session_zone_with_a_template_and_without() {
    let now = Clocked::from(crate::value::datetime::Now { utc: 0, zone: 120 });
    let context = parse("\"2024-01-02 03:04:05\"", Keys::Any).unwrap();
    let mut zones = Vec::new();
    for text in ["lax $.datetime()", "lax $.datetime(\"YYYY-MM-DD HH24:MI:SS\")"] {
        let path = parse_path(text, now).unwrap();
        assert_eq!(path.zone, 120_i16);
        let got = path.evaluate(&context, &[]).unwrap();
        let [Item::Scalar(scalar)] = got.as_slice() else { panic!("{text} gave {got:?}") };
        let Value::Datetime(d) = scalar.value() else { panic!("{text} gave no datetime") };
        zones.push(d.zone);
    }
    assert_eq!(zones, vec![120_i16, 120_i16]);
    let equal = parse_path("lax $ ? (@.datetime() == @.datetime(\"YYYY-MM-DD HH24:MI:SS\"))", now).unwrap();
    assert_eq!(equal.evaluate(&context, &[]).unwrap().len(), 1);
}

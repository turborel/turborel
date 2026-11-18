//! Tests of the item methods that cast, lax and strict: each kind of item
//! each takes and refuses, and the exceptions of the casts.

use crate::json::Item;
use crate::json::path::eval::tests::{error, gives, json, state, value};
use crate::json::path::parse_path;
use crate::value::Value;
use crate::value::datetime::{Clocked, Datetime, Moment};

/// The value `path` gives of the item that is the date 2024-01-02.
fn of_a_date(path: &str) -> crate::error::Result<Vec<Item>> {
    let date = parse_path("lax \"2024-01-02\".date()", Clocked::default())?.evaluate(&json("0"), &[])?;
    parse_path(path, Clocked::default())?.evaluate(&json("0"), &[("d".to_owned(), date)])
}

/// The exact number `value` is, as its units and scale.
fn exact(value: &Value) -> (i128, u32) {
    let Value::Exact(d) = value else { panic!("an exact number: {value:?}") };
    (d.units, d.scale)
}

/// The datetime `value` is.
fn datetime(value: &Value) -> Datetime {
    let Value::Datetime(d) = value else { panic!("a datetime: {value:?}") };
    *d
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C IX,
/// XIII] `bigint()` and `integer()` cast a number or a character string
/// to BIGINT and INTEGER; lax: the base is unwrapped; one out of range is
/// 22003, a string that is no number 22018.
#[test]
fn bigint_and_integer_cast_to_their_types() {
    assert_eq!(exact(&value("lax $.bigint()", "\"123456789012\"")), (123_456_789_012_i128, 0_u32));
    assert_eq!(exact(&value("lax $.integer()", "42")), (42_i128, 0_u32));
    assert_eq!(exact(&value("strict $.bigint()", "-7")), (-7_i128, 0_u32));
    assert_eq!(exact(&value("lax $.integer()", "4e0")), (4_i128, 0_u32));
    gives("lax $.integer()", "[1, \"2\", 3]", "[1, 2, 3]");
    assert_eq!(state("lax $.integer()", "12345678901"), "22003");
    assert_eq!(state("lax $.bigint()", "1e30"), "22003");
    assert_eq!(state("lax $.bigint()", "\"x\""), "22018");
    assert_eq!(state("lax $.integer()", "\"\""), "22018");
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C IX 1 a,
/// XII 1 a, XIII 1 a, XIV 1 a] `bigint()`, `decimal()`, `integer()` and
/// `number()` of what is neither a number nor a character string are
/// SQLSTATE 22036: null, a truth value, an object, a datetime; strict: an
/// array, not unwrapped.
#[test]
fn numeric_casts_of_what_is_no_number_or_string_are_errors() {
    for (method, rule) in [("bigint", "IX"), ("decimal", "XII"), ("integer", "XIII"), ("number", "XIV")] {
        for context in ["null", "true", "{}", "[[1]]"] {
            assert_eq!(state(&format!("lax $.{method}()"), context), "22036", "{method} of {context}");
        }
        let e = error(&format!("strict $.{method}()"), "[1]");
        assert_eq!(e.sqlstate().as_str(), "22036", "{method}");
        assert_eq!(e.rule.as_deref(), Some(format!("ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C {rule} 1 a").as_str()));
        assert_eq!(of_a_date(&format!("lax $d.{method}()")).unwrap_err().sqlstate().as_str(), "22036", "{method}");
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C XII]
/// `decimal()` casts to DECIMAL of the precision and scale written, of
/// none 18 and 0: cut to the scale, as CAST cuts; past the precision
/// 22003.
#[test]
fn decimal_casts_to_its_precision_and_scale() {
    assert_eq!(exact(&value("lax $.decimal()", "\"12.5\"")), (12_i128, 0_u32));
    assert_eq!(exact(&value("lax $.decimal(5)", "12")), (12_i128, 0_u32));
    assert_eq!(exact(&value("lax $.decimal(5, 2)", "1.234")), (123_i128, 2_u32));
    assert_eq!(exact(&value("lax $.decimal(5, 2)", "\"-1.5\"")), (-150_i128, 2_u32));
    assert_eq!(exact(&value("strict $.decimal(18, 18)", "0.5")), (500_000_000_000_000_000_i128, 18_u32));
    assert_eq!(state("lax $.decimal(3, 1)", "100"), "22003");
    assert_eq!(state("lax $.decimal(2)", "\"100\""), "22003");
    assert_eq!(state("lax $.decimal()", "\"1,5\""), "22018");
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C XIV]
/// (IV186) `number()` keeps a number as it is, exact, approximate or
/// decimal floating-point; casts a character string as the `<signed
/// numeric literal>` it is, exact or approximate; a string that is none is
/// 22018.
#[test]
fn number_keeps_numbers_and_reads_strings() {
    assert_eq!(exact(&value("lax $.number()", "1.50")), (150_i128, 2_u32));
    assert!(matches!(value("lax $.number()", "2.5e0"), Value::Approx(x) if x.to_bits() == 2.5_f64.to_bits()));
    assert!(matches!(value("lax $.number()", "123456789012345678901234567890"), Value::DecFloat(_)));
    assert_eq!(exact(&value("lax $.number()", "\" -12.5 \"")), (-125_i128, 1_u32));
    assert!(matches!(value("lax $.number()", "\"1E2\""), Value::Approx(x) if x.to_bits() == 100.0_f64.to_bits()));
    gives("lax $.number()", "[1, \"2\"]", "[1, 2]");
    assert_eq!(state("lax $.number()", "\"one\""), "22018");
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C X]
/// `boolean()` casts a truth value or a character string to BOOLEAN; a
/// string that is no truth value is 22018; anything else, a number among
/// them, is 2202V.
#[test]
fn boolean_casts_truth_values_and_strings() {
    assert!(matches!(value("lax $.boolean()", "true"), Value::Bool(true)));
    assert!(matches!(value("lax $.boolean()", "\"false\""), Value::Bool(false)));
    assert!(matches!(value("strict $.boolean()", "\" TRUE \""), Value::Bool(true)));
    gives("lax $.boolean()", "[true, \"false\"]", "[true, false]");
    assert_eq!(state("lax $.boolean()", "\"yes\""), "22018");
    for context in ["1", "null", "{}", "[[true]]"] {
        assert_eq!(state("lax $.boolean()", context), "2202V", "{context}");
    }
    let e = error("strict $.boolean()", "[true]");
    assert_eq!(e.rule.as_deref(), Some("ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C X 1 a"));
    assert_eq!(of_a_date("lax $d.boolean()").unwrap_err().sqlstate().as_str(), "2202V");
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C XV]
/// `string()` casts a character string, a number or a truth value to
/// CHARACTER VARYING, each written as its literal; a datetime, null, an
/// array or an object is 2202X.
#[test]
fn string_casts_strings_numbers_and_truth_values() {
    gives("lax $.string()", "[\"a\", 1.50, true, 2e0]", r#"["a", "1.50", "TRUE", "2.0E0"]"#);
    assert!(matches!(value("strict $.string()", "-3"), Value::Char(s) if s == "-3"));
    for context in ["null", "{}", "[[\"a\"]]"] {
        assert_eq!(state("lax $.string()", context), "2202X", "{context}");
    }
    let e = error("strict $.string()", "[\"a\"]");
    assert_eq!(e.rule.as_deref(), Some("ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C XV 1 a"));
    assert_eq!(of_a_date("lax $d.string()").unwrap_err().sqlstate().as_str(), "2202X");
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C XI]
/// `date()` casts a character string to DATE; a string of no date is
/// 22018; anything else, a datetime among them, is 2202W.
#[test]
fn date_casts_strings() {
    let d = datetime(&value("lax $.date()", "\"2024-02-29\""));
    assert_eq!((d.moment, d.zoned), (Moment::Date, false));
    assert_eq!(d.to_string(), "2024-02-29");
    gives("lax $.date().type()", "[\"2024-01-01\", \"0001-12-31\"]", r#"["date", "date"]"#);
    assert_eq!(state("lax $.date()", "\"2023-02-29\""), "22018");
    for context in ["20240101", "true", "null", "[[\"2024-01-01\"]]"] {
        assert_eq!(state("lax $.date()", context), "2202W", "{context}");
    }
    let e = error("strict $.date()", "[\"2024-01-01\"]");
    assert_eq!(e.rule.as_deref(), Some("ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C XI 1 a"));
    assert_eq!(of_a_date("lax $d.date()").unwrap_err().sqlstate().as_str(), "2202W");
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C XVI,
/// XVII] `time()` and `time_tz()` cast a character string to TIME and TIME
/// WITH TIME ZONE, of the precision written, else 0, the fraction cut to
/// it; a string of no time is 22018.
#[test]
fn time_and_time_tz_cast_strings() {
    let t = datetime(&value("lax $.time()", "\"12:34:56.789\""));
    assert_eq!((t.moment, t.zoned, t.precision), (Moment::Time, false, 0_u8));
    assert_eq!(t.to_string(), "12:34:56");
    let t2 = datetime(&value("lax $.time(2)", "\"12:34:56.789\""));
    assert_eq!(t2.to_string(), "12:34:56.78");
    let t3 = datetime(&value("strict $.time_tz()", "\"12:00:00+05:30\""));
    assert_eq!((t3.moment, t3.zoned, t3.zone), (Moment::Time, true, 330_i16));
    let t4 = datetime(&value("lax $.time_tz(6)", "\"23:59:59.5-01:00\""));
    assert_eq!((t4.zoned, t4.precision, t4.zone), (true, 6_u8, -60_i16));
    gives("lax $.time().type()", "[\"01:02:03\"]", r#"["time without time zone"]"#);
    gives("lax $.time_tz().type()", "[\"01:02:03+00:00\"]", r#"["time with time zone"]"#);
    assert_eq!(state("lax $.time()", "\"25:00:00\""), "22018");
    assert_eq!(state("lax $.time_tz()", "\"noon\""), "22018");
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C XVIII,
/// XIX] `timestamp()` and `timestamp_tz()` cast a character string to
/// TIMESTAMP and TIMESTAMP WITH TIME ZONE, of the precision written, else
/// 6; a string of no timestamp is 22018.
#[test]
fn timestamp_and_timestamp_tz_cast_strings() {
    let t = datetime(&value("lax $.timestamp()", "\"2024-01-02 03:04:05.123456\""));
    assert_eq!((t.moment, t.zoned, t.precision), (Moment::Timestamp, false, 6_u8));
    assert_eq!(datetime(&value("lax $.timestamp(0)", "\"2024-01-02 03:04:05.9\"")).to_string(), "2024-01-02 03:04:05");
    let t2 = datetime(&value("strict $.timestamp_tz(3)", "\"2024-01-02 03:04:05.1234+02:00\""));
    assert_eq!((t2.moment, t2.zoned, t2.precision, t2.zone), (Moment::Timestamp, true, 3_u8, 120_i16));
    gives("lax $.timestamp_tz().type()", "[\"2024-01-02 03:04:05+00:00\"]", r#"["timestamp with time zone"]"#);
    assert_eq!(state("lax $.timestamp()", "\"2024-01-02\""), "22018");
    assert_eq!(state("lax $.timestamp_tz()", "\"2024-13-02 00:00:00+00:00\""), "22018");
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C XVI 1 a,
/// XVII 1 a, XVIII 1 a, XIX 1 a] `time()` and `timestamp()` of what is no
/// character string are 2202Y and 2202Z; `time_tz()` and `timestamp_tz()`
/// of what is neither a number nor a string too, and of a number, which
/// their rules take as printed, the cast's 22005.
#[test]
fn datetime_casts_of_what_they_do_not_take_are_errors() {
    for (method, refusal, rule) in
        [("time", "2202Y", "XVI"), ("time_tz", "2202Y", "XVII"), ("timestamp", "2202Z", "XVIII"), ("timestamp_tz", "2202Z", "XIX")]
    {
        for context in ["true", "null", "{}", "[[\"12:00:00\"]]"] {
            assert_eq!(state(&format!("lax $.{method}()"), context), refusal, "{method} of {context}");
        }
        let e = error(&format!("strict $.{method}()"), "[\"12:00:00\"]");
        assert_eq!(e.rule.as_deref(), Some(format!("ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C {rule} 1 a").as_str()));
        assert_eq!(of_a_date(&format!("lax $d.{method}()")).unwrap_err().sqlstate().as_str(), refusal, "{method}");
    }
    assert_eq!(state("lax $.time()", "1"), "2202Y");
    assert_eq!(state("lax $.timestamp()", "1"), "2202Z");
    assert_eq!(state("lax $.time_tz()", "1"), "22005");
    assert_eq!(state("lax $.timestamp_tz(2)", "1.5"), "22005");
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 A] the
/// casts are of each item, in lax mode of an array's elements; of no item,
/// no item.
#[test]
fn casts_apply_to_each_item() {
    gives("lax $.bigint()", "[]", "[]");
    gives("lax $.none.string()", "{}", "[]");
    gives("strict $[*].integer()", "[\"1\", 2]", "[1, 2]");
    gives("lax $.a.decimal(3, 1)", r#"{"a": [1.25, "2.35"]}"#, "[1.2, 2.3]");
}

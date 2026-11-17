//! Tests of the item methods, lax and strict.

use super::*;
use crate::json::Scalar;
use crate::json::path::eval::tests::{error, gives, json, state, value};
use crate::json::path::parse_path;

/// The names `type()` gives of the datetimes of the fields `moment`,
/// zoned or not.
fn datetime_names(moment: Moment) -> Vec<String> {
    [false, true]
        .into_iter()
        .map(|zoned| {
            let datetime = Datetime { moment, utc: 0, zone: 0, zoned, precision: 0 };
            let variables = vec![("d".to_owned(), vec![Item::Scalar(Scalar::new(Value::Datetime(datetime)).unwrap())])];
            let got = parse_path("strict $d.type()").unwrap().evaluate(&json("0"), &variables).unwrap();
            let [Item::Scalar(name)] = got.as_slice() else { panic!("{got:?}") };
            let Value::Char(name) = name.value() else { panic!("{name:?}") };
            name.clone()
        })
        .collect()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C I] the
/// name of each item's type: null, number of exact, approximate and
/// decimal floating-point alike, string, boolean, array, object; in lax
/// mode too the base is not unwrapped.
#[test]
fn type_names_each_items_type() {
    gives(
        "strict $[*].type()",
        r#"[null, 1, 1.5e0, 123456789012345678901234567890123456789012, "s", false, [], {}]"#,
        r#"["null", "number", "number", "number", "string", "boolean", "array", "object"]"#,
    );
    gives("lax $.type()", "[1, 2]", r#"["array"]"#);
    gives("lax $.none.type()", "{}", "[]");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C I 1 e
/// to i] a datetime's type is named by its fields and whether it is of a
/// type WITH TIME ZONE.
#[test]
fn type_names_each_datetime_type() {
    assert_eq!(datetime_names(Moment::Date), [DATE, DATE]);
    assert_eq!(datetime_names(Moment::Time), [TIME, TIME_TZ]);
    assert_eq!(datetime_names(Moment::Timestamp), [TIMESTAMP, TIMESTAMP_TZ]);
    assert_eq!(TIME_TZ, "time with time zone");
    assert_eq!(TIMESTAMP, "timestamp without time zone");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C II] the
/// size of an array is its number of elements, of any other item 1; lax:
/// not unwrapped; strict: an item that is no array is SQLSTATE 22039.
#[test]
fn size_counts_an_arrays_elements() {
    gives("lax $.size()", "[1, [2, 3], 4]", "[3]");
    gives("lax $.size()", "{\"a\": 1, \"b\": 2}", "[1]");
    gives("lax $.size()", "7", "[1]");
    gives("lax $.size()", "[]", "[0]");
    gives("lax $[*].size()", "[[1, 2], 3]", "[2, 1]");
    gives("strict $.size()", "[1, 2]", "[2]");
    assert_eq!(state("strict $.size()", "7"), "22039");
    assert_eq!(state("strict $[*].size()", "[[1], {}]"), "22039");
    assert_eq!(error("strict $.size()", "null").rule.as_deref(), Some("ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C II 1"));
    assert!(matches!(value("lax $.size()", "[1, 2]"), Value::Exact(d) if d.units == 2 && d.scale == 0));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C III]
/// `double()` casts a number or character string to DOUBLE PRECISION; lax:
/// the base is unwrapped; a cast's exception is the result.
#[test]
fn double_casts_to_double_precision() {
    assert!(matches!(value("lax $.double()", "1.5"), Value::Approx(x) if x.to_bits() == 1.5_f64.to_bits()));
    assert!(matches!(value("lax $.double()", "\"-2.5\""), Value::Approx(x) if x.to_bits() == (-2.5_f64).to_bits()));
    assert!(matches!(value("lax $.double()", "2e0"), Value::Approx(x) if x.to_bits() == 2.0_f64.to_bits()));
    assert!(matches!(value("lax $.double()", "123456789012345678901234567890123456789012"), Value::Approx(_)));
    gives("lax $.double()", "[1, \"2\"]", "[1, 2]");
    assert_eq!(state("lax $.double()", "\"abc\""), "22018");
    assert_eq!(state("strict $.double()", "[1]"), "22036");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C III 1 a]
/// `double()` of what is neither a number nor a character string is
/// SQLSTATE 22036: null, a truth value, an object, an array within one.
#[test]
fn double_of_what_is_no_number_or_string_is_an_error() {
    for context in ["null", "true", "{}", "[[1]]"] {
        assert_eq!(state("lax $.double()", context), "22036", "{context}");
    }
    let e = error("lax $.double()", "false");
    assert_eq!(e.rule.as_deref(), Some("ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C III 1 a"));
    let date = Datetime { moment: Moment::Date, utc: 0, zone: 0, zoned: false, precision: 0 };
    let variables = vec![("d".to_owned(), vec![Item::Scalar(Scalar::new(Value::Datetime(date)).unwrap())])];
    let refused = parse_path("lax $d.double()").unwrap().evaluate(&json("0"), &variables).unwrap_err();
    assert_eq!(refused.sqlstate().as_str(), "22036");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C IV, V,
/// VI] `ceiling()`, `floor()` and `abs()` of each number; lax: the base is
/// unwrapped.
#[test]
fn ceiling_floor_and_abs_apply_to_each_number() {
    gives("lax $.ceiling()", "[1.2, -1.2, 3]", "[2, -1, 3]");
    gives("lax $.floor()", "[1.8, -1.2, 3]", "[1, -2, 3]");
    gives("lax $.abs()", "[-1.5, 2, -3e0]", "[1.5, 2, 3]");
    gives("strict $[*].abs()", "[-1]", "[1]");
    gives("strict $.floor()", "2.5", "[2]");
    gives("lax $.none.abs()", "{}", "[]");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C IV 1 a,
/// V 1 a, VI 1 a] `ceiling()`, `floor()` or `abs()` of what is no number
/// is SQLSTATE 22036: a string, null; strict: an array, not unwrapped.
#[test]
fn numeric_methods_of_what_is_no_number_are_errors() {
    for (path, rule) in [("$.ceiling()", "GR11g ii 6 C IV 1 a"), ("$.floor()", "GR11g ii 6 C V 1 a"), ("$.abs()", "GR11g ii 6 C VI 1 a")] {
        assert_eq!(state(&format!("lax {path}"), "\"1\""), "22036", "{path}");
        assert_eq!(state(&format!("lax {path}"), "[1, null]"), "22036", "{path}");
        let e = error(&format!("strict {path}"), "[1]");
        assert_eq!(e.sqlstate().as_str(), "22036", "{path}");
        assert_eq!(e.rule.as_deref(), Some(format!("ISO/IEC 9075-2:2023 §9.46 {rule}").as_str()));
    }
}

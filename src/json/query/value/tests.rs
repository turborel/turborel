//! Tests of JSON_VALUE and of casting an SQL/JSON sequence to an SQL
//! type.

use super::*;
use crate::error::Code;
use crate::json::query::tests::{integer, on, text, typed};
use crate::json::query::{IMPLICIT_RETURNING, is_exception};
use crate::json::{Keys, parse};
use crate::value::lob::LobKind;
use crate::value::{CharUnits, Spelled, compare};

/// JSON_VALUE of `context` and `path`, `passing` as `on` takes it, of the
/// other operands as given.
fn value_of(context: &Value, path: &str, returning: &DataType, on_empty: &ValueBehavior, on_error: &ValueBehavior) -> Result<Warned> {
    on(context, path, &[], |common| json_value(common, returning, on_empty, on_error))
}

/// JSON_VALUE of the JSON text `context` and `path`, RETURNING `returning`,
/// ERROR ON EMPTY and ERROR ON ERROR.
fn strict(context: &str, path: &str, returning: &DataType) -> Result<Value> {
    value_of(&text(context), path, returning, &ValueBehavior::Error, &ValueBehavior::Error).map(|(value, _)| value)
}

/// The SQLSTATE of the exception `strict` raises.
fn state(context: &str, path: &str, returning: &DataType) -> String {
    strict(context, path, returning).unwrap_err().sqlstate().as_str().to_owned()
}

/// That `got` equals `expected`, as SQL values compare.
fn equal(got: &Value, expected: &Value) {
    assert_eq!(compare(got, expected), Some(std::cmp::Ordering::Equal), "{got:?}, not {expected:?}");
}

fn decimal() -> DataType {
    DataType::Exact { precision: 5, scale: 2, spelled: Spelled::Decimal }
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.28 GR5, §9.48 GR4b iii 2]
/// the one scalar the path gives cast to the returned type: a string to a
/// character string, a number, a truth value or a datetime type; a
/// number to a character string or a number; a truth value to BOOLEAN; the
/// implicit type a character string type.
#[test]
fn the_one_scalar_is_cast_to_the_returned_type() {
    let context = r#"{"s": "abc", "n": 1.5, "i": "12", "b": true, "d": "2020-01-02", "t": "10:20:30", "z": "2020-01-02 10:20:30"}"#;
    equal(&strict(context, "lax $.s", &IMPLICIT_RETURNING).unwrap(), &text("abc"));
    equal(&strict(context, "lax $.n", &decimal()).unwrap(), &typed("1.50", &decimal()));
    equal(&strict(context, "lax $.n", &DataType::varying(9)).unwrap(), &text("1.5"));
    equal(&strict(context, "lax $.i", &integer()).unwrap(), &typed("12", &integer()));
    equal(&strict(context, "lax $.b", &DataType::Boolean).unwrap(), &Value::Bool(true));
    equal(&strict(context, "lax $.b", &DataType::varying(9)).unwrap(), &text("TRUE"));
    let timestamp = DataType::Timestamp { precision: 0, zoned: false };
    let time = DataType::Time { precision: 0, zoned: false };
    equal(&strict(context, "lax $.d", &DataType::Date).unwrap(), &typed("2020-01-02", &DataType::Date));
    equal(&strict(context, "lax $.t", &time).unwrap(), &typed("10:20:30", &time));
    equal(&strict(context, "lax $.z", &timestamp).unwrap(), &typed("2020-01-02 10:20:30", &timestamp));
    let clob = DataType::Lob { kind: LobKind::Character, length: 9_u64, units: CharUnits::Characters };
    assert!(matches!(strict(context, "lax $.s", &clob).unwrap(), Value::Lob(_)));
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.48 GR4b iii 2, §6.13 SR7]
/// a datetime item, as a PASSING argument gives one, is cast to a
/// character string and to the datetime types the table allows.
#[test]
fn a_datetime_item_is_cast_as_the_table_allows() {
    let timestamp = DataType::Timestamp { precision: 0, zoned: false };
    let time = DataType::Time { precision: 0, zoned: false };
    let moments = [typed("2020-01-02", &DataType::Date), typed("10:20:30", &time), typed("2020-01-02 10:20:30", &timestamp)];
    let allowed: [&[DataType]; 3] =
        [&[DataType::Date, timestamp.clone()], &[time.clone(), timestamp.clone()], &[DataType::Date, time.clone(), timestamp.clone()]];
    let refused: [&[DataType]; 3] = [&[time.clone(), integer(), DataType::Boolean], &[DataType::Date], &[integer()]];
    for ((moment, to_types), refusals) in moments.iter().zip(allowed).zip(refused) {
        let passing = [("d", moment.clone(), false)];
        let cast = |to: &DataType| {
            on(&text("0"), "lax $d", &passing, |common| json_value(common, to, &ValueBehavior::Error, &ValueBehavior::Error))
        };
        assert!(matches!(cast(&DataType::varying(30)).unwrap().0, Value::Char(_)));
        for to in to_types {
            assert!(matches!(cast(to).unwrap().0, Value::Datetime(_)), "{moment:?} to {to}");
        }
        for to in refusals {
            assert_eq!(cast(to).unwrap_err().sqlstate().as_str(), "2203G", "{moment:?} to {to}");
        }
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.48 GR4a, GR4b i, ii, iii
/// 1] more than one item is SQLSTATE 22034; an array or an object 2203F;
/// the SQL/JSON null the null value; an item of a type that cannot be cast
/// to the returned type 2203G; a cast that fails its own exception.
#[test]
fn what_cannot_be_cast_is_an_exception() {
    let context = r#"{"a": [1, 2], "o": {}, "n": null, "b": false, "x": 7, "s": "abc"}"#;
    assert_eq!(state(context, "lax $.a[*]", &integer()), "22034");
    assert_eq!(state(context, "strict $.a", &integer()), "2203F");
    assert_eq!(state(context, "lax $.o", &DataType::varying(9)), "2203F");
    assert!(strict(context, "lax $.n", &integer()).unwrap().is_null());
    assert_eq!(state(context, "lax $.b", &integer()), "2203G");
    assert_eq!(state(context, "lax $.x", &DataType::Boolean), "2203G");
    assert_eq!(state(context, "lax $.x", &DataType::Date), "2203G");
    assert_eq!(state(context, "lax $.s", &integer()), "22018");
    let e = strict(context, "lax $.b", &integer()).unwrap_err();
    assert_eq!(e.rule.as_deref(), Some("ISO/IEC 9075-2:2023 §9.48 GR4b iii 1"));
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.28 SR4, §9.48 GR4c] no
/// item: NULL ON EMPTY gives the null value, DEFAULT its value cast,
/// ERROR SQLSTATE 22035, which ON ERROR replaces, so that with NULL ON
/// ERROR it is the null value; a DEFAULT that fails to cast is an
/// exception ON ERROR replaces too.
#[test]
fn no_item_is_as_on_empty_says() {
    let empty = |on_empty: &ValueBehavior, on_error: &ValueBehavior| value_of(&text("{}"), "lax $.a", &integer(), on_empty, on_error);
    assert!(empty(&ValueBehavior::Null, &ValueBehavior::Error).unwrap().0.is_null());
    equal(&empty(&ValueBehavior::Default(text(" 5 ")), &ValueBehavior::Error).unwrap().0, &typed("5", &integer()));
    let e = empty(&ValueBehavior::Error, &ValueBehavior::Error).unwrap_err();
    assert_eq!(e.sqlstate().as_str(), "22035");
    assert_eq!(e.rule.as_deref(), Some("ISO/IEC 9075-2:2023 §9.48 GR4c i"));
    assert!(empty(&ValueBehavior::Error, &ValueBehavior::Null).unwrap().0.is_null());
    let bad = ValueBehavior::Default(text("x"));
    assert_eq!(empty(&bad, &ValueBehavior::Error).unwrap_err().sqlstate().as_str(), "22018");
    equal(&empty(&bad, &ValueBehavior::Default(text("6"))).unwrap().0, &typed("6", &integer()));
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.28 SR5, §9.48 GR5] an
/// exception, of the path, of the context item or of the rules, is
/// replaced as ON ERROR says: NULL, the default, by the null value;
/// DEFAULT by its value cast, whose own exception is raised; ERROR raises
/// it.
#[test]
fn an_exception_is_replaced_as_on_error_says() {
    for (context, path) in [("{}", "strict $.a"), ("{", "lax $"), (r#"{"a": [1, 2]}"#, "lax $.a[*]")] {
        let with = |on_error: &ValueBehavior| value_of(&text(context), path, &integer(), &ValueBehavior::Null, on_error);
        assert!(with(&ValueBehavior::Null).unwrap().0.is_null(), "{path}");
        equal(&with(&ValueBehavior::Default(text("9"))).unwrap().0, &typed("9", &integer()));
        assert!(with(&ValueBehavior::Default(Value::Null)).unwrap().0.is_null());
        assert_eq!(with(&ValueBehavior::Default(text("x"))).unwrap_err().sqlstate().as_str(), "22018");
        assert!(is_exception(&with(&ValueBehavior::Error).unwrap_err()));
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.28 GR2, GR6] of a null
/// context item, the null value, whatever the path and behaviours; the
/// warning of a string cut to the returned type is kept.
#[test]
fn a_null_context_item_is_the_null_value_and_warnings_are_kept() {
    let got = value_of(&Value::Null, "strict $.a", &integer(), &ValueBehavior::Error, &ValueBehavior::Error).unwrap();
    assert!(got.0.is_null() && got.1.is_none());
    let (cut, warning) =
        value_of(&text(r#""abcdef""#), "lax $", &DataType::varying(3), &ValueBehavior::Error, &ValueBehavior::Error).unwrap();
    equal(&cut, &text("abc"));
    assert_eq!(warning.map(|w| w.as_str().to_owned()).as_deref(), Some("01004"));
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.28 SR2] the returned type
/// is a predefined character string, numeric, Boolean or datetime type:
/// another is refused with SQLCODE -103, of a null context item too; and
/// an SQL/JSON sequence is cast to no other.
#[test]
fn the_returned_type_is_one_json_value_returns() {
    let blob = DataType::Lob { kind: LobKind::Binary, length: 9_u64, units: CharUnits::Characters };
    for to in [DataType::Binary { length: 1 }, DataType::VarBinary { length: 1 }, blob] {
        let e = value_of(&Value::Null, "lax $", &to, &ValueBehavior::Null, &ValueBehavior::Null).unwrap_err();
        assert_eq!(e.sqlcode(), -103_i32, "{to}");
        assert_eq!(e.rule.as_deref(), Some("ISO/IEC 9075-2:2023 §6.28 SR2"));
        assert_eq!(cast_sequence(Ok(Vec::new()), &ValueBehavior::Null, &ValueBehavior::Null, &to).unwrap_err().sqlcode(), -103_i32);
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.48 GR3, GR5] STATUS IN an
/// exception condition is replaced as ON ERROR says; an error not one, as
/// a Syntax Rule's, is raised whatever it says.
#[test]
fn the_status_in_is_replaced_or_raised() {
    let exception = Err(SqlError::new(Code::Data, "[DESIGN] an exception"));
    assert!(cast_sequence(exception, &ValueBehavior::Error, &ValueBehavior::Null, &integer()).unwrap().0.is_null());
    let rule = Err(SqlError::new(Code::SyntaxRule, "[DESIGN] a rule"));
    assert_eq!(cast_sequence(rule, &ValueBehavior::Null, &ValueBehavior::Null, &integer()).unwrap_err().sqlcode(), -103_i32);
    let one = vec![parse("3", Keys::Any).unwrap()];
    equal(&cast_sequence(Ok(one), &ValueBehavior::Error, &ValueBehavior::Error, &integer()).unwrap().0, &typed("3", &integer()));
    let e = on(&text("1"), "lax $x", &[], |common| json_value(common, &integer(), &ValueBehavior::Null, &ValueBehavior::Null)).unwrap_err();
    assert_eq!(e.sqlcode(), -103_i32);
}

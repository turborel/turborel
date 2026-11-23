//! Tests of the functions that let a DEFAULT of JSON_VALUE be evaluated
//! only when taken: which behavior is taken, and whether a cast fails.

use super::*;
use crate::error::Code;
use crate::functions::json::{JsonOperation, ScalarFunction};
use crate::json::parse_path;
use crate::value::INTEGER_PRECISION;
use crate::value::datetime::Clocked;

/// INTEGER.
fn integer() -> DataType {
    DataType::Exact { precision: INTEGER_PRECISION, scale: 0, spelled: Spelled::Integer }
}

/// Which behavior JSON_VALUE of `path`, RETURNING INTEGER, of the slots
/// `on_empty` and `on_error`, takes of the JSON text `context`.
fn taken(path: &str, context: &str, on_empty: ValueSlot, on_error: ValueSlot) -> Result<Value> {
    let operation = JsonOperation::Value { returning: integer(), on_empty, on_error };
    let call = JsonCall::new(path, parse_path(path, Clocked::default())?, Vec::new(), operation);
    JsonValueTaken::new(call).call(&[Value::Char(context.into())])
}

/// The code `n` of a behavior taken, as a value shows it.
fn code(n: i128) -> String {
    Value::Exact(Decimal::new(n, 0)).to_string()
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.48 GR4b, GR4c iii, GR5d] the
/// behavior taken: the value of a sequence of one item cast, of the null
/// context item, and of a NULL behavior; DEFAULT ON EMPTY of an empty
/// sequence; DEFAULT ON ERROR of an exception condition, the path's, the
/// context item's parse or the cast's; ERROR ON ERROR raises it. MC/DC of
/// each slot DEFAULT or not.
#[test]
fn the_behavior_taken() {
    use ValueSlot::{Default, Error, Null};
    assert_eq!(taken("lax $.a", r#"{"a": 1}"#, Default, Default).unwrap().to_string(), code(TAKEN_VALUE));
    assert_eq!(taken("lax $.b", r#"{"a": 1}"#, Default, Default).unwrap().to_string(), code(TAKEN_EMPTY));
    assert_eq!(taken("lax $.b", r#"{"a": 1}"#, Null, Default).unwrap().to_string(), code(TAKEN_VALUE));
    assert_eq!(taken("strict $.b", r#"{"a": 1}"#, Default, Default).unwrap().to_string(), code(TAKEN_ERROR));
    assert_eq!(taken("lax $.a", r#"{"a": "x"}"#, Null, Default).unwrap().to_string(), code(TAKEN_ERROR));
    assert_eq!(taken("lax $.a", r#"{"a": "x"}"#, Null, Null).unwrap().to_string(), code(TAKEN_VALUE));
    assert_eq!(taken("lax $.a", "{", Default, Default).unwrap().to_string(), code(TAKEN_ERROR));
    assert_eq!(taken("lax $.a", r#"{"a": "x"}"#, Default, Error).unwrap_err().sqlstate().as_str(), "22018");
    let operation = JsonOperation::Value { returning: integer(), on_empty: Default, on_error: Default };
    let call = JsonCall::new("lax $", parse_path("lax $", Clocked::default()).unwrap(), Vec::new(), operation);
    let f = JsonValueTaken::new(call);
    assert_eq!(f.call(&[Value::Null]).unwrap().to_string(), code(TAKEN_VALUE));
    assert_eq!(f.call(&[]).unwrap_err().code, Code::Api);
    assert!(f.name().ends_with(" TAKEN"));
    assert_eq!(f.return_type(&[]).unwrap(), SMALLINT);
    assert_eq!(f.parameter_type(1, &[]), None);
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.48 GR4c iii] whether a DEFAULT ON
/// EMPTY's cast raises an exception condition: True of a string no
/// literal of the type, or a number out of its range; False of one cast;
/// of no argument, SQLCODE -910.
#[test]
fn whether_a_cast_fails() {
    let fails = JsonCastFails::new(integer());
    assert!(matches!(fails.call(&[Value::Char("x".into())]).unwrap(), Value::Bool(true)));
    assert!(matches!(fails.call(&[Value::Char("99999999999".into())]).unwrap(), Value::Bool(true)));
    assert!(matches!(fails.call(&[Value::Char("12".into())]).unwrap(), Value::Bool(false)));
    assert!(matches!(fails.call(&[Value::Null]).unwrap(), Value::Bool(false)));
    assert_eq!(fails.call(&[]).unwrap_err().code, Code::Api);
    assert_eq!(fails.return_type(&[]).unwrap(), DataType::Boolean);
    assert!(fails.name().starts_with("JSON_VALUE DEFAULT CAST FAILS"));
}

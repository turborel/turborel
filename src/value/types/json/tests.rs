//! Tests of the JSON type's kind: its name and text, comparison of its
//! values, CAST and assignment to it, and its keys read back.

use super::*;
use crate::error::SqlState;
use crate::value::lob::Lob;
use crate::value::{Binary, Decimal};

fn parsed(text: &str) -> Value {
    Value::Json(Json::parse(text, Keys::Any).unwrap())
}

fn state<T>(r: Result<T>) -> Option<SqlState> {
    r.err().map(|e| e.sqlstate())
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.1, §4.8.1] the type's name is JSON,
/// its descriptor that name alone; a value's text is its JSON text and
/// its literal JSON ( ) of it, quotes doubled; it has no position.
#[test]
fn json_names_and_text() {
    let v = parsed(r#"{"a":"it's"}"#);
    assert_eq!(JSON.name(), JSON_NAME);
    assert_eq!(JSON.written(&DataType::Json), "JSON");
    assert_eq!(JSON.descriptor(&DataType::Json).name, "JSON");
    assert_eq!(JSON.text(&v), r#"{"a":"it's"}"#);
    assert_eq!(JSON.literal(&v), r#"JSON('{"a":"it''s"}')"#);
    assert_eq!(JSON.position(&v), None);
    assert_eq!(JSON.text(&Value::Null), "");
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.8.2, §8.2 GR10] SQL/JSON values
/// compare by their comparison keys, equal values alike; a value of
/// another type has no comparison with one.
#[test]
fn json_compares_by_keys() {
    let (one, two) = (parsed("1"), parsed("2"));
    assert!(JSON.comparable(&DataType::Json));
    assert!(!JSON.comparable(&DataType::Boolean));
    assert_eq!(JSON.compare(&one, &two), Some(Ordering::Less));
    assert_eq!(JSON.compare(&one, &parsed("1")), Some(Ordering::Equal));
    assert_eq!(JSON.compare(&one, &Value::Bool(true)), None);
    assert_eq!(JSON.bucket(&one), JSON.bucket(&parsed("1")));
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.13 GR22, §9.2] CAST of the null value
/// to JSON is null; a value of JSON and, as an extension, a character
/// string or character large object of JSON text are cast and assigned;
/// a number, a boolean or a binary large object is neither (SQLSTATE
/// 22005), and text that is no JSON text is SQLSTATE 22032.
#[test]
fn json_cast_and_assignment() {
    let t = DataType::Json;
    assert!(JSON.cast(Value::Null, &t).unwrap().is_null());
    let cast = JSON.cast(Value::Char("[1, 2]".into()), &t).unwrap();
    assert_eq!(JSON.text(&cast), "[1,2]");
    let lob = JSON.assign(Value::Lob(Lob::of_text("{\"k\": true}")), &t, "test").unwrap();
    assert_eq!(JSON.text(&lob), r#"{"k":true}"#);
    assert_eq!(JSON.text(&JSON.cast(parsed("null"), &t).unwrap()), "null");
    assert_eq!(state(JSON.cast(Value::Exact(Decimal::new(1, 0)), &t)), Some(SqlState::ERROR_IN_ASSIGNMENT));
    assert_eq!(state(JSON.assign(Value::Bool(true), &t, "test")), Some(SqlState::ERROR_IN_ASSIGNMENT));
    assert_eq!(state(JSON.assign(Value::Lob(Lob::of_octets(b"{}")), &t, "test")), Some(SqlState::ERROR_IN_ASSIGNMENT));
    assert_eq!(state(JSON.cast(Value::Binary(Binary::default()), &t)), Some(SqlState::ERROR_IN_ASSIGNMENT));
    assert_eq!(state(JSON.cast(Value::Char("{".into()), &t)), Some(SqlState::INVALID_JSON_TEXT));
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.8.2] a value's key reads back as an
/// equal value, and is its own anchor; a value of another type is not
/// stored or keyed as JSON (SQLCODE -910) and has no anchor.
#[test]
fn json_keys_read_back() {
    let (t, v) = (DataType::Json, parsed(r#"{"b":[1,"x"],"a":null}"#));
    let mut key = Vec::new();
    JSON.encode_key(&v, &t, &mut key).unwrap();
    let back = JSON.decode_key(&mut key.as_slice(), &t).unwrap();
    assert_eq!(JSON.compare(&back, &v), Some(Ordering::Equal));
    let (anchor, exact) = JSON.anchor(&v, &t).unwrap();
    assert!(exact && JSON.compare(&anchor, &v) == Some(Ordering::Equal));
    assert!(JSON.anchor(&Value::Bool(true), &t).is_none());
    assert_eq!(JSON.encode_record(&Value::Bool(true), &t).err().map(|e| e.code), Some(Code::Api));
    assert_eq!(JSON.encode_key(&Value::Bool(true), &t, &mut Vec::new()).err().map(|e| e.code), Some(Code::Api));
}

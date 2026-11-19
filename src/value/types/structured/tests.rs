//! Tests of the structured types' kind: names and supertypes, comparison
//! with and without keys, CAST and assignment, and the stored form.

use super::*;
use crate::error::SqlState;
use crate::value::Decimal;

fn number(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

fn point(x: i128, key: Option<Value>) -> Value {
    Value::Structured(Arc::new(Structured { of: Arc::new(UdtName::new("S", "POINT")), attributes: Arc::from(vec![number(x)]), key }))
}

fn point_type() -> DataType {
    DataType::structured(UdtName::new("S", "POINT"), vec![UdtName::new("S", "SHAPE")])
}

fn code<T>(r: Result<T>) -> Option<Code> {
    r.err().map(|e| e.code)
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.9.3.1, §4.9.1] the maximal supertype
/// of a family is its last proper supertype, or the type itself if it has
/// none; the kind's name is USER-DEFINED and a value's text names its
/// type and attributes; it has no position.
#[test]
fn structured_names_and_text() {
    let shape = UdtName::new("S", "SHAPE");
    assert_eq!(point_type().structured_type().unwrap().maximal(), &shape);
    let root = DataType::structured(shape.clone(), Vec::new());
    assert_eq!(root.structured_type().unwrap().maximal(), &shape);
    assert_eq!(STRUCTURED.name(), USER_DEFINED_NAME);
    assert_eq!(STRUCTURED.text(&point(3, None)), "S.POINT(3)");
    assert_eq!(STRUCTURED.text(&Value::Null), "NULL");
    assert_eq!(STRUCTURED.position(&point(3, None)), None);
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.9.5, §9.10 GR2e] values with keys
/// compare, hash and normalize by their keys; values without, by their
/// types and attributes; a value of another type has no comparison.
#[test]
fn structured_compares_by_keys() {
    let (low, high) = (point(9, Some(number(1))), point(1, Some(number(2))));
    assert!(STRUCTURED.comparable(&point_type()));
    assert!(!STRUCTURED.comparable(&DataType::Boolean));
    assert_eq!(STRUCTURED.compare(&low, &high), Some(Ordering::Less));
    assert_eq!(STRUCTURED.compare(&point(9, None), &point(1, None)), Some(Ordering::Greater));
    assert_eq!(STRUCTURED.compare(&low, &number(1)), None);
    assert_eq!(STRUCTURED.bucket(&low), crate::value::bucket(&number(1)));
    assert_eq!(STRUCTURED.bucket(&point(5, None)), STRUCTURED.bucket(&point(5, None)));
    assert_eq!(STRUCTURED.bucket(&number(1)), None);
    assert_eq!(low.norm(), number(1).norm());
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.2 GR3b xxiv, §6.13 GR2, GR3a] the
/// null value is assigned and cast to a structured type; a value of
/// another type is neither (SQLSTATE 22005); a value assigned drops its
/// key and keeps its most specific type.
#[test]
fn structured_cast_and_assignment() {
    let t = point_type();
    assert!(STRUCTURED.assign(Value::Null, &t, "test").unwrap().is_null());
    assert!(STRUCTURED.cast(Value::Null, &t).unwrap().is_null());
    assert_eq!(STRUCTURED.assign(number(1), &t, "test").unwrap_err().sqlstate(), SqlState::ERROR_IN_ASSIGNMENT);
    assert_eq!(STRUCTURED.cast(number(1), &t).unwrap_err().sqlstate(), SqlState::ERROR_IN_ASSIGNMENT);
    let assigned = STRUCTURED.cast(point(4, Some(number(7))), &t).unwrap();
    assert!(assigned.structured().unwrap().key.is_none());
    assert_eq!(STRUCTURED.text(&assigned), "S.POINT(4)");
}

/// Verifies: [DESIGN] RULES.md D81: a value written with its key reads
/// back with it; a value of another type is not stored, written as an
/// empty one, and a key flag other than 0 or 1 is SQLCODE -916; no value
/// is in a key, and one is its own anchor.
#[test]
fn structured_stored_form() {
    let (t, v) = (point_type(), point(2, Some(number(5))));
    let mut out = Vec::new();
    STRUCTURED.put_value(&v, &mut out);
    let back = STRUCTURED.value(&mut Reader::new(&out, "value")).unwrap();
    assert_eq!(back.structured().unwrap().key.as_ref().map(Value::norm), Some(number(5).norm()));
    let mut empty = Vec::new();
    STRUCTURED.put_value(&number(1), &mut empty);
    let read = STRUCTURED.value(&mut Reader::new(&empty, "value")).unwrap();
    assert_eq!(STRUCTURED.text(&read), ".()");
    let flag = empty.len().checked_sub(1).unwrap();
    empty[flag] = 2;
    assert_eq!(code(STRUCTURED.value(&mut Reader::new(&empty, "value"))), Some(Code::Corrupt));
    assert_eq!(code(STRUCTURED.encode_record(&number(1), &t)), Some(Code::Api));
    assert_eq!(code(STRUCTURED.encode_key(&v, &t, &mut Vec::new())), Some(Code::Api));
    assert_eq!(code(STRUCTURED.decode_key(&mut &[][..], &t)), Some(Code::Corrupt));
    assert_eq!(STRUCTURED.anchor(&v, &t).map(|(_, exact)| exact), Some(true));
    assert!(STRUCTURED.anchor(&number(1), &t).is_none());
}

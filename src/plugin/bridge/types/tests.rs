//! Unit tests: a plug-in's type working with values it keeps, with no
//! process running: compared, hashed, keyed, stored, shown and cast by
//! the forms each value holds (RULES.md D223, R-TEST-1, R-TEST-2).

use super::*;
use std::cmp::Ordering;

use crate::bytes::Put;
use crate::error::SqlState;
use crate::value::types::Descriptor;
use crate::value::{Binary, Decimal, Spelled};

/// The extension of the types these tests make, apart from any other.
const EXTENSION: &str = "BRIDGE_TYPE_TESTS";

/// The type POINTISH, ordered, of `EXTENSION`.
fn pointish() -> &'static PluginType {
    type_of(EXTENSION, "POINTISH", true).0
}

/// A value of POINTISH of key `key` and text `text`.
fn point(key: u8, text: &str) -> Value {
    pointish().value_of(&Made { key: vec![key], binary: vec![key, 0xFF], text: text.into() })
}

/// The type INTEGER.
fn integer() -> DataType {
    DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer }
}

/// Verifies: [EXTENSION] RULES.md D223: a plug-in's type is the same
/// kind each time it is asked, named and described by its name, takes no
/// parameters, is ordered as its manifest says, and is comparable only
/// with itself.
#[test]
fn a_plugin_type_is_one_kind() {
    let t = pointish();
    let other = type_of(EXTENSION, "POINTISH", false).0;
    assert!(t.is(*t.kind()) && !t.is(*other.kind()), "ORDERED or not, two kinds");
    assert_eq!((t.name(), format!("{t:?}")), ("POINTISH", format!("{EXTENSION}.POINTISH")));
    let dt = t.data_type();
    assert_eq!((t.written(&dt), dt.to_string()), ("POINTISH".to_owned(), "POINTISH".to_owned()));
    assert!(t.comparable(&dt) && t.comparable(&other.data_type()) && !t.comparable(&integer()));
    assert!(t.ordered() && !other.ordered() && t.assigns_text());
    assert_eq!((t.record_width(&dt), t.position(&point(1, "a"))), (None, None));
    assert_eq!(format!("{:?}", t.descriptor(&dt)), format!("{:?}", Descriptor::of("POINTISH")));
    let mut params = Vec::new();
    t.put_params(&dt, &mut params);
    assert_eq!(params, Vec::<u8>::new());
    assert_eq!(t.params(&mut Reader::new(&params, "params")).unwrap().to_string(), "POINTISH");
}

/// Verifies: [EXTENSION] RULES.md D223: values of the type are compared
/// and hashed by their keys, shown and written as a literal by their text,
/// cast to a character string by their text and to a binary string by
/// their binary form, and to no other type.
#[test]
fn values_are_compared_and_shown_by_their_forms() {
    let t = pointish();
    let (a, b) = (point(1, "it's"), point(2, "b"));
    assert_eq!(t.compare(&a, &b), Some(Ordering::Less));
    assert_eq!(t.compare(&a, &Value::Char("x".into())), None);
    assert_eq!(format!("{:?}", t.bucket(&a)), format!("{:?}", t.bucket(&point(1, "other"))));
    assert!(t.bucket(&Value::Null).is_none());
    assert_eq!((t.text(&a), t.literal(&a)), ("it's".to_owned(), "CAST('it''s' AS POINTISH)".to_owned()));
    assert_eq!(t.text(&Value::Null), "");
    let Some(Value::Char(text)) = t.lowered(&a, &DataType::character(10)) else { panic!("a character string") };
    assert_eq!(text, "it's");
    let Some(Value::Binary(binary)) = t.lowered(&a, &DataType::Binary { length: 2 }) else { panic!("a binary string") };
    assert_eq!(binary.bytes(), [1, 0xFF]);
    assert!(t.lowered(&a, &integer()).is_none());
}

/// Verifies: [EXTENSION] RULES.md D223 [ISO/IEC 9075:1992 §6.10 SR4]: a
/// value of the type or the null value is assigned and cast to it as it
/// is; another value is not assigned, SQLSTATE 22005, and only a
/// character or binary string is cast, by the program: with its extension
/// not created, none is made.
#[test]
fn values_are_assigned_and_cast_to_the_type() {
    let t = pointish();
    let dt = t.data_type();
    let a = point(3, "c");
    assert!(t.assign(a.clone(), &dt, "§9.2").is_ok() && t.assign(Value::Null, &dt, "§9.2").is_ok());
    assert_eq!(t.assign(Value::Char("c".into()), &dt, "§9.2").unwrap_err().sqlstate(), SqlState::ERROR_IN_ASSIGNMENT);
    assert!(t.cast(a, &dt).is_ok() && t.cast(Value::Null, &dt).is_ok());
    for given in [Value::Char("c".into()), Value::Binary(Binary::new(vec![3]))] {
        let e = t.cast(given, &dt).unwrap_err();
        assert!(e.msg.contains("is not created"), "{}", e.msg);
    }
    let number = Value::Exact(Decimal::new(1, 0));
    assert_eq!(t.cast(number, &dt).unwrap_err().sqlstate(), SqlState::ERROR_IN_ASSIGNMENT);
}

/// Verifies: [EXTENSION] RULES.md D223: a value is stored, logged and
/// keyed as its bytes, and read back the same; bytes that are no value's,
/// a key cut short, or another type's value are refused.
#[test]
fn values_are_stored_and_keyed_as_their_bytes() {
    let t = pointish();
    let dt = t.data_type();
    let a = point(4, "d");
    let mut out = Vec::new();
    t.put_value(&a, &mut out);
    let back = t.value(&mut Reader::new(&out, "a value")).unwrap();
    assert_eq!(t.compare(&a, &back), Some(Ordering::Equal));
    let mut garbage = Vec::new();
    garbage.put_bytes(&[9, 9]);
    assert!(t.value(&mut Reader::new(&garbage, "a value")).is_err());
    let record = t.encode_record(&a, &dt).unwrap();
    assert_eq!(t.compare(&t.decode_record(&record, &dt).unwrap(), &a), Some(Ordering::Equal));
    assert!(t.decode_record(&[1, 2, 3], &dt).is_none());
    assert!(t.encode_record(&Value::Char("d".into()), &dt).is_err());
    let mut key = Vec::new();
    t.encode_key(&a, &dt, &mut key).unwrap();
    let mut rest = key.as_slice();
    assert_eq!(t.compare(&t.decode_key(&mut rest, &dt).unwrap(), &a), Some(Ordering::Equal));
    assert_eq!(rest.len(), 0, "the key read whole");
    assert!(t.decode_key(&mut &key[..1], &dt).is_err(), "a key cut short");
    assert!(t.encode_key(&Value::Null, &dt, &mut Vec::new()).is_err());
    assert!(t.anchor(&a, &dt).is_some_and(|(v, whole)| whole && t.compare(&v, &a) == Some(Ordering::Equal)));
    assert!(t.anchor(&Value::Null, &dt).is_none());
}

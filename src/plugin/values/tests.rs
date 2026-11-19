//! Unit tests of the values of protocol 1: each kind written and read
//! back, malformed bytes refused, and a program's value stored in its
//! declared type (RULES.md R-TEST-1, R-TEST-2).

use super::*;
use crate::value::Spelled;

/// `v` written and read back.
fn again(v: &Value) -> Value {
    let mut out = Vec::new();
    put_value(&mut out, v).unwrap();
    let mut r = Reader::new(&out, "test");
    let read = value(&mut r).unwrap();
    r.finish().unwrap();
    read
}

/// Verifies: [EXTENSION] RULES.md D167: null, truth values, exact and
/// approximate numbers, character and binary strings, arrays and rows
/// read back as written; a datetime crosses in its text form; a vector,
/// dense or sparse, crosses as a vector.
#[test]
fn values_read_back_as_written() {
    let plain = [
        Value::Null,
        Value::Bool(true),
        Value::Exact(Decimal::new(-12_345, 2)),
        Value::Approx(2.5),
        Value::Char("é and ü".into()),
        Value::Binary(Binary::new(vec![0, 255, 7])),
        Value::Array(vec![Value::Bool(false), Value::Null].into()),
        Value::Row(vec![Value::Char("a".into()), Value::Approx(1.0)].into()),
    ];
    for v in plain {
        assert_eq!(again(&v).to_string(), v.to_string(), "{v:?}");
    }
    let date = cast(Value::Char("2026-11-19".into()), &DataType::Date).unwrap();
    let Value::Char(text) = again(&date) else { panic!("a text form") };
    assert_eq!(text, "2026-11-19");
    for made in [
        Vector::dense(Element::Float32, vec![1.0_f64, 0.0_f64, -2.0_f64]),
        Vector::sparse(Element::Float64, 6, vec![(1, 3.0_f64), (4, -1.0_f64)]),
    ] {
        let v = value_of(made.unwrap().encode().unwrap());
        let read = again(&v);
        let (a, b) = (Vector::of(vector_of(&v).unwrap()), Vector::of(vector_of(&read).unwrap()));
        assert_eq!((a.values(), a.layout()), (b.values(), b.layout()));
    }
}

/// Verifies: [EXTENSION] RULES.md D167: bytes of no value of protocol 1,
/// a tag unknown or a value cut short, are refused with SQLSTATE 38T03.
#[test]
fn malformed_values_are_refused() {
    for bad in [&[11_u8][..], &[EXACT, 1, 2], &[CHARACTER, 9, 0, 0, 0, b'a'], &[DENSE_VECTOR, 2, 0, 0, 0, 0]] {
        let mut r = Reader::received(bad, "test");
        let read = value(&mut r).map_err(|e| violation(&e.msg));
        assert_eq!(read.err().and_then(|e| e.state).map(|s| s.as_str().to_owned()), Some("38T03".into()), "{bad:?}");
    }
}

/// Verifies: [EXTENSION] RULES.md D169, ISO/IEC 9075-2:2023 §9.2: a
/// program's value is stored in its declared type, a string too long for
/// it a data exception, a text form cast.
#[test]
fn values_are_stored_in_their_declared_type() {
    let int = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };
    assert_eq!(stored(Value::Exact(Decimal::new(7, 0)), &int).unwrap().to_string(), "7");
    assert!(stored(Value::Char("too long".into()), &DataType::varying(3)).is_err());
    assert_eq!(stored(Value::Char("42".into()), &int).unwrap().to_string(), "42");
    assert!(stored(Value::Null, &int).unwrap().is_null());
}

//! Tests of the reference types' kind: text, comparison by octets, CAST
//! and assignment, parameters read back, and keys.

use super::*;
use crate::error::{Code, SqlState};
use crate::value::Decimal;

fn reference(octets: &[u8]) -> Value {
    Value::Reference(Binary::new(octets.to_vec()))
}

fn ref_of(scope: Option<UdtName>) -> DataType {
    DataType::reference(StructuredType { name: UdtName::new("S", "PERSON"), supertypes: Vec::new() }, scope)
}

fn number() -> Value {
    Value::Exact(Decimal::new(1, 0))
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.11.1] the kind's name is REF, a type
/// of another kind is written REF alone, and a value's text is REF and its
/// octets in hexadecimal, the null value's NULL; it has no position.
#[test]
fn reference_names_and_text() {
    assert_eq!(REFERENCE.name(), REF_NAME);
    assert_eq!(REFERENCE.written(&DataType::Boolean), "REF");
    assert_eq!(REFERENCE.written(&ref_of(None)), "REF(S.PERSON)");
    assert_eq!(REFERENCE.text(&reference(&[0x0A, 0xFF])), "REF(0AFF)");
    assert_eq!(REFERENCE.text(&Value::Null), "NULL");
    assert_eq!(REFERENCE.position(&reference(&[1])), None);
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.2 GR9] references compare and group
/// by their octets alone; a value of another type has none.
#[test]
fn reference_compares_by_octets() {
    assert!(REFERENCE.comparable(&ref_of(None)));
    assert!(!REFERENCE.comparable(&DataType::Boolean));
    assert_eq!(REFERENCE.compare(&reference(&[1]), &reference(&[2])), Some(Ordering::Less));
    assert_eq!(REFERENCE.compare(&reference(&[3]), &reference(&[3])), Some(Ordering::Equal));
    assert_eq!(REFERENCE.compare(&reference(&[1]), &number()), None);
    assert_eq!(REFERENCE.bucket(&reference(&[4])), REFERENCE.bucket(&reference(&[4])));
    assert_ne!(REFERENCE.bucket(&reference(&[4])), REFERENCE.bucket(&reference(&[5])));
    assert_eq!(REFERENCE.bucket(&number()), None);
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.2 GR3b xxi, §6.13 GR4a] a reference
/// and the null value are assigned and cast to a reference type; a value
/// of another type, or to a type of another kind, is neither (SQLSTATE
/// 22005).
#[test]
fn reference_cast_and_assignment() {
    let t = ref_of(None);
    assert!(REFERENCE.assign(Value::Null, &t, "test").unwrap().is_null());
    assert_eq!(REFERENCE.text(&REFERENCE.cast(reference(&[7]), &t).unwrap()), "REF(07)");
    assert_eq!(REFERENCE.assign(number(), &t, "test").unwrap_err().sqlstate(), SqlState::ERROR_IN_ASSIGNMENT);
    assert_eq!(REFERENCE.cast(number(), &t).unwrap_err().sqlstate(), SqlState::ERROR_IN_ASSIGNMENT);
    assert_eq!(REFERENCE.cast(reference(&[7]), &DataType::Boolean).unwrap_err().sqlstate(), SqlState::ERROR_IN_ASSIGNMENT);
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.11.1, §6.1 GR24] a reference type's
/// parameters, of a scope or none, read back as written; a type of
/// another kind writes none, and a scope flag other than 0 or 1 is
/// SQLCODE -916.
#[test]
fn reference_parameters_read_back() {
    for t in [ref_of(None), ref_of(Some(UdtName::new("S", "PEOPLE")))] {
        let mut out = Vec::new();
        REFERENCE.put_params(&t, &mut out);
        assert_eq!(REFERENCE.params(&mut Reader::new(&out, "parameters")).unwrap(), t);
    }
    let mut none = Vec::new();
    REFERENCE.put_params(&DataType::Boolean, &mut none);
    assert_eq!(none, Vec::<u8>::new());
    let mut out = Vec::new();
    REFERENCE.put_params(&ref_of(None), &mut out);
    let flag = out.len().checked_sub(1).unwrap();
    out[flag] = 2;
    assert_eq!(REFERENCE.params(&mut Reader::new(&out, "parameters")).unwrap_err().code, Code::Corrupt);
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.2 GR9] a reference's key reads back
/// as the same octets and the reference is its own anchor; a value of
/// another type is not keyed (SQLCODE -910), and a key cut short is
/// SQLCODE -916.
#[test]
fn reference_keys_read_back() {
    let (t, v) = (ref_of(None), reference(&[0, 9, 0]));
    let mut key = Vec::new();
    REFERENCE.encode_key(&v, &t, &mut key).unwrap();
    let back = REFERENCE.decode_key(&mut key.as_slice(), &t).unwrap();
    assert_eq!(REFERENCE.compare(&back, &v), Some(Ordering::Equal));
    assert_eq!(REFERENCE.anchor(&v, &t).map(|(_, exact)| exact), Some(true));
    assert!(REFERENCE.anchor(&number(), &t).is_none());
    assert_eq!(REFERENCE.encode_key(&number(), &t, &mut Vec::new()).unwrap_err().code, Code::Api);
    assert_eq!(REFERENCE.decode_key(&mut &[][..], &t).unwrap_err().code, Code::Corrupt);
}

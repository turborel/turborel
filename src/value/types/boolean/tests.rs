//! Unit tests of BOOLEAN's kind (`value/types/boolean.rs`).

use super::*;
use crate::error::Code;

fn code<T>(r: Result<T>) -> Option<Code> {
    r.err().map(|e| e.code)
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.13 GR21a] a character string is a
/// `<boolean literal>` in any case and with spaces either side, UNKNOWN
/// being null; any other is SQLSTATE 22018, a tab no space.
#[test]
fn boolean_literals_are_read_6_13() {
    assert_eq!(boolean_literal("  tRuE ").unwrap().to_string(), "TRUE");
    assert_eq!(boolean_literal("FALSE").unwrap().to_string(), "FALSE");
    assert!(boolean_literal("Unknown").unwrap().is_null());
    for not in ["", "T", "1", "\tTRUE", "TRUE FALSE", "NULL"] {
        let e = boolean_literal(not).unwrap_err();
        assert_eq!((e.code, e.sqlstate()), (Code::Data, SqlState::INVALID_CHARACTER_VALUE_FOR_CAST), "{not:?}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.13 GR11f, GR12f] a truth value cast
/// to a string is 'TRUE' or 'FALSE', padded only if fixed, and 22018 if
/// the string is shorter than the word.
///
/// Decision `length >= word's`: (4, TRUE) true and (3, TRUE) false, the
/// boundary pair; (5, FALSE) and (4, FALSE) likewise.
#[test]
fn truth_values_are_cast_to_text_6_13() {
    assert_eq!(as_text(true, &DataType::Character { length: 4 }).unwrap().to_string(), "TRUE");
    assert_eq!(as_text(true, &DataType::Character { length: 6 }).unwrap().to_string(), "TRUE  ");
    assert_eq!(as_text(true, &DataType::Varying { length: 6 }).unwrap().to_string(), "TRUE");
    assert_eq!(as_text(false, &DataType::Varying { length: 5 }).unwrap().to_string(), "FALSE");
    assert_eq!(code(as_text(true, &DataType::Varying { length: 3 })), Some(Code::Data));
    assert_eq!(code(as_text(false, &DataType::Character { length: 4 })), Some(Code::Data));
    assert_eq!(code(as_text(true, &DataType::Date)), Some(Code::Data), "no character string");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.6.2] True is greater than False, and
/// their keys order so; a value is one byte in a record and a key, read
/// back as written; a byte not 0 or 1 is no truth value (SQLCODE -916).
#[test]
fn truth_values_order_and_round_trip_4_6() {
    let (t, f) = (Value::Bool(true), Value::Bool(false));
    assert_eq!(BOOLEAN.compare(&t, &f), Some(Ordering::Greater));
    assert_eq!(BOOLEAN.compare(&f, &f), Some(Ordering::Equal));
    let (mut kt, mut kf) = (Vec::new(), Vec::new());
    BOOLEAN.encode_key(&t, &DataType::Boolean, &mut kt).unwrap();
    BOOLEAN.encode_key(&f, &DataType::Boolean, &mut kf).unwrap();
    assert!(kf < kt);
    for v in [&t, &f] {
        let record = BOOLEAN.encode_record(v, &DataType::Boolean).unwrap();
        assert_eq!(BOOLEAN.decode_record(&record, &DataType::Boolean).map(|d| d.to_string()), Some(v.to_string()));
        let mut key = Vec::new();
        BOOLEAN.encode_key(v, &DataType::Boolean, &mut key).unwrap();
        assert_eq!(BOOLEAN.decode_key(&mut key.as_slice(), &DataType::Boolean).unwrap().to_string(), v.to_string());
        let mut bytes = Vec::new();
        BOOLEAN.put_value(v, &mut bytes);
        assert_eq!(BOOLEAN.value(&mut Reader::new(&bytes, "a value")).unwrap().to_string(), v.to_string());
    }
    assert!(BOOLEAN.decode_record(&[2], &DataType::Boolean).is_none());
    assert!(BOOLEAN.decode_record(&[1, 1], &DataType::Boolean).is_none());
    assert_eq!(code(BOOLEAN.decode_key(&mut &[2][..], &DataType::Boolean)), Some(Code::Corrupt));
    assert_eq!(code(BOOLEAN.value(&mut Reader::new(&[7], "a value"))), Some(Code::Corrupt));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.13 GR21, §4.6.2] CAST to BOOLEAN
/// takes a truth value, null and a character string, and nothing else;
/// assignment takes a truth value alone; BOOLEAN is comparable with itself
/// alone.
#[test]
fn booleans_are_cast_assigned_and_compared_6_13() {
    let t = DataType::Boolean;
    assert_eq!(BOOLEAN.cast(Value::Char("true".into()), &t).unwrap().to_string(), "TRUE");
    assert!(BOOLEAN.cast(Value::Null, &t).unwrap().is_null());
    assert!(BOOLEAN.cast(Value::Exact(crate::value::Decimal::new(1, 0)), &t).is_err());
    assert_eq!(BOOLEAN.assign(Value::Bool(false), &t, "test").unwrap().to_string(), "FALSE");
    assert!(BOOLEAN.assign(Value::Char("TRUE".into()), &t, "test").is_err());
    assert!(BOOLEAN.comparable(&t) && !BOOLEAN.comparable(&DataType::Character { length: 4 }));
    assert_eq!(BOOLEAN.written(&t), "BOOLEAN");
    assert_eq!(BOOLEAN.literal(&Value::Bool(true)), "TRUE");
}

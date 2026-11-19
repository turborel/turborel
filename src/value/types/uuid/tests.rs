use super::*;
use crate::value::{cast, compare};

/// The UUID of RFC 9562's example of version 4 [RFC 9562 Appendix B.2].
const SAMPLE: &str = "919108f7-52d1-4320-9bac-f847db4148a8";

/// Verifies: [EXTENSION] RULES.md D180: a UUID's text is read hyphenated,
/// braced, upper case, without hyphens and with a hyphen after any group
/// of four digits; a text of the wrong length, a stray hyphen or a digit
/// not hexadecimal is refused.
#[test]
fn uuid_texts_are_read_in_each_form() {
    let octets = parse(SAMPLE).unwrap();
    assert_eq!(text(&octets), SAMPLE);
    for form in [
        "{919108f7-52d1-4320-9bac-f847db4148a8}",
        "919108F7-52D1-4320-9BAC-F847DB4148A8",
        "919108f752d143209bacf847db4148a8",
        " 9191-08f7-52d1-4320-9bac-f847-db41-48a8 ",
    ] {
        assert_eq!(parse(form), Some(octets), "{form}");
    }
    for bad in [
        "919108f7-52d1-4320-9bac-f847db4148a",
        "919108f7-52d1-4320-9bac-f847db4148a8-",
        "-919108f7-52d1-4320-9bac-f847db4148a8",
        "919108f7--52d1-4320-9bac-f847db4148a8",
        "919108f7-52d1-4320-9bac-f847db4148ag",
        "919-108f7-52d1-4320-9bac-f847db4148a8",
        "{919108f7-52d1-4320-9bac-f847db4148a8",
        "",
    ] {
        assert_eq!(parse(bad), None, "{bad}");
    }
    assert_eq!(version(&octets), 4);
}

/// Verifies: [EXTENSION] RULES.md D180: CAST takes a character string and
/// a binary string of 16 octets to UUID, and a UUID to both; text not a
/// UUID's fails with 22018, a binary string of another length too, and a
/// number cannot be cast (22005).
#[test]
fn uuid_casts_both_ways() {
    let u = cast(Value::Char(SAMPLE.to_uppercase()), &UuidType::data_type()).unwrap();
    assert_eq!(u.to_string(), SAMPLE);
    assert_eq!(cast(u.clone(), &DataType::varying(40)).unwrap().to_string(), SAMPLE);
    let bytes = cast(u.clone(), &DataType::VarBinary { length: 16 }).unwrap();
    assert_eq!(cast(bytes, &UuidType::data_type()).unwrap().to_string(), SAMPLE);
    let state = |v: Value| cast(v, &UuidType::data_type()).unwrap_err().state.unwrap();
    assert_eq!(state(Value::Char("nope".into())), SqlState::INVALID_CHARACTER_VALUE_FOR_CAST);
    assert_eq!(state(Value::Binary(Binary::new(vec![1, 2]))), SqlState::INVALID_CHARACTER_VALUE_FOR_CAST);
    assert_eq!(state(Value::Bool(true)), SqlState::ERROR_IN_ASSIGNMENT);
    assert_eq!(u.to_literal(), format!("UUID '{SAMPLE}'"));
    assert!(cast(Value::Null, &UuidType::data_type()).unwrap().is_null());
}

/// Verifies: [EXTENSION] RULES.md D180: UUIDs order as their octets do,
/// unsigned, equal ones share a bucket, and keys order as the values do;
/// a record and a key read back what was written, and the bytes the log
/// and the wire carry too.
#[test]
fn uuid_order_keys_and_bytes() {
    let low = uuid_value([0x7f; UUID_OCTETS]);
    let high = uuid_value([0x80; UUID_OCTETS]);
    assert_eq!(compare(&low, &high), Some(Ordering::Less));
    assert_eq!(UUID_TYPE.bucket(&low), UUID_TYPE.bucket(&low.clone()));
    let t = UuidType::data_type();
    let (mut a, mut b) = (Vec::new(), Vec::new());
    t.encode_key(&low, &mut a).unwrap();
    t.encode_key(&high, &mut b).unwrap();
    assert!(a < b);
    let mut rest = a.as_slice();
    assert_eq!(compare(&t.decode_key(&mut rest).unwrap(), &low), Some(Ordering::Equal));
    assert_eq!(rest.len(), 0, "the key consumed whole");
    let record = t.encode_record(&high).unwrap();
    assert_eq!((record.len(), UUID_TYPE.record_width(&t)), (UUID_OCTETS, Some(UUID_OCTETS)));
    assert_eq!(compare(&t.decode_record(&record).unwrap(), &high), Some(Ordering::Equal));
    let mut wire = Vec::new();
    UUID_TYPE.put_value(&high, &mut wire);
    let read = UUID_TYPE.value(&mut Reader::new(&wire, "a value")).unwrap();
    assert_eq!(compare(&read, &high), Some(Ordering::Equal));
    assert!(t.encode_key(&Value::Bool(true), &mut Vec::new()).is_err());
    assert!(UUID_TYPE.position(&low).unwrap() < UUID_TYPE.position(&high).unwrap());
    assert!(UUID_TYPE.comparable(&t) && !UUID_TYPE.comparable(&DataType::varying(36)));
}

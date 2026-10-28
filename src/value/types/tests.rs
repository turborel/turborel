//! Unit tests of the kinds' refusals (RULES.md R-TEST-1, R-TEST-2): a
//! value of another kind given to each, a malformed key, and what a value
//! of a type not registered here is and is not. Split from types.rs
//! (RULES.md R-SIZE-2).

use super::*;
use crate::bytes::Put;
use crate::error::Code;
use std::cmp::Ordering;

use crate::value::{Bits, Decimal, compare, compare_strings};

/// A value of the kind beyond the standard nothing here knows.
fn foreign() -> Value {
    unregistered("UUID", "ABC", &[1, 2, 3])
}

fn code<T>(r: Result<T>) -> Option<Code> {
    r.err().map(|e| e.code)
}

/// Verifies: [DESIGN] RULES.md D53: a value of a type not registered here
/// keeps its type's name, its text and its bytes, is shown and written
/// back as it came, and is nothing else: not compared, hashed, assigned,
/// cast, stored or keyed.
#[test]
fn an_unregistered_value_is_kept_and_shown_only() {
    let v = foreign();
    let k = &UNREGISTERED;
    assert_eq!(parts(&v), Some(("UUID".to_owned(), "ABC".to_owned(), vec![1, 2, 3])));
    assert_eq!(parts(&Value::Null), None);
    assert_eq!((k.text(&v), k.literal(&v), v.to_string()), ("ABC".to_owned(), "ABC".to_owned(), "ABC".to_owned()));
    let t = DataType::Extension(ExtensionType { kind: &UNREGISTERED_KIND, params: TypeParams::default() });
    assert_eq!((k.written(&t), k.descriptor(&t).name), ("UNREGISTERED".to_owned(), "UNREGISTERED".to_owned()));
    assert!(!k.comparable(&t) && k.compare(&v, &v).is_none() && k.bucket(&v).is_none() && k.position(&v).is_none());
    let mut bytes = Vec::new();
    k.put_value(&v, &mut bytes);
    let read = k.value(&mut Reader::new(&bytes, "value")).unwrap();
    assert_eq!(parts(&read), parts(&v), "written back as it came");
}

/// Verifies: [DESIGN] RULES.md D53: a value of a type not registered here
/// is not assigned, cast, stored or keyed, and its type has no parameters
/// to read.
#[test]
fn an_unregistered_value_is_nothing_else() {
    let (v, k) = (foreign(), &UNREGISTERED);
    let t = DataType::Extension(ExtensionType { kind: &UNREGISTERED_KIND, params: TypeParams::default() });
    assert_eq!(code(k.assign(v.clone(), &t, "test")), Some(Code::Data));
    assert_eq!(code(k.cast(v.clone(), &t)), Some(Code::Data));
    let mut params = Vec::new();
    k.put_params(&t, &mut params);
    assert_eq!(params, Vec::<u8>::new());
    assert_eq!(code(k.params(&mut Reader::new(&[], "parameters"))), Some(Code::Api));
    assert!(k.record_width(&t).is_none() && k.decode_record(&[], &t).is_none() && k.anchor(&v, &t).is_none());
    assert_eq!(code(k.encode_record(&v, &t)), Some(Code::Api));
    assert_eq!(code(k.encode_key(&v, &t, &mut Vec::new())), Some(Code::Api));
    assert_eq!(code(k.decode_key(&mut &[][..], &t)), Some(Code::Api));
    assert_eq!(code(k.check_params(&[1])), Some(Code::SyntaxRule), "none by default");
    assert!(k.check_params(&[]).is_ok());
}

/// Verifies: [DESIGN] RULES.md D53: each built-in kind, given a value of
/// another kind (as a plug-in function or type might give), refuses to
/// store or key it (SQLCODE -910), has no anchor or position for it, and
/// CAST and assignment refuse a value beyond the standard (SQLSTATE 22005);
/// a key cut short is SQLCODE -916.
#[test]
fn built_in_kinds_refuse_other_kinds() {
    let types = [
        DataType::Character { length: 3 },
        DataType::Exact { precision: 5, scale: 1 },
        DataType::Approximate { precision: 53 },
        DataType::Bit { length: 3 },
        DataType::BitVarying { length: 3 },
        DataType::Date,
        DataType::Time { precision: 3, zoned: true },
        DataType::Timestamp { precision: 6, zoned: false },
    ];
    let others = [
        Value::Exact(Decimal::new(1, 0)),
        Value::Char("A".into()),
        Value::Char("B".into()),
        Value::Char("C".into()),
        Value::Exact(Decimal::new(2, 0)),
        Value::Char("2024-01-01".into()),
        Value::Bits(Bits::default()),
        Value::Approx(1.0),
    ];
    for (t, other) in types.iter().zip(&others) {
        refuses(t, other);
    }
}

/// Checks that `t`'s kind refuses `other`, a value of another kind, and a
/// value beyond the standard.
fn refuses(t: &DataType, other: &Value) {
    let k = t.kind();
    assert_eq!(code(k.encode_record(&foreign(), t)), Some(Code::Api), "{t}");
    assert_eq!(code(k.encode_record(other, t)), Some(Code::Api), "{t}");
    assert_eq!(code(k.encode_key(other, t, &mut Vec::new())), Some(Code::Api), "{t}");
    assert!(k.anchor(other, t).is_none() && k.anchor(&foreign(), t).is_none(), "{t}");
    assert!(k.position(&Value::Char("A".into())).is_none(), "{t}");
    assert!(!k.comparable(&DataType::Extension(ExtensionType { kind: &UNREGISTERED_KIND, params: TypeParams::default() })));
    assert_eq!(code(k.assign(foreign(), t, "test")), Some(Code::Data), "{t}");
    assert_eq!(code(k.decode_key(&mut &[0xC3][..], t)), Some(Code::Corrupt), "{t}: a key cut short");
    assert!(k.compare(&Value::Null, other).is_none() && k.bucket(&Value::Null).is_none(), "{t}");
    assert_eq!(k.text(&Value::Null), "NULL");
}

/// Verifies: [DESIGN] RULES.md D53: CAST to a number refuses a value
/// beyond the standard, and CAST to a string gives its text; the built-in
/// kinds write literals as SQL does, and a value of another kind as their
/// kind's zero.
#[test]
fn built_in_kinds_cast_and_write() {
    assert_eq!(code(EXACT.cast(foreign(), &DataType::Exact { precision: 5, scale: 0 })), Some(Code::Data));
    assert_eq!(code(APPROXIMATE.cast(foreign(), &DataType::Approximate { precision: 24 })), Some(Code::Data));
    let shown = CHARACTER.cast(foreign(), &DataType::Character { length: 5 }).unwrap();
    assert_eq!(shown.to_string(), "ABC  ", "a value beyond the standard CAST to a string is its text");
    assert_eq!(EXACT.literal(&Value::Exact(Decimal::new(15, 1))), "1.5");
    assert_eq!(APPROXIMATE.literal(&Value::Approx(1.5)), "1.5E0");
    assert_eq!(CHARACTER.literal(&Value::Char("It's".into())), "'It''s'");
    let mut out = Vec::new();
    CHARACTER.put_value(&Value::Null, &mut out);
    EXACT.put_value(&Value::Null, &mut out);
    APPROXIMATE.put_value(&Value::Null, &mut out);
    assert_eq!(out.len(), 4 + 16 + 4 + 8, "a value of another kind writes its kind's zero");
}

/// Verifies: [DESIGN] RULES.md D53; [ISO/IEC 9075:1992 §6.10 SR4, §5.3]
/// the bit kinds refuse to CAST a number or a value beyond the standard
/// (SQLSTATE 22005), write a literal as B'...', a value of another kind as
/// the empty bit string, and read back no bit string longer than its
/// bytes (SQLCODE -916); a record of a fixed length is of the bytes its
/// bits take, one of another length refused (SQLCODE -910).
#[test]
fn bit_kinds_cast_and_write() {
    let t = DataType::Bit { length: 4 };
    assert_eq!(code(BIT.cast(Value::Exact(Decimal::new(1, 0)), &t)), Some(Code::Data));
    assert_eq!(code(BIT_VARYING.cast(foreign(), &t)), Some(Code::Data));
    let b = Value::Bits(Bits::of_bits("101").unwrap());
    assert_eq!((BIT.literal(&b), BIT.literal(&Value::Null)), ("B'101'".to_owned(), "NULL".to_owned()));
    assert_eq!(code(BIT.encode_record(&b, &t)), Some(Code::Api), "3 bits for BIT(4)");
    assert_eq!(BIT.record_width(&t), Some(1));
    assert!(BIT.decode_record(&[], &t).is_none() && BIT_VARYING.decode_record(&[], &t).is_none());
    assert!(BIT_VARYING.decode_record(&[9, 0], &t).is_none(), "more bits unused than a byte has");
    let mut out = Vec::new();
    BIT.put_value(&Value::Null, &mut out);
    assert_eq!(out.len(), 4 + 4, "the empty bit string");
    let mut long = Vec::new();
    long.put_u32(9);
    long.put_bytes(&[0xFF]);
    assert_eq!(code(BIT.value(&mut Reader::new(&long, "value"))), Some(Code::Corrupt), "9 bits in a byte");
    assert_eq!(BIT.descriptor(&t).octets, Some(1));
    assert_eq!(BIT_VARYING.written(&DataType::BitVarying { length: 7 }), "BIT VARYING(7)");
}

/// Verifies: [DESIGN] RULES.md D53: a type's parameters are at most two,
/// none `u32::MAX`, read back in order; a kind is registered once by its
/// name; a value or type named beyond the standard is read only where its
/// kind is registered (SQLCODE -910).
#[test]
fn registries_and_parameters() {
    assert_eq!(TypeParams::of(&[3, 4]).map(|p| p.values().to_vec()), Some(vec![3, 4]));
    assert_eq!(TypeParams::of(&[7]).map(|p| p.values().to_vec()), Some(vec![7]));
    assert!(TypeParams::of(&[1, 2, 3]).is_none() && TypeParams::of(&[u32::MAX]).is_none());
    let none: &[u32] = &[];
    assert_eq!(TypeParams::default().values(), none);
    let mut registry = TypeRegistry::default();
    registry.add(&UNREGISTERED).unwrap();
    assert_eq!(code(registry.add(&UNREGISTERED)), Some(Code::Api), "once");
    assert_eq!(registry.names().collect::<Vec<_>>(), ["UNREGISTERED"]);
    assert!(registry.get("UNREGISTERED").is_some() && registry.kind("NOPE").is_none());
    let mut named = Vec::new();
    named.put_str("NOPE");
    assert_eq!(code(Reader::new(&named, "value").kind("NOPE")), Some(Code::Api), "no registry");
    assert_eq!(code(Reader::new(&named, "value").with_types(&registry).kind("NOPE")), Some(Code::Api), "not in it");
    let a = ExtensionType { kind: &UNREGISTERED_KIND, params: TypeParams::default() };
    let b = ExtensionType { kind: &UNREGISTERED_KIND, params: TypeParams::of(&[1]).unwrap() };
    assert!(a == a && a != b, "the same kind and parameters");
}

/// Verifies: [DESIGN] RULES.md D53; [ISO/IEC 9075:1992 §8.2 GR3a] the keys
/// of CHARACTER VARYING, compared as bytes, order every two strings of up
/// to three characters, of characters below, at and above the space and
/// beyond the Basic Multilingual Plane, as they compare padded with
/// spaces; equal strings have one key; no key begins another, so two
/// columns' keys order as the pairs of strings; each reads back equal.
#[test]
fn varying_keys_order_as_strings_compare() {
    let alphabet = ['\u{0}', '\u{9}', ' ', '!', 'a', 'é', '\u{10000}'];
    let mut strings = vec![String::new()];
    for _ in 0..3_u8 {
        let longer: Vec<String> = strings.iter().flat_map(|s| alphabet.iter().map(move |c| format!("{s}{c}"))).collect();
        strings.extend(longer);
    }
    strings.sort();
    strings.dedup();
    let t = DataType::Varying { length: 3 };
    let key = |s: &str| {
        let mut out = Vec::new();
        t.encode_key(&Value::Char(s.to_owned()), &mut out).unwrap();
        out
    };
    let keys: Vec<Vec<u8>> = strings.iter().map(|s| key(s)).collect();
    for (a, ka) in strings.iter().zip(&keys) {
        let read = t.decode_key(&mut ka.as_slice()).unwrap();
        assert_eq!(compare(&read, &Value::Char(a.clone())), Some(Ordering::Equal), "{a:?} reads back");
        for (b, kb) in strings.iter().zip(&keys) {
            assert_eq!(ka.cmp(kb), compare_strings(a, b), "{a:?} and {b:?}");
            assert!(ka == kb || !kb.starts_with(ka), "{a:?}'s key begins {b:?}'s");
        }
    }
}

//! Unit tests of the kinds' refusals (RULES.md R-TEST-1, R-TEST-2): a
//! value of another kind given to each, a malformed key, and what a value
//! of a type not registered here is and is not; with the bit kinds' bytes
//! and literals, registries and parameters, and CHARACTER VARYING's keys.

use super::*;
use crate::bytes::Put;
use crate::error::Code;
use std::cmp::Ordering;

use crate::value::Spelled;
use crate::value::{Binary, Decimal, compare, compare_strings};

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
        DataType::character(3),
        DataType::Exact { precision: 5, scale: 1, spelled: Spelled::Numeric },
        DataType::Approximate { precision: 53, spelled: Spelled::Float },
        DataType::Binary { length: 3 },
        DataType::VarBinary { length: 3 },
        DataType::Boolean,
        DataType::Date,
        DataType::Time { precision: 3, zoned: true },
        DataType::Timestamp { precision: 6, zoned: false },
        DataType::Interval(crate::value::interval::Qualifier::new(
            crate::value::datetime::Field::Day,
            crate::value::datetime::Field::Day,
            2,
            0,
        )),
    ];
    let others = [
        Value::Exact(Decimal::new(1, 0)),
        Value::Char("A".into()),
        Value::Char("B".into()),
        Value::Char("C".into()),
        Value::Exact(Decimal::new(4, 0)),
        Value::Exact(Decimal::new(2, 0)),
        Value::Char("2024-01-01".into()),
        Value::Binary(Binary::default()),
        Value::Approx(1.0),
        Value::Exact(Decimal::new(3, 0)),
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
    assert_eq!(code(EXACT.cast(foreign(), &DataType::Exact { precision: 5, scale: 0, spelled: Spelled::Numeric })), Some(Code::Data));
    assert_eq!(code(APPROXIMATE.cast(foreign(), &DataType::Approximate { precision: 24, spelled: Spelled::Float })), Some(Code::Data));
    let shown = CHARACTER.cast(foreign(), &DataType::character(5)).unwrap();
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

/// Verifies: [DESIGN] RULES.md D53; [ISO/IEC 9075-2:2023 §6.13 SR7,
/// §5.3] the binary kinds refuse to CAST a number, a character string or
/// a value beyond the standard (SQLSTATE 22005), write a literal as
/// X'...', a value of another kind as the empty binary string, and read
/// back no value cut short (SQLCODE -916); a record of a fixed length is
/// of its octets, one of another length refused (SQLCODE -910), and read
/// back only of that length.
#[test]
fn binary_kinds_cast_and_write() {
    let t = DataType::Binary { length: 4 };
    assert_eq!(code(BINARY.cast(Value::Exact(Decimal::new(1, 0)), &t)), Some(Code::Data));
    assert_eq!(code(BINARY.cast(Value::Char("AB".into()), &t)), Some(Code::Data));
    assert_eq!(code(BINARY_VARYING.cast(foreign(), &t)), Some(Code::Data));
    let b = Value::Binary(Binary::new(vec![0xA0, 0x0B, 0]));
    assert_eq!((BINARY.literal(&b), BINARY.literal(&Value::Null)), ("X'A00B00'".to_owned(), "NULL".to_owned()));
    assert_eq!(code(BINARY.encode_record(&b, &t)), Some(Code::Api), "3 octets for BINARY(4)");
    assert_eq!(BINARY.record_width(&t), Some(4));
    assert!(BINARY.decode_record(&[1, 2, 3], &t).is_none() && BINARY_VARYING.decode_record(&[], &t).is_some());
    let mut out = Vec::new();
    BINARY.put_value(&Value::Null, &mut out);
    assert_eq!(out.len(), 4, "the empty binary string");
    let mut short = Vec::new();
    short.put_u32(9);
    short.put_u8(0xFF);
    assert_eq!(code(BINARY.value(&mut Reader::new(&short, "value"))), Some(Code::Corrupt), "9 octets of 1");
    assert_eq!(BINARY.descriptor(&t).octets, Some(4));
    assert_eq!(BINARY_VARYING.written(&DataType::VarBinary { length: 7 }), "BINARY VARYING(7)");
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
    let t = DataType::varying(3);
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

/// Verifies: [DESIGN] R-ERR-2: an assignment's message cites the rule
/// applying it and the clause applied, a clause of SQL-92 after a rule of
/// SQL-92 without its standard again, after another rule with it, and a
/// clause naming its standard as it is. MC/DC of `assignment_rule`: the
/// clause names its standard (third case against the first), the rule is
/// of SQL-92 (first against second).
#[test]
fn assignment_rules_cite_their_standards() {
    use crate::value::assignment_rule;
    let (sql92, x3, sql2023) = ("ISO/IEC 9075:1992 §13.8 GR4b iii", "X3.135-1986 §8.7 GR7", "ISO/IEC 9075-2:2023 §9.2 GR3b ix 2");
    assert_eq!(assignment_rule(sql92, "§9.2 GR3e"), "ISO/IEC 9075:1992 §13.8 GR4b iii, §9.2 GR3e");
    assert_eq!(assignment_rule(x3, "§9.2 GR3e"), "X3.135-1986 §8.7 GR7; ISO/IEC 9075:1992 §9.2 GR3e");
    assert_eq!(assignment_rule(sql92, sql2023), "ISO/IEC 9075:1992 §13.8 GR4b iii; ISO/IEC 9075-2:2023 §9.2 GR3b ix 2");
    assert_eq!(assignment_rule(x3, sql2023), "X3.135-1986 §8.7 GR7; ISO/IEC 9075-2:2023 §9.2 GR3b ix 2");
}

/// Verifies: [DESIGN] RULES.md D53: CHARACTER VARYING and DECFLOAT, given a
/// value of another kind, refuse to store or key it as every built-in kind
/// does.
#[test]
fn varying_and_decfloat_refuse_other_kinds() {
    refuses(&DataType::varying(3), &Value::Exact(Decimal::new(1, 0)));
    refuses(&DataType::DecFloat { precision: 16 }, &Value::Char("A".into()));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.13 GR6, §5.3, §4.2.3]
/// CHARACTER VARYING's kind compares strings padded, buckets them without
/// trailing spaces, writes a literal of its quotes doubled and has no
/// position; CAST to it of an approximate number gives its literal, of a
/// datetime its literal's string and of a value beyond the standard its
/// text, cut to the length.
#[test]
fn varying_compares_buckets_and_casts() {
    let (a, b) = (Value::Char("ab".into()), Value::Char("ab  ".into()));
    assert_eq!(VARYING.compare(&a, &b), Some(Ordering::Equal));
    assert_eq!(VARYING.compare(&a, &Value::Exact(Decimal::new(1, 0))), None);
    assert_eq!(VARYING.bucket(&b), Some(Bucket::Char("ab".into())));
    assert_eq!(VARYING.literal(&Value::Char("it's".into())), "'it''s'");
    assert_eq!(VARYING.literal(&Value::Exact(Decimal::new(1, 0))), "1");
    assert_eq!(VARYING.text(&a), "ab");
    assert!(VARYING.position(&a).is_none());
    let t = DataType::varying(10);
    assert_eq!(VARYING.cast(Value::Approx(1.5), &t).unwrap().to_string(), "1.5E0");
    let day = crate::value::datetime::parse(crate::value::datetime::Moment::Date, "2024-02-29").unwrap();
    let date = Value::Datetime(crate::value::Datetime::of_local(crate::value::datetime::Moment::Date, day.local, 0, false, 0));
    assert_eq!(VARYING.cast(date, &t).unwrap().to_string(), "2024-02-29");
    assert_eq!(VARYING.cast(foreign(), &DataType::varying(2)).unwrap().to_string(), "AB", "its text, cut");
    assert_eq!(code(VARYING.cast(Value::Binary(Binary::default()), &t)), Some(Code::Data));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.2 GR3b xv, §6.13 GR10]
/// DECFLOAT's kind is named DECFLOAT, compares and buckets numbers of any
/// kind as numbers, assigns a number rounded and refuses a value of
/// another kind (22005); an exact number
/// is its own anchor, an approximate one is none.
#[test]
fn decfloat_compares_assigns_and_anchors() {
    let t = DataType::DecFloat { precision: 16 };
    let (d, e) = (Value::DecFloat(crate::value::DecFloat::of_decimal(&Decimal::new(15, 1)).unwrap()), Value::Exact(Decimal::new(15, 1)));
    assert_eq!(DECFLOAT.name(), DECFLOAT_NAME);
    assert_eq!(DECFLOAT.compare(&d, &e), Some(Ordering::Equal));
    assert_eq!(DECFLOAT.bucket(&d), crate::value::bucket(&e));
    assert_eq!(DECFLOAT.text(&d), d.to_string());
    assert_eq!(DECFLOAT.assign(e.clone(), &t, "store").unwrap().to_string(), d.to_string());
    let state = |r: Result<Value>| r.unwrap_err().sqlstate().as_str().to_owned();
    assert_eq!(state(DECFLOAT.assign(Value::Char("1".into()), &t, "store")), "22005");
    let (anchor, exact) = DECFLOAT.anchor(&e, &t).unwrap();
    assert!(exact && compare(&anchor, &e) == Some(Ordering::Equal));
    assert!(DECFLOAT.anchor(&Value::Approx(1.5), &t).is_none());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.10, §4.12, §8.2 GR1b,
/// §9.12 SR3] the kinds of ROW, ARRAY and MULTISET: their names; values
/// compared and bucketed part by part, a multiset's in any order; their
/// text their literal; no position; no array or multiset in a key, so a
/// key of one is malformed and each is its own anchor; a row's key of a
/// field neither null nor present malformed; a row type of no fields, or
/// an array type of no maximum cardinality, malformed.
#[test]
fn composite_kinds_compare_bucket_and_refuse_keys() {
    let n = |u: i128| Value::Exact(Decimal::new(u, 0));
    let row = Value::Row(vec![n(1), Value::Char("a".into())].into());
    let array = Value::Array(vec![n(1), n(2)].into());
    let bag = Value::Multiset(vec![n(2), n(1)].into());
    let kinds: [(&dyn SqlType, &Value, &str); 3] = [(&ROW, &row, ROW_NAME), (&ARRAY, &array, ARRAY_NAME), (&MULTISET, &bag, MULTISET_NAME)];
    for (k, v, name) in kinds {
        assert_eq!(k.name(), name);
        assert_eq!(k.compare(v, v), Some(Ordering::Equal), "{name}");
        assert!(k.bucket(v).is_some(), "{name}");
        assert_eq!(k.text(v), k.literal(v), "{name}");
        assert!(k.position(v).is_none(), "{name}");
    }
    assert_eq!(MULTISET.bucket(&bag), MULTISET.bucket(&Value::Multiset(vec![n(1), n(2)].into())), "in any order");
    let list = DataType::array(DataType::Boolean, 2);
    assert_eq!(code(ARRAY.decode_key(&mut &[0_u8][..], &list)), Some(Code::Corrupt));
    let (anchor, exact) = ARRAY.anchor(&array, &list).unwrap();
    assert!(exact && anchor.to_literal() == array.to_literal());
    let bags = DataType::multiset(DataType::Boolean);
    assert_eq!(code(MULTISET.decode_key(&mut &[0_u8][..], &bags)), Some(Code::Corrupt));
    assert!(MULTISET.anchor(&bag, &bags).is_some() && MULTISET.anchor(&n(1), &bags).is_none());
    let pair = DataType::row(vec![Field { name: "X".into(), data_type: DataType::Boolean, unnamed: false }]);
    assert_eq!(code(ROW.decode_key(&mut &[7_u8][..], &pair)), Some(Code::Corrupt), "neither null nor present");
    assert_eq!(code(ROW.params(&mut Reader::new(&[0, 0, 0, 0], "parameters"))), Some(Code::Corrupt), "no fields");
    assert_eq!(code(ARRAY.params(&mut Reader::new(&[0, 0, 0, 0], "parameters"))), Some(Code::Corrupt), "cardinality 0");
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.1] a binary large object type, a
/// decimal floating-point type and a time type without time zone read
/// back as written; a tag that names no type is SQLCODE -916.
#[test]
fn type_tags_read_back() {
    let types = [
        DataType::Lob { kind: crate::value::lob::LobKind::Binary, length: 100, units: crate::value::units::CharUnits::Characters },
        DataType::DecFloat { precision: 16 },
        DataType::Time { precision: 0, zoned: false },
    ];
    for t in types {
        let mut out = Vec::new();
        tags::put_type(&mut out, &t);
        assert_eq!(tags::data_type(&mut Reader::new(&out, "type")).unwrap(), t);
    }
    assert_eq!(code(tags::data_type(&mut Reader::new(&[255], "type"))), Some(Code::Corrupt));
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.12 SR3, §6.13 GR5d, §4.10] no
/// multiset is in a key (SQLCODE -910), nor cast to a type not a
/// collection (SQLSTATE 22005); a row's null field keys as null and reads
/// back so; a row of fields other than its type's is not stored; a
/// distinct type's kind is USER-DEFINED.
#[test]
fn multisets_rows_and_distinct_kind() {
    let n = |u: i128| Value::Exact(Decimal::new(u, 0));
    let bags = DataType::multiset(DataType::Boolean);
    let bag = Value::Multiset(vec![Value::Bool(true)].into());
    assert_eq!(code(MULTISET.encode_key(&bag, &bags, &mut Vec::new())), Some(Code::Api));
    assert_eq!(MULTISET.cast(bag, &DataType::Boolean).unwrap_err().sqlstate(), SqlState::ERROR_IN_ASSIGNMENT);
    let field = |name: &str| Field {
        name: name.into(),
        data_type: DataType::Exact { precision: 5, scale: 0, spelled: Spelled::Integer },
        unnamed: false,
    };
    let pair = DataType::row(vec![field("X"), field("Y")]);
    let mut key = Vec::new();
    ROW.encode_key(&Value::Row(vec![Value::Null, n(4)].into()), &pair, &mut key).unwrap();
    let back = ROW.decode_key(&mut key.as_slice(), &pair).unwrap();
    assert_eq!(back.to_literal(), Value::Row(vec![Value::Null, n(4)].into()).to_literal());
    assert_eq!(code(ROW.encode_record(&Value::Row(vec![n(1)].into()), &pair)), Some(Code::Api));
    assert_eq!(DISTINCT.name(), USER_DEFINED_NAME);
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.1 SR16, §4.2.2] a key of CHARACTER of
/// a length in octets reads back as the padded string; a key of a length
/// in characters holding fewer characters, or a units tag other than 0 or
/// 1, is SQLCODE -916; the interval kinds are named INTERVAL.
#[test]
fn character_keys_of_units_and_interval_name() {
    let octets = DataType::Character { length: 3, units: crate::value::units::CharUnits::Octets };
    let mut key = Vec::new();
    CHARACTER.encode_key(&Value::Char("ab".into()), &octets, &mut key).unwrap();
    let back = CHARACTER.decode_key(&mut key.as_slice(), &octets).unwrap();
    assert_eq!(CHARACTER.text(&back), "ab ");
    let chars = DataType::character(3);
    assert_eq!(code(CHARACTER.decode_key(&mut &b"ab"[..], &chars)), Some(Code::Corrupt));
    assert_eq!(code(CHARACTER.params(&mut Reader::new(&[0, 0, 0, 3, 2], "parameters"))), Some(Code::Corrupt));
    assert_eq!(INTERVAL_DAY_TIME.name(), "INTERVAL");
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.30 GR7, §9.2 GR3b xv, §4.17.2.3]
/// DECFLOAT arithmetic with an approximate operand beyond its range is
/// SQLSTATE 22003; a value not a number is not assigned to DECFLOAT
/// (22005); a time type has no highest value, as only DATE and timestamps
/// end a system-time period.
#[test]
fn decfloat_refusals_and_no_highest_time() {
    let t = DataType::DecFloat { precision: 16 };
    let one = DECFLOAT.cast(Value::Exact(Decimal::new(1, 0)), &t).unwrap();
    let sum = decfloat_arith(crate::value::operators::ArithOp::Add, &Value::Approx(f64::INFINITY), &one);
    assert_eq!(sum.unwrap_err().sqlstate(), SqlState::NUMERIC_VALUE_OUT_OF_RANGE);
    assert_eq!(DECFLOAT.assign(Value::Bool(true), &t, "test").unwrap_err().sqlstate(), SqlState::ERROR_IN_ASSIGNMENT);
    assert!(datetime::highest(&DataType::Time { precision: 0, zoned: false }).is_none());
    assert!(datetime::highest(&DataType::Date).is_some());
}

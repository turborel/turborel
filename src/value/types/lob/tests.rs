use super::*;
use crate::value::Decimal;
use crate::value::lob::fixtures::{chunked_by, octets_by, text};

/// CHARACTER LARGE OBJECT(`length`) of `units`.
fn clob(length: u64, units: CharUnits) -> DataType {
    DataType::Lob { kind: LobKind::Character, length, units }
}

/// BINARY LARGE OBJECT(`length`).
fn blob(length: u64) -> DataType {
    DataType::Lob { kind: LobKind::Binary, length, units: CharUnits::Characters }
}

/// The whole number `n`.
fn n(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// The octets of `v`, a large object, read whole.
fn octets_of(v: &Value) -> Vec<u8> {
    let Value::Lob(l) = v else { panic!("not a large object: {v:?}") };
    l.reader().read(0, l.octets()).unwrap()
}

/// The SQLSTATE `r` fails with.
fn state<T: std::fmt::Debug>(r: Result<T>) -> String {
    r.unwrap_err().sqlstate().as_str().to_owned()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.1, §4.3.2, §4.4.2] the
/// names, designators and descriptors of the large object types: a
/// character one of characters or octets, four octets to a character, and
/// collated; a binary one of octets; each comparable with strings of its
/// family alone.
#[test]
fn names_designators_and_descriptors() {
    assert_eq!(CLOB.written(&clob(10, CharUnits::Characters)), "CHARACTER LARGE OBJECT(10)");
    assert_eq!(CLOB.written(&clob(10, CharUnits::Octets)), "CHARACTER LARGE OBJECT(10 OCTETS)");
    assert_eq!(BLOB.written(&blob(7)), "BINARY LARGE OBJECT(7)");
    assert_eq!(CLOB.written(&DataType::Boolean), "CHARACTER LARGE OBJECT(0)", "a type of no length");
    let chars = CLOB.descriptor(&clob(10, CharUnits::Characters));
    assert_eq!((chars.length, chars.octets, chars.collated), (Some(10), Some(40), true));
    let octets = CLOB.descriptor(&clob(10, CharUnits::Octets));
    assert_eq!((octets.length, octets.octets), (Some(10), Some(10)));
    let binary = BLOB.descriptor(&blob(5));
    assert_eq!((binary.name.as_str(), binary.length, binary.octets, binary.collated), ("BINARY LARGE OBJECT", Some(5), Some(5), false));
    assert!(CLOB.comparable(&DataType::varying(3)) && !CLOB.comparable(&blob(3)));
    assert!(BLOB.comparable(&blob(3)) && !BLOB.comparable(&DataType::varying(3)));
    assert_eq!((kind_of(LobKind::Binary).name(), kind_of(LobKind::Character).name()), (BLOB.name(), CLOB.name()));
    assert_eq!(BLOB.record_width(&blob(1)), None);
    assert_eq!(CLOB.position(&Value::Lob(Lob::of_text("a"))), None);
    assert!(CLOB.bucket(&Value::Lob(Lob::of_text("a"))).is_none());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.3.3.4, §4.4.2] values held
/// inline compare as strings of their family do, a character string with a
/// character large object padded with spaces; a large object kept in
/// chunks, a value of another family or octets not UTF-8 compare with
/// nothing here, as the executor reads them.
#[test]
fn compared_when_held_inline() {
    let text = |s: &str| Value::Lob(Lob::of_text(s));
    assert_eq!(CLOB.compare(&text("ab"), &Value::Char("ab  ".into())), Some(Ordering::Equal));
    assert_eq!(CLOB.compare(&text("a"), &text("b")), Some(Ordering::Less));
    let octets = |b: &[u8]| Value::Lob(Lob::of_octets(b));
    assert_eq!(BLOB.compare(&octets(&[2]), &Value::Binary(Binary::new(vec![1]))), Some(Ordering::Greater));
    let chunked = Value::Lob(chunked_by("abcdef", 2));
    assert_eq!(CLOB.compare(&chunked, &text("abcdef")), None, "kept in chunks");
    assert_eq!(CLOB.compare(&n(1), &text("1")), None, "a number");
    assert_eq!(BLOB.compare(&text("a"), &octets(b"a")), None, "a character large object");
    let broken = Value::Lob(Lob::of_parts(LobKind::Character, vec![Part::Inline(std::sync::Arc::from(&[0xFF_u8][..]), 1)]));
    assert_eq!(CLOB.compare(&broken, &text("a")), None, "not UTF-8");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.2 GR3e, GR3b xii] store
/// assignment to a large object: one within its length as it is; spaces or
/// X'00' past it cut; any other past it refused, 22001, of a character one
/// counted in characters or in octets, kept inline or in chunks; a value of
/// another family refused, 22005.
#[test]
fn assigned_within_the_length() {
    let t = clob(4, CharUnits::Characters);
    assert_eq!(octets_of(&CLOB.assign(Value::Char("ab".into()), &t, "store").unwrap()), b"ab");
    assert_eq!(octets_of(&CLOB.assign(Value::Char("abcd   ".into()), &t, "store").unwrap()), b"abcd");
    assert_eq!(state(CLOB.assign(Value::Char("abcde".into()), &t, "store")), "22001");
    assert_eq!(state(CLOB.assign(Value::Lob(chunked_by("aaaaaaaaaa", 3)), &t, "store")), "22001", "in chunks");
    let o = clob(5, CharUnits::Octets);
    assert_eq!(octets_of(&CLOB.assign(Value::Char("ab€ ".into()), &o, "store").unwrap()), "ab€".as_bytes());
    assert_eq!(state(CLOB.assign(Value::Char("ab€c".into()), &o, "store")), "22001", "6 octets");
    assert_eq!(state(CLOB.assign(Value::Char("a€€".into()), &clob(6, CharUnits::Octets), "store")), "22001", "a character past 6 octets");
    let b = blob(2);
    assert_eq!(octets_of(&BLOB.assign(Value::Binary(Binary::new(vec![1, 2, 0, 0])), &b, "store").unwrap()), [1, 2]);
    assert_eq!(state(BLOB.assign(Value::Binary(Binary::new(vec![1, 2, 3])), &b, "store")), "22001");
    assert_eq!(state(BLOB.assign(Value::Lob(octets_by(&[1, 2, 3, 4, 5], 2)), &b, "store")), "22001", "in chunks");
    assert_eq!(state(BLOB.assign(Value::Char("a".into()), &b, "store")), "22005");
    assert_eq!(state(CLOB.assign(n(1), &t, "store")), "22005");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.13 GR12, GR13b, SR7, SR8]
/// CAST to a large object: null to null; a string of its family cut, with
/// 01004 if other than spaces of a character one, or any octet of a binary
/// one, was cut; a number, truth value, datetime or interval to a character
/// one as its literal; to a binary one, or another value to either,
/// refused, 22005.
#[test]
fn cast_to_a_large_object() {
    let t = clob(3, CharUnits::Characters);
    assert!(CLOB.cast_warned(Value::Null, &t).unwrap().0.is_null());
    let warned = |k: &dyn SqlType, v: Value, to: &DataType| {
        let (cast, w) = k.cast_warned(v, to).unwrap();
        (octets_of(&cast), w)
    };
    assert_eq!(warned(&CLOB, Value::Char("abc  ".into()), &t), (b"abc".to_vec(), None), "spaces cut");
    assert_eq!(warned(&CLOB, Value::Char("abcd".into()), &t), (b"abc".to_vec(), Some(SqlState::STRING_TRUNCATED)));
    let zero = Value::Binary(Binary::new(vec![1, 0]));
    assert_eq!(warned(&BLOB, zero, &blob(1)), (vec![1], Some(SqlState::STRING_TRUNCATED)), "X'00' cut warns");
    assert_eq!(octets_of(&CLOB.cast(n(42), &clob(10, CharUnits::Characters)).unwrap()), b"42");
    assert_eq!(octets_of(&CLOB.cast(Value::Bool(true), &clob(10, CharUnits::Characters)).unwrap()), b"TRUE");
    assert_eq!(state(CLOB.cast(n(12_345), &clob(3, CharUnits::Characters))), "22001", "a literal longer than the length");
    assert_eq!(state(BLOB.cast(n(1), &blob(5))), "22005");
    assert_eq!(state(CLOB.cast(Value::Binary(Binary::new(vec![1])), &t)), "22005");
    assert_eq!(state(CLOB.cast(Value::Array(vec![n(1)].into()), &t)), "22005");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §5.3] the literal of a large
/// object held inline, a character one's quotes doubled, a binary one in
/// hex; of one kept in chunks, its text alone.
#[test]
fn literals_of_values_held_inline() {
    assert_eq!(CLOB.literal(&Value::Lob(Lob::of_text("it's"))), "'it''s'");
    assert_eq!(BLOB.literal(&Value::Lob(Lob::of_octets(&[0xAB, 1]))), "X'AB01'");
    let chunked = Value::Lob(chunked_by("abcd", 2));
    assert_eq!(CLOB.literal(&chunked), chunked.to_string());
    assert_eq!(CLOB.text(&Value::Lob(Lob::of_text("x"))), "x");
    assert_eq!(BLOB.literal(&n(1)), "1", "not a large object");
}

/// Verifies: [DESIGN] RULES.md D79: a large object's value, record and
/// parameter bytes read back: held inline, by the id its table keeps it by,
/// as a whole slice of a kept source, and as no octets if neither; a
/// record of a value not a large object of the type refused; bytes of an
/// unknown form, or octets not UTF-8 of a character one, malformed; no
/// key of any large object, and no anchor.
#[test]
fn value_and_record_bytes_read_back() {
    let t = clob(9, CharUnits::Octets);
    let mut params = Vec::new();
    CLOB.put_params(&t, &mut params);
    assert_eq!(CLOB.params(&mut Reader::new(&params, "parameters")).unwrap(), t);
    let inline = Value::Lob(Lob::of_text("é"));
    let mut bytes = Vec::new();
    CLOB.put_value(&inline, &mut bytes);
    assert_eq!(octets_of(&CLOB.value(&mut Reader::new(&bytes, "value")).unwrap()), "é".as_bytes());
    let kept = Value::Lob(Lob::of_reference(LobKind::Binary, 7, (300, 300)));
    let record = BLOB.encode_record(&kept, &blob(400)).unwrap();
    let Some(Value::Lob(read)) = BLOB.decode_record(&record, &blob(400)) else { panic!("no large object") };
    assert!(matches!(read.parts(), [Part::Reference(7)]) && read.octets() == 300);
    assert!(BLOB.encode_record(&inline, &blob(4)).is_err(), "a character large object");
    assert!(BLOB.encode_record(&n(1), &blob(4)).is_err(), "a number");
    let mut none = Vec::new();
    put_lob(&n(1), &mut none);
    put_lob(&Value::Lob(chunked_by("abcd", 2)), &mut none);
    let mut r = Reader::new(&none, "none");
    assert_eq!(octets_of(&read_lob(&mut r, LobKind::Character).unwrap()), b"");
    assert_eq!(octets_of(&read_lob(&mut r, LobKind::Character).unwrap()), b"", "a source no table keeps");
    assert!(read_lob(&mut Reader::new(&[9], "tag"), LobKind::Binary).is_err(), "no such form");
    let mut bad = Vec::new();
    put_lob(&Value::Lob(Lob::of_octets(&[0xFF])), &mut bad);
    assert!(read_lob(&mut Reader::new(&bad, "utf-8"), LobKind::Character).is_err(), "not UTF-8");
    assert!(CLOB.decode_record(&bad, &t).is_none());
    assert!(CLOB.encode_key(&inline, &t, &mut Vec::new()).is_err());
    assert!(CLOB.decode_key(&mut &b""[..], &t).is_err());
    assert!(CLOB.anchor(&inline, &t).is_none());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.13 GR11d, GR12d; §9.2
/// GR3b ix 2, GR3b xii 2, GR3e ii] a large object lowered to a string of
/// its family: cut by CAST with 01004, or refused by assignment, 22001, to
/// a fixed or varying string of each family; a character one counted in
/// octets keeps whole characters; to a type of another kind, CAST takes
/// its text past spaces and assignment refuses it; to an array, none.
#[test]
fn lowered_to_strings() {
    let cast = |l: Lob, t: &DataType| {
        let (v, w) = lowered(l, t, Lowering::Cast).unwrap().unwrap();
        (v.to_string(), w)
    };
    let long = Lob::of_text("abcdef");
    assert_eq!(cast(long.clone(), &DataType::varying(4)), ("abcd".to_owned(), Some(SqlState::STRING_TRUNCATED)));
    assert_eq!(state(lowered(long.clone(), &DataType::character(2), Lowering::Assign("store"))), "22001");
    let bin = Lob::of_octets(&[1, 2, 3]);
    assert_eq!(state(lowered(bin.clone(), &DataType::Binary { length: 2 }, Lowering::Assign("store"))), "22001");
    assert_eq!(state(lowered(bin.clone(), &DataType::VarBinary { length: 2 }, Lowering::Assign("store"))), "22001");
    let (cut, w) = lowered(bin, &DataType::VarBinary { length: 2 }, Lowering::Cast).unwrap().unwrap();
    assert_eq!((cut.to_literal(), w), ("X'0102'".to_owned(), Some(SqlState::STRING_TRUNCATED)));
    let octets = |length| DataType::Varying { length, units: CharUnits::Octets };
    assert_eq!(cast(Lob::of_text("a€b"), &octets(4)).0, "a€", "whole characters within 4 octets");
    assert_eq!(cast(Lob::of_text("ab€"), &octets(3)).0, "ab", "a character 3 octets would split");
    assert_eq!(cast(Lob::of_text("  12 "), &DataType::Boolean), ("12".to_owned(), None));
    assert!(lowered(Lob::of_text("1"), &DataType::Boolean, Lowering::Assign("store")).unwrap().is_none());
    assert!(lowered(Lob::of_text("1"), &DataType::array(DataType::Boolean, 2), Lowering::Cast).unwrap().is_none());
    assert!(lowered(Lob::of_octets(&[1]), &DataType::varying(2), Lowering::Cast).unwrap().is_none());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.13 GR6b, GR8b, §5.3] a
/// character large object cast to a type of another kind is trimmed of its
/// spaces, read from each end a chunk at a time, across chunks of spaces
/// alone; all spaces, it is empty; longer than the longest character
/// string past its spaces, it is no literal, 22018.
#[test]
fn trimmed_for_cast_to_another_kind() {
    let spaced = chunked_by(&format!("{}x y{}", " ".repeat(10), " ".repeat(10)), 4);
    assert_eq!(trimmed(&spaced, 100).unwrap().as_deref(), Some("x y"));
    assert_eq!(trimmed(&chunked_by(&" ".repeat(9), 4), 100).unwrap().as_deref(), Some(""));
    assert_eq!(trimmed(&Lob::of_text("abc"), 2).unwrap(), None, "longer than the most");
    let big = Lob::of_text(&"9".repeat(usize::try_from(crate::value::MAX_CHARACTER_LENGTH).unwrap() + 1));
    assert_eq!(state(lowered(big, &DataType::Boolean, Lowering::Cast)), "22018");
    let wide = chunked_by(&"é".repeat(5), 4);
    assert_eq!(whole_chars_within(&wide, 3).unwrap(), 1, "the second é would split");
    assert_eq!(whole_chars_within(&wide, 4).unwrap(), 2);
    assert_eq!(text(&cut_to(wide, 3, true).unwrap().kept), "é");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.3, §4.4] a value of each
/// type is a large object of its family alone: a character string of a
/// character one, a binary string of a binary one, a large object of its
/// kind; of any other value, none.
#[test]
fn values_of_each_family() {
    assert!(as_lob(&Value::Char("a".into()), LobKind::Character).is_some());
    assert!(as_lob(&Value::Char("a".into()), LobKind::Binary).is_none());
    assert!(as_lob(&Value::Binary(Binary::new(vec![1])), LobKind::Binary).is_some());
    assert!(as_lob(&Value::Lob(Lob::of_octets(&[1])), LobKind::Character).is_none());
    for v in [Value::Null, n(1), Value::Bool(true), Value::Array(vec![].into()), Value::Multiset(vec![].into()), Value::Row(vec![].into())]
    {
        assert!(as_lob(&v, LobKind::Character).is_none(), "{v:?}");
    }
}

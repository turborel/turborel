//! Tests of the stored form of a value of the JSON type: its layout, read
//! in place, checked whole, its comparison keys and the paths read on it.

use std::cmp::Ordering;

use super::*;
use crate::json::{Keys, Member, Scalar, parse, parse_path, serialize};
use crate::value::datetime::Clocked;
use crate::value::{DecFloat, Decimal, Value};

mod keys;

/// The stored form of the JSON text `text`.
fn stored(text: &str) -> Vec<u8> {
    encode(&parse(text, Keys::Any).unwrap()).unwrap()
}

/// The JSON text of the stored form `bytes`, read back.
fn text_of(bytes: &[u8]) -> String {
    serialize(&decode(bytes).unwrap()).unwrap()
}

/// How the JSON texts `a` and `b` compare by their stored forms.
fn order(a: &str, b: &str) -> Ordering {
    compare_documents(&stored(a), &stored(b)).unwrap()
}

fn scalar(v: Value) -> Item {
    Item::Scalar(Scalar::new(v).unwrap())
}

/// Verifies: [DESIGN] RULES.md D89: a document is its version,
/// then its root node; scalars, arrays and objects read back as written,
/// an object's members in the order written, of one key too (UA057).
#[test]
fn a_document_reads_back_as_written() {
    for text in ["null", "true", "false", "0", "-12.50", "1.5E+300", "\"x\\u0000y\"", "[]", "{}", "[1,[2,{\"a\":[]}]]"] {
        let bytes = stored(text);
        assert_eq!(bytes.first(), Some(&FORMAT_VERSION));
        assert_eq!(text_of(&bytes), serialize(&parse(text, Keys::Any).unwrap()).unwrap(), "{text}");
    }
    assert_eq!(text_of(&stored("{\"b\":1,\"a\":2,\"b\":3}")), "{\"b\":1,\"a\":2,\"b\":3}");
}

/// Verifies: [DESIGN] RULES.md D89: an exact number is stored
/// in the narrowest of its encodings, a decimal floating-point number and
/// an approximate one as such, each read back equal.
#[test]
fn numbers_keep_their_kind_and_value() {
    let wide = Decimal::new(i128::from(i64::MAX) * 1000, 3);
    for v in [
        Value::Exact(Decimal::new(7, 0)),
        Value::Exact(Decimal::new(-125, 2)),
        Value::Exact(wide),
        Value::Approx(-0.25),
        Value::DecFloat(DecFloat::new(15, -1, 34).unwrap()),
    ] {
        let item = scalar(v.clone());
        let back = decode(&encode(&item).unwrap()).unwrap();
        assert!(back.equivalent(&item).unwrap(), "{v:?}");
        assert_eq!(std::mem::discriminant(back.as_scalar().unwrap().value()), std::mem::discriminant(&v));
    }
    assert_eq!(stored("7").get(1), Some(&INTEGER));
    assert_eq!(stored("1.25").get(1), Some(&DECIMAL));
}

/// Verifies: [DESIGN] RULES.md D89: a node is read in place: an
/// element by its offset, a member by binary search of the key
/// directory, the first written of a key; the root's kind, length and
/// scalar without making an item.
#[test]
fn nodes_are_read_in_place() {
    let bytes = stored("{\"z\":1,\"a\":[10,20,30],\"m\":\"s\",\"a\":2}");
    let root = root(&bytes).unwrap();
    assert_eq!(root.kind().unwrap(), NodeKind::Object);
    assert_eq!(root.len().unwrap(), Some(4));
    let a = root.member("a").unwrap().unwrap();
    assert_eq!(a.kind().unwrap(), NodeKind::Array);
    assert!(matches!(a.element(2).unwrap().unwrap().scalar().unwrap(), Some(Value::Exact(d)) if d.units == 30));
    assert!(a.element(3).unwrap().is_none());
    assert_eq!(root.member("m").unwrap().unwrap().as_str().unwrap(), Some("s"));
    assert!(root.member("b").unwrap().is_none());
    assert!(root.member("zz").unwrap().is_none());
    let keys: Vec<&str> = root.members().unwrap().unwrap().into_iter().map(|(k, _)| k).collect();
    assert_eq!(keys, ["z", "a", "m", "a"]);
    assert!(a.members().unwrap().is_none());
    assert!(root.elements().unwrap().is_none());
    assert!(a.length().unwrap() < bytes.len());
}

/// Verifies: [DESIGN] RULES.md D89, R-ERR-2: damaged octets are
/// refused with SQLCODE -916, never a panic: an unknown version or tag,
/// a cut node, an offset out of its container, trailing octets, an
/// unsorted key directory.
#[test]
fn damaged_documents_are_refused() {
    let good = stored("{\"a\":[1,\"x\"],\"b\":true}");
    assert!(validate(&good).is_ok());
    for cut in 0..good.len() {
        let e = validate(&good[..cut]).unwrap_err();
        assert_eq!(e.sqlcode(), -916_i32, "cut at {cut}");
    }
    let mut longer = good.clone();
    longer.push(0);
    assert_eq!(validate(&longer).unwrap_err().sqlcode(), -916_i32);
    let mut version = good.clone();
    version[0] = FORMAT_VERSION + 1;
    assert_eq!(validate(&version).unwrap_err().sqlcode(), -916_i32);
    let mut tag = good.clone();
    tag[1] = OBJECT + 1;
    assert_eq!(validate(&tag).unwrap_err().sqlcode(), -916_i32);
    // Every single octet flipped is refused or reads as some document.
    for at in 0..good.len() {
        let mut flipped = good.clone();
        flipped[at] ^= 0xFF;
        if validate(&flipped).is_ok() {
            assert!(decode(&flipped).is_ok());
            assert!(document_key(&flipped).is_ok());
        }
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.2 GR10, §4.8.2] the comparison of
/// SQL/JSON values (IA224, IA228, IA229): null before scalars before
/// arrays before objects; numbers by value, before strings by code point,
/// before truth values; arrays element by element; objects by their
/// members ordered by key and value, whatever order they are written in.
#[test]
fn values_compare_by_the_documented_order() {
    let ascending = [
        "null",
        "-1E+10",
        "-1",
        "0",
        "0.5",
        "1",
        "1E+300",
        "\"\"",
        "\"A\"",
        "\"a\"",
        "\"a \"",
        "false",
        "true",
        "[]",
        "[1]",
        "[1,2]",
        "[2]",
        "{}",
        "{\"a\":1}",
        "{\"a\":2}",
        "{\"b\":0}",
    ];
    for pair in ascending.windows(2) {
        assert_eq!(order(pair[0], pair[1]), Ordering::Less, "{pair:?}");
        assert_eq!(order(pair[1], pair[0]), Ordering::Greater, "{pair:?}");
    }
    assert_eq!(order("1", "1.000"), Ordering::Equal);
    assert_eq!(order("100", "1E+2"), Ordering::Equal);
    assert_eq!(order("-0", "0"), Ordering::Equal);
    assert_eq!(order("{\"a\":1,\"b\":2}", "{\"b\":2,\"a\":1}"), Ordering::Equal);
    assert_eq!(order("{\"a\":1,\"a\":2}", "{\"a\":2,\"a\":1}"), Ordering::Equal);
    assert_ne!(order("{\"a\":1,\"a\":1}", "{\"a\":1,\"a\":2}"), Ordering::Equal);
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.48.3] equivalence is by code point:
/// strings that differ by a trailing space are distinct, as keys and as
/// values; members pair off one to one whatever their order.
#[test]
fn equivalence_is_by_code_point_and_one_to_one() {
    let item = |t: &str| parse(t, Keys::Any).unwrap();
    assert!(!item("\"a\"").equivalent(&item("\"a \"")).unwrap());
    assert!(!item("{\"a\":1}").equivalent(&item("{\"a \":1}")).unwrap());
    assert!(item("{\"a\":\"x\",\"a\":\"x \"}").equivalent(&item("{\"a\":\"x \",\"a\":\"x\"}")).unwrap());
    assert!(!item("{\"a\":\"x\",\"a\":\"x\"}").equivalent(&item("{\"a\":\"x \",\"a\":\"x\"}")).unwrap());
}

/// Verifies: [DESIGN] RULES.md D89: a key decodes to an item
/// that compares equal, and keys of distinct values are prefix-free, so
/// a key of several values splits back.
#[test]
fn keys_decode_to_equal_values() {
    for text in ["null", "-12.5", "1E+300", "\"a\\u0000b\"", "true", "[1,[\"x\"]]", "{\"b\":[],\"a\":null}"] {
        let key = document_key(&stored(text)).unwrap();
        let mut both = key.clone();
        both.extend_from_slice(&key);
        let mut rest = both.as_slice();
        let first = item_of_key(&mut rest).unwrap();
        let second = item_of_key(&mut rest).unwrap();
        assert!(rest.is_empty(), "{text}");
        let again = encode(&first).unwrap();
        assert_eq!(compare_documents(&again, &stored(text)).unwrap(), Ordering::Equal, "{text}");
        assert!(first.equivalent(&second).unwrap());
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 GR11g ii] a lax path of members,
/// wildcards and integer subscripts read on the stored form gives what
/// the evaluator gives of the item; any other path is not read so.
#[test]
fn fast_paths_match_the_evaluator() {
    let doc = "{\"a\":[{\"b\":1},{\"b\":2},3],\"c\":{\"d\":\"x\"},\"a\":9}";
    let bytes = stored(doc);
    let item = parse(doc, Keys::Any).unwrap();
    for text in ["lax $", "lax $.a", "lax $.a.b", "lax $.a[*]", "lax $.a[0,2]", "lax $.a[1 to 5]", "lax $.*", "lax $.c[0].d", "lax $.zz"] {
        let path = parse_path(text, Clocked::default()).unwrap();
        let fast = fast_path(&path, root(&bytes).unwrap()).unwrap().expect(text);
        let slow = path.evaluate(&item, &[]).unwrap();
        assert_eq!(fast.len(), slow.len(), "{text}");
        for (f, s) in fast.iter().zip(&slow) {
            assert!(f.equivalent(s).unwrap(), "{text}");
        }
    }
    for text in ["strict $.a", "lax $.a.size()", "lax $.a[last]", "lax $.a ? (@ > 1)", "lax $x"] {
        let path = parse_path(text, Clocked::default()).unwrap();
        assert!(fast_path(&path, root(&bytes).unwrap()).unwrap().is_none(), "{text}");
    }
}

/// Verifies: [DESIGN] RULES.md R-CF-6: nesting beyond `MAX_NESTING` is
/// refused as it is written, SQLCODE -900.
#[test]
fn nesting_is_bounded() {
    let deep = (0..=MAX_NESTING).fold(Item::Null, |inner, _| Item::Array(vec![inner]));
    assert_eq!(encode(&deep).unwrap_err().sqlcode(), -900_i32);
    let within = (0..MAX_NESTING).fold(Item::Null, |inner, _| Item::Object(vec![Member { key: "k".to_owned(), value: inner }]));
    assert!(decode(&encode(&within).unwrap()).unwrap().equivalent(&within).unwrap());
}

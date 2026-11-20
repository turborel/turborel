//! Tests of the byte forms of an XML index's keys.

use super::*;
use crate::index::xml::{Method, VALUE_PREFIX_BYTES};

/// Verifies: [DESIGN] RULES.md D106: ordinals compare as bytes as the
/// numbers they write, their end below every one, and read back.
#[test]
fn ordinals_order_as_numbers() {
    let numbers = [1_u64, 2, 255, 256, 65_535, 65_536, u64::MAX];
    let keys: Vec<Vec<u8>> = numbers.iter().map(|&n| ordinal_of(n)).collect();
    assert!(keys.windows(2).all(|w| w[0] < w[1]), "{keys:?}");
    assert!(vec![END] < keys[0]);
    for (n, key) in numbers.iter().zip(&keys) {
        assert_eq!(Cursor { bytes: key, at: 0 }.ordinal().unwrap(), *n);
    }
}

/// Verifies: [DESIGN] RULES.md D106 [XQueryFO §7.3.1]: string keys order as
/// strings do by code points, a string before every longer one it begins,
/// a whole string's key before a cut one's of the same first octets, an
/// octet 00 escaped; each reads back as one value.
#[test]
fn strings_order_by_code_points() {
    let long = "z".repeat(VALUE_PREFIX_BYTES + 1);
    let prefix = "z".repeat(VALUE_PREFIX_BYTES);
    let texts = ["", "a", "a\0", "ab", "b", &prefix, &long, "é", "\u{10000}"];
    let keys: Vec<Vec<u8>> = texts.iter().map(|t| string_value(t.as_bytes(), VALUE_PREFIX_BYTES)).collect();
    assert!(keys.windows(2).all(|w| w[0] < w[1]), "{keys:?}");
    assert_eq!(
        string_value(long.as_bytes(), VALUE_PREFIX_BYTES),
        string_value(format!("{long}more").as_bytes(), VALUE_PREFIX_BYTES),
        "cut alike"
    );
    for key in &keys {
        let with_more = [key.as_slice(), &[9, 9]].concat();
        assert_eq!(Cursor { bytes: &with_more, at: 0 }.value().unwrap(), key.as_slice());
    }
}

/// Verifies: [DESIGN] RULES.md D106 [XQueryFO §6.3]: number keys order as
/// numbers, -0 as 0, NaN unordered, below every number, and every number
/// below every string.
#[test]
fn numbers_order_as_numbers() {
    let numbers = [f64::NEG_INFINITY, -1e300_f64, -1.5_f64, -f64::MIN_POSITIVE, 0.0_f64, 1e-300_f64, 1.5_f64, 2.0_f64, f64::INFINITY];
    let keys: Vec<Vec<u8>> = numbers.iter().map(|&n| number_value(n)).collect();
    assert!(keys.windows(2).all(|w| w[0] < w[1]), "{keys:?}");
    assert_eq!(number_value(-0.0), number_value(0.0));
    assert_eq!(number_value(f64::NAN), vec![UNORDERED]);
    assert!(number_value(f64::NAN) < keys[0] && keys[8] < string_value(b"", VALUE_PREFIX_BYTES));
}

/// Verifies: [DESIGN] RULES.md D106 [XQuery §3.5.2] [XQueryFO §17.1.1]: a
/// node's number is its value cast as an `xdt:untypedAtomic` to
/// xs:double, white space collapsed, INF and -INF included; none of a
/// value that does not cast, or is cut.
#[test]
fn a_nodes_number_is_its_value_cast() {
    assert_eq!(number_of(b" 12.5 ", true), Some(12.5_f64));
    assert_eq!(number_of(b"-INF", true), Some(f64::NEG_INFINITY));
    assert_eq!(number_of(b"1e3", true), Some(1_000.0_f64));
    assert!(number_of(b"NaN", true).is_some_and(f64::is_nan));
    assert_eq!(number_of(b"twelve", true), None);
    assert_eq!(number_of(b"12", false), None, "cut");
    assert_eq!(values_of(b"12", true)[1], number_value(12.0));
    assert_eq!(values_of(b"x", true)[1], vec![UNORDERED]);
}

/// Verifies: [DESIGN] RULES.md D106 [Pal et al. §3.2.1-§3.2.3]: each
/// method's entry reads back as the path, value and row it was made of,
/// in its own order of fields.
#[test]
fn entries_read_back() {
    let value = string_value(b"x\0y", VALUE_PREFIX_BYTES);
    let ordpath = [ordinal_of(1), ordinal_of(5)].concat();
    let node = Node { path: 300, kind: ELEMENT, value: &value, ordpath: &ordpath };
    for method in [Method::Path, Method::Value, Method::Property, Method::Primary] {
        let key = entry(method, &node, 77);
        assert_eq!(parse_entry(method, &key).unwrap(), (300, value.as_slice(), 77), "{method:?}");
    }
    assert_eq!(entry(Method::Value, &node, 77)[1], STRING, "a value first");
    assert_eq!(parse_entry(Method::Path, &[ENTRIES, 9]).unwrap_err().sqlcode(), -916_i32);
    assert_eq!(parse_entry(Method::Path, &[NAMES]).unwrap_err().sqlcode(), -916_i32);
}

/// Verifies: [DESIGN] RULES.md D106: an ORDPATH is read to its end, and an
/// entry's count is one when its value is empty.
#[test]
fn ordpaths_and_counts_read_back() {
    let key = [ordinal_of(1), ordinal_of(3), vec![END], vec![42]].concat();
    let mut at = Cursor { bytes: &key, at: 0 };
    assert_eq!(at.ordpath().unwrap(), &key[..4]);
    assert_eq!(at.byte().unwrap(), 42);
    assert_eq!((count_of(&counted(1)).unwrap(), count_of(&counted(7)).unwrap()), (1, 7));
    assert_eq!(count_of(&[1]).unwrap_err().sqlcode(), -916_i32);
    assert_eq!(after(&[1, 0xFF]), Bound::Excluded(vec![2]));
    assert_eq!(after(&[0xFF]), Bound::Unbounded);
}

/// Checks `result` is the error of a malformed key, SQLCODE -916.
fn malformed<T: std::fmt::Debug>(what: &str, result: Result<T>) {
    assert_eq!(result.expect_err(what).sqlcode(), -916_i32, "{what}");
}

/// Verifies: [DESIGN] RULES.md D106: an entry of each method cut short
/// anywhere within its fields, a string value or a pathed entry cut
/// short, a value of no kind, an ordinal longer than eight octets and a
/// string of a bad end are each a malformed key, SQLCODE -916, never a
/// panic.
#[test]
fn keys_cut_short_are_malformed() {
    let value = string_value(b"x\0y", VALUE_PREFIX_BYTES);
    let number = number_value(2.5);
    let ordpath = [ordinal_of(1), ordinal_of(300)].concat();
    for v in [&value, &number] {
        let node = Node { path: 70_000, kind: ELEMENT, value: v, ordpath: &ordpath };
        for method in [Method::Path, Method::Value, Method::Property, Method::Primary] {
            let key = entry(method, &node, 77);
            let fields_end = if method == Method::Primary { key.len() } else { key.len().saturating_sub(ordpath.len().saturating_add(1)) };
            for cut in 0..fields_end {
                malformed(&format!("{method:?} cut at {cut}"), parse_entry(method, &key[..cut]));
            }
        }
        let pathed_key = pathed(&node, 77);
        let at = [vec![PATHED], 77_u64.to_be_bytes().to_vec(), ordinal_of(70_000)].concat().len();
        assert_eq!(pathed_fields(&pathed_key, at).unwrap(), (v.as_slice(), ordpath.as_slice()));
        for cut in at..pathed_key.len() {
            malformed(&format!("pathed cut at {cut}"), pathed_fields(&pathed_key[..cut], at));
        }
    }
    for cut in 1..value.len() {
        malformed(&format!("text cut at {cut}"), text_of(&value[..cut]));
    }
    malformed("a number's text", text_of(&number));
    malformed("a bad end", text_of(&[STRING, b'a', 0, 7]));
    malformed("a value of no kind", Cursor { bytes: &[9], at: 0 }.value());
    malformed("an ordinal of nine octets", Cursor { bytes: &[9, 1, 2, 3, 4, 5, 6, 7, 8, 9], at: 0 }.ordinal());
    malformed("past the end", Cursor { bytes: &[1], at: 1 }.take(usize::MAX));
}

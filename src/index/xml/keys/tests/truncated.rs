//! Robustness tests of an XML index's keys: each entry and value key cut
//! short is refused, or reads the fields its uncut entry holds.

use super::*;

/// An entry's path, value key and row.
type Fields = (u64, Vec<u8>, RowId);

/// The entries of every method of a node of string, number and unordered
/// values, each with the fields it was made of.
fn entries() -> Vec<(Method, Vec<u8>, Fields)> {
    let ordpath = [ordinal_of(1), ordinal_of(300)].concat();
    let mut all = Vec::new();
    for value in [string_value(b"a\0b", VALUE_PREFIX_BYTES), number_value(-2.5), vec![UNORDERED]] {
        let node = Node { path: 70_000, kind: ELEMENT, value: &value, ordpath: &ordpath };
        for method in [Method::Path, Method::Value, Method::Property, Method::Primary] {
            all.push((method, entry(method, &node, 9), (70_000, value.clone(), 9)));
        }
    }
    all
}

/// Verifies: [DESIGN] RULES.md D106, R-ERR-2: an entry of every method cut
/// at any byte is refused with SQLCODE -916, or, cut past the fields it
/// is read for, reads those fields as the whole entry does.
#[test]
fn every_entry_prefix_is_refused_or_its_fields() {
    for (method, key, (path, value, row)) in entries() {
        assert_eq!(parse_entry(method, &key).unwrap(), (path, value.as_slice(), row), "{method:?}");
        for cut in 0..key.len() {
            match parse_entry(method, &key[..cut]) {
                Ok(fields) => assert_eq!(fields, (path, value.as_slice(), row), "{method:?} cut at {cut}"),
                Err(e) => assert_eq!(e.sqlcode(), -916_i32),
            }
        }
    }
}

/// Verifies: [DESIGN] RULES.md D106, R-ERR-2: a string value key cut short
/// is refused, as is a 0 followed by a mark no string writes; a key of a
/// number is no string's; the primary's entry by path and value reads
/// back its value and ORDPATH, and cut short is refused.
#[test]
fn value_keys_and_pathed_entries_are_checked() {
    let whole = string_value(b"x\0y", VALUE_PREFIX_BYTES);
    assert_eq!(text_of(&whole).unwrap(), (b"x\0y".to_vec(), true));
    for cut in 0..whole.len() {
        assert_eq!(text_of(&whole[..cut]).unwrap_err().sqlcode(), -916_i32, "cut at {cut}");
    }
    assert_eq!(text_of(&[STRING, b'a', 0, 0x7F]).unwrap_err().sqlcode(), -916_i32);
    assert_eq!(text_of(&number_value(1.0)).unwrap_err().sqlcode(), -916_i32);
    let ordpath = [ordinal_of(2), ordinal_of(4)].concat();
    let value = number_value(3.0);
    let key = pathed(&Node { path: 5, kind: ELEMENT, value: &value, ordpath: &ordpath }, 11);
    let after_path = 1 + ROW + ordinal_of(5).len();
    assert_eq!(pathed_fields(&key, after_path).unwrap(), (value.as_slice(), ordpath.as_slice()));
    for cut in after_path..key.len() {
        assert_eq!(pathed_fields(&key[..cut], after_path).unwrap_err().sqlcode(), -916_i32, "cut at {cut}");
    }
    let mut bad_value = key.clone();
    bad_value[after_path] = 0xEE;
    assert_eq!(pathed_fields(&bad_value, after_path).unwrap_err().sqlcode(), -916_i32);
    let mut long_ordinal = key;
    long_ordinal[after_path + 1 + DOUBLE] = 9;
    assert_eq!(pathed_fields(&long_ordinal, after_path).unwrap_err().sqlcode(), -916_i32);
}

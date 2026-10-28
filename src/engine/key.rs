//! \[DESIGN\] Index keys as bytes (RULES.md D31): the values of a key's
//! columns encoded so that comparing the bytes, as unsigned bytes from the
//! first, orders keys as SQL compares their values [X3.135-1986 §5.11], so a
//! B+tree compares keys without decoding them.
//!
//! ```text
//! key:       each column's value, in order
//! NULL:      0x00 (before every value)
//! value:     0x01, then
//!   exact:       the units at the column's scale as i64, sign bit flipped,
//!                big-endian
//!   approximate: the IEEE-754 bits, all inverted if negative, else the
//!                sign bit set, big-endian (-0.0 as 0.0)
//!   character:   the UTF-8 of the value padded with spaces to the column's
//!                length [X3.135-1986 §5.11 GR5]
//! ```
//!
//! Character strings compare in Unicode scalar order [X3.135-1986 §5.11 GR6,
//! implementor-defined], which is the order of their UTF-8 bytes. Padded to
//! the same number of characters, no value's bytes are a prefix of another
//! value's, so the columns of a composite key are simply concatenated, and
//! a key is decoded again by its columns' types (`decode_key`).
//!
//! A value compared with a column's keys need not be of the column's type:
//! `1.555` with a column of scale 2, a string longer than the column. Its
//! anchor (`anchor`) is the key of the greatest value of the column's type
//! not above it, so every other key compares with the value as with the
//! anchor, and only the anchor's own key needs comparing with the value.

use crate::error::{Code, Result, SqlError};
use crate::value::types::{malformed, take};
use crate::value::{DataType, Value};

/// The tags before a column's value.
const NULL: u8 = 0;
const PRESENT: u8 = 1;

/// Appends the key of `values`, the values of columns of `types`, to `out`.
///
/// # Errors
///
/// SQLCODE -910 if there is not one value for each type, a value is not of
/// its column's type, or a string is longer than its column.
pub fn encode_key(values: &[Value], types: &[DataType], out: &mut Vec<u8>) -> Result<()> {
    if values.len() != types.len() {
        return Err(SqlError::new(Code::Api, format!("[DESIGN] a key of {} values for an index on {} columns", values.len(), types.len())));
    }
    for (value, &data_type) in values.iter().zip(types) {
        encode_column(value, data_type, out)?;
    }
    Ok(())
}

/// The key of one column: its kind's, after a tag.
fn encode_column(value: &Value, data_type: DataType, out: &mut Vec<u8>) -> Result<()> {
    if value.is_null() {
        out.push(NULL);
        return Ok(());
    }
    out.push(PRESENT);
    data_type.encode_key(value, out)
}

/// The values of the key `bytes`, of columns of `types`; a character value
/// padded to its column's length.
///
/// # Errors
///
/// SQLCODE -916 if `bytes` is not a key of columns of `types`.
pub fn decode_key(bytes: &[u8], types: &[DataType]) -> Result<Vec<Value>> {
    let mut rest = bytes;
    let values = types.iter().map(|&t| decode_column(&mut rest, t)).collect::<Result<Vec<_>>>()?;
    if rest.is_empty() { Ok(values) } else { Err(malformed()) }
}

/// The value of one column, from the start of `rest`.
fn decode_column(rest: &mut &[u8], data_type: DataType) -> Result<Value> {
    match take(rest, 1)? {
        [NULL] => Ok(Value::Null),
        [PRESENT] => data_type.decode_key(rest),
        _ => Err(malformed()),
    }
}

/// The key of a value's anchor, and whether it is the value's own key.
#[derive(Debug, PartialEq, Eq)]
pub struct Anchor {
    /// The key.
    pub key: Vec<u8>,
    /// Whether it is the key of the value itself, so that keys compare
    /// with the value as with it, its own included.
    pub exact: bool,
}

/// \[DESIGN\] The anchor of `value` among the keys of a column of
/// `data_type` (see the module's documentation): the key of `value` if it
/// is of the column's type; for an exact number, of it rounded down at the
/// column's scale, within 64 bits; for a string, of it cut to the column's
/// length. None for NULL or a value of another kind, which every key must
/// be compared with.
pub fn anchor(value: &Value, data_type: DataType) -> Option<Anchor> {
    if value.is_null() {
        return None;
    }
    let (anchored, exact) = data_type.kind().anchor(value, &data_type)?;
    let mut key = Vec::new();
    encode_column(&anchored, data_type, &mut key).ok()?;
    Some(Anchor { key, exact })
}

// Unit tests: keys sort as SQL compares their values (RULES.md R-TEST-1,
// R-TEST-2).
#[cfg(test)]
mod tests {
    use std::cmp::Ordering;

    use super::*;
    use crate::value::{Decimal, compare};

    fn key(values: &[Value], types: &[DataType]) -> Vec<u8> {
        let mut out = Vec::new();
        encode_key(values, types, &mut out).unwrap();
        out
    }

    /// Checks that every pair of `values`, of a column of `data_type`, has
    /// keys in the order SQL compares them, equal keys for equal values.
    fn keys_sort_as_values(data_type: DataType, values: &[Value]) {
        for a in values {
            for b in values {
                let sql = compare(a, b).unwrap();
                let bytes = key(std::slice::from_ref(a), &[data_type]).cmp(&key(std::slice::from_ref(b), &[data_type]));
                assert_eq!(bytes, sql, "{a:?} against {b:?}");
            }
        }
    }

    fn exact(units: i128, scale: u32) -> Value {
        Value::Exact(Decimal::new(units, scale))
    }

    /// Verifies: [X3.135-1986 §5.11]; [DESIGN] RULES.md D31, key encoding:
    /// exact keys sort as their values, negatives first, at the extremes of
    /// 18 digits, values of a smaller scale scaled to the column's.
    #[test]
    fn exact_keys_sort_as_numbers() {
        let values = [
            exact(-999_999_999_999_999_999, 2),
            exact(-1, 0),
            exact(-1, 2),
            exact(0, 0),
            exact(1, 2),
            exact(15, 1),
            exact(150, 2),
            exact(2, 0),
            exact(999_999_999_999_999_999, 2),
        ];
        keys_sort_as_values(DataType::Exact { precision: 18, scale: 2 }, &values);
    }

    /// Verifies: [X3.135-1986 §5.11]; [DESIGN] RULES.md D31, key encoding:
    /// approximate keys sort as their values, from negative infinity to
    /// positive, -0.0 and 0.0 as one key.
    #[test]
    fn approximate_keys_sort_as_numbers() {
        let values: [f64; 10] =
            [f64::NEG_INFINITY, -1e300, -2.5, -f64::MIN_POSITIVE, -0.0, 0.0, f64::MIN_POSITIVE, 1.0, 1e300, f64::INFINITY];
        keys_sort_as_values(DataType::Approximate { precision: 53 }, &values.map(Value::Approx));
    }

    /// Verifies: [X3.135-1986 §5.11 GR5, GR6]; [DESIGN] RULES.md D31, key
    /// encoding: character keys sort as SQL compares strings, the shorter
    /// padded with spaces, in Unicode scalar order: empty strings, inner and
    /// trailing spaces, characters before the space, and characters of
    /// several bytes.
    #[test]
    fn character_keys_sort_as_padded_strings() {
        let values = ["", " ", "a", "a ", "a\u{1}", "a b", "ab", "abcd", "b", "é", "€", "\u{10FFFF}"];
        keys_sort_as_values(DataType::Character { length: 4 }, &values.map(|s| Value::Char(s.into())));
    }

    /// Verifies: [DESIGN] RULES.md D31, key encoding: NULL sorts before every
    /// value; a composite key sorts by its first column, then its second;
    /// a key of the wrong number of values, of another type, or longer than
    /// its column is refused with SQLCODE -910.
    #[test]
    fn composite_keys_and_refusals() {
        let types = [DataType::Character { length: 2 }, DataType::Exact { precision: 4, scale: 0 }];
        let k = |s: &str, n: i128| key(&[Value::Char(s.into()), exact(n, 0)], &types);
        assert_eq!(key(&[Value::Null, exact(9, 0)], &types).cmp(&k("", -9)), Ordering::Less);
        assert!(k("a", 9) < k("b", -9) && k("a", -1) < k("a", 1) && k("é", 0) > k("z", 0));
        let refused = |values: &[Value]| encode_key(values, &types, &mut Vec::new()).err().map(|e| e.code);
        assert_eq!(refused(&[Value::Char("a".into())]), Some(Code::Api));
        assert_eq!(refused(&[exact(1, 0), exact(1, 0)]), Some(Code::Api));
        assert_eq!(refused(&[Value::Char("abc".into()), exact(1, 0)]), Some(Code::Api));
    }

    /// Verifies: [DESIGN] RULES.md D31, key encoding: a key decodes to its
    /// values, strings padded to their column, -0.0 as 0.0, NULL as NULL;
    /// bytes that are not a key of the columns are refused with SQLCODE
    /// -916.
    #[test]
    fn keys_decode_to_their_values() {
        let types =
            [DataType::Character { length: 3 }, DataType::Exact { precision: 9, scale: 2 }, DataType::Approximate { precision: 53 }];
        for values in [
            [Value::Char("é€".into()), exact(-12_345, 2), Value::Approx(-2.5)],
            [Value::Null, exact(7, 0), Value::Approx(-0.0)],
            [Value::Char(String::new()), Value::Null, Value::Null],
        ] {
            let decoded = decode_key(&key(&values, &types), &types).unwrap();
            let shown = |v: &[Value]| v.iter().map(|v| format!("{:?}", v.norm())).collect::<Vec<_>>();
            assert_eq!(shown(&decoded), shown(&values));
        }
        assert_eq!(format!("{:?}", decode_key(&key(&[Value::Char("a".into())], &types[..1]), &types[..1]).unwrap()), "[Char(\"a  \")]");
        let k = key(&[Value::Char("ab".into()), exact(1, 0), Value::Approx(1.0)], &types);
        let cut = k[..k.len() - 1].to_vec();
        let longer = [k.as_slice(), &[0]].concat();
        // A word cut short, a byte too many, an unknown tag, a string not UTF-8.
        for (bad, columns) in [(cut, &types[..]), (longer, &types[..]), (vec![2], &types[..1]), (vec![1, 0xFF, 0xFE, 0x20], &types[..1])] {
            assert_eq!(decode_key(&bad, columns).err().map(|e| e.code), Some(Code::Corrupt), "{bad:?}");
        }
    }

    /// Verifies: [X3.135-1986 §5.11]; [DESIGN] RULES.md D31, anchors: every
    /// key other than a value's anchor compares with the value as with the
    /// anchor: a number of a finer scale rounded down, beyond 64 bits held
    /// at the extremes; a string cut to the column's length; NULL and values
    /// of another kind have none.
    #[test]
    fn anchors_order_keys_as_their_values() {
        let check = |data_type: DataType, keys: &[Value], probes: &[Value]| {
            for (k, probe) in probes.iter().flat_map(|p| keys.iter().map(move |k| (k, p))) {
                let at = anchor(probe, data_type).unwrap();
                let bytes = key(std::slice::from_ref(k), &[data_type]);
                // Only an inexact anchor's own key may compare otherwise.
                if bytes != at.key || at.exact {
                    assert_eq!(bytes.cmp(&at.key), compare(k, probe).unwrap(), "{k:?} against {probe:?}");
                }
            }
        };
        let money = DataType::Exact { precision: 9, scale: 2 };
        let keys: Vec<Value> = [-1_001, -1_000, -999, -1, 0, 1, 155, 156, 1_000].map(|u| exact(u, 2)).to_vec();
        check(
            money,
            &keys,
            &[
                exact(1_555, 3),
                exact(-1_555, 3),
                exact(-10_001, 4),
                exact(1, 50),
                exact(-1, 50),
                exact(i128::MAX, 0),
                exact(i128::MIN, 0),
                exact(i128::MAX, 1),
                exact(155, 2),
                exact(15_500, 4),
            ],
        );
        let exactness = |value: &Value, data_type| anchor(value, data_type).map(|a| a.exact);
        assert_eq!((exactness(&exact(15_500, 4), money), exactness(&exact(15_501, 4), money)), (Some(true), Some(false)));
        let short = DataType::Character { length: 3 };
        let words: Vec<Value> = ["ab", "abc", "abd", "b"].map(|s| Value::Char(s.into())).to_vec();
        check(short, &words, &["abcd", "abc   ", "aaaa", "zzzz", "ab", "abc"].map(|s| Value::Char(s.into())));
        let real = DataType::Approximate { precision: 53 };
        assert_eq!(anchor(&Value::Approx(1.5), real), Some(Anchor { key: key(&[Value::Approx(1.5)], &[real]), exact: true }));
        for (value, data_type) in
            [(Value::Null, money), (Value::Approx(1.0), money), (exact(1, 0), short), (Value::Char("a".into()), money)]
        {
            assert_eq!(anchor(&value, data_type), None, "{value:?}");
        }
    }
}

//! The contract of a data type's kind, `SqlType` (RULES.md R-SAFE-3, D53),
//! checked for every built-in kind; a plug-in type can be checked the same
//! way.

use std::cmp::Ordering;

use turborel::bytes::Reader;
use turborel::value::{Bits, DataType, Decimal, SqlType, Value, assign, bucket, compare};

/// Checks the contract of `t`'s kind on `t` and `values`, which are
/// assigned to `t` first.
pub(crate) fn type_contract(t: DataType, values: &[Value]) {
    let kind: &dyn SqlType = t.kind();
    assert!(kind.comparable(&t), "{t} is comparable with itself");
    let mut params = Vec::new();
    kind.put_params(&t, &mut params);
    assert_eq!(kind.params(&mut Reader::new(&params, "parameters")).unwrap(), t, "{t}'s parameters read back");
    let values: Vec<Value> = values.iter().map(|v| assign(v.clone(), &t, "contract").unwrap()).collect();
    let same = |a: &Value, b: &Value| compare(a, b) == Some(Ordering::Equal) && a.to_literal() == b.to_literal();
    for a in &values {
        let again = assign(a.clone(), &t, "contract").unwrap();
        assert!(same(&again, a), "assigning {a} to {t} again gives it");
        let mut bytes = Vec::new();
        kind.put_value(a, &mut bytes);
        assert!(same(&kind.value(&mut Reader::new(&bytes, "value")).unwrap(), a), "{a}'s bytes read back");
        let record = kind.encode_record(a, &t).unwrap();
        if let Some(width) = kind.record_width(&t) {
            assert_eq!(record.len(), width, "{a}'s record bytes are {t}'s width");
        }
        assert!(same(&kind.decode_record(&record, &t).unwrap(), a), "{a}'s record bytes read back");
        let mut key = Vec::new();
        kind.encode_key(a, &t, &mut key).unwrap();
        let mut rest = key.as_slice();
        let read = kind.decode_key(&mut rest, &t).unwrap();
        assert_eq!(compare(&read, a), Some(Ordering::Equal), "{a}'s key reads back a value equal to it");
        assert!(rest.is_empty(), "{a}'s key is read whole");
        let (anchor, exact) = kind.anchor(a, &t).unwrap();
        assert!(exact && same(&anchor, a), "{a} is its own anchor");
        for b in &values {
            let mut other = Vec::new();
            kind.encode_key(b, &t, &mut other).unwrap();
            assert_eq!(Some(key.cmp(&other)), compare(a, b), "the keys of {a} and {b} order as they compare");
            if compare(a, b) == Some(Ordering::Equal) {
                assert_eq!(bucket(a), bucket(b), "{a} and {b} share a bucket");
            }
        }
    }
}

fn exact(units: i128, scale: u32) -> Value {
    Value::Exact(Decimal::new(units, scale))
}

/// Verifies: [DESIGN] RULES.md R-SAFE-3, D53: CHARACTER keeps the
/// `SqlType` contract: on strings empty, short, of trailing spaces, of
/// characters below the space and beyond ASCII.
#[test]
fn character_keeps_the_type_contract() {
    let strings = ["", "a", "a  ", "ab", "\t", "z", "é", "Ω€"];
    type_contract(DataType::Character { length: 5 }, &strings.map(|s| Value::Char(s.into())));
}

/// Verifies: [DESIGN] RULES.md R-SAFE-3, D53; [ISO/IEC 9075:1992 §8.2 GR3a]
/// CHARACTER VARYING keeps the `SqlType` contract: on strings empty, of
/// spaces, of trailing spaces, of characters below and above the space,
/// beyond ASCII, longer than a key's chunk.
#[test]
fn varying_keeps_the_type_contract() {
    let strings = ["", "   ", "a", "a  ", "a\t", "a!", "ab", "\u{0}", "é", "Ω€", "abcdefgh", "abcdefghi", "abcdefgh  x", "abcdefgh\t"];
    type_contract(DataType::Varying { length: 12 }, &strings.map(|s| Value::Char(s.into())));
}

/// Bit strings of the 0s and 1s of `digits`.
fn bits<const N: usize>(digits: [&str; N]) -> Vec<Value> {
    digits.iter().map(|d| Value::Bits(Bits::of_bits(d).unwrap())).collect()
}

/// Verifies: [DESIGN] RULES.md R-SAFE-3, D53; [ISO/IEC 9075:1992 §8.2 GR4]
/// BIT keeps the `SqlType` contract: on bit strings of its length, all 0s,
/// all 1s, of a last byte part used.
#[test]
fn bit_keeps_the_type_contract() {
    type_contract(DataType::Bit { length: 10 }, &bits(["0000000000", "1111111111", "1010101010", "0000000001", "1000000000"]));
}

/// Verifies: [DESIGN] RULES.md R-SAFE-3, D53; [ISO/IEC 9075:1992 §8.2 GR4]
/// BIT VARYING keeps the `SqlType` contract: on bit strings empty, of one
/// bit, of a byte, of a byte and part of another, each beginning others.
#[test]
fn bit_varying_keeps_the_type_contract() {
    let strings = ["", "0", "1", "00", "01", "10", "11111111", "111111110", "111111111", "000000001", "101010101010"];
    type_contract(DataType::BitVarying { length: 12 }, &bits(strings));
}

/// Verifies: [DESIGN] RULES.md R-SAFE-3, D53: NUMERIC keeps the `SqlType`
/// contract: on numbers negative, zero, positive, of fewer fraction digits
/// than the scale, at the precision's ends.
#[test]
fn exact_numbers_keep_the_type_contract() {
    let numbers = [exact(-999_999_999, 2), exact(-5, 1), exact(0, 0), exact(1, 2), exact(42, 0), exact(999_999_999, 2)];
    type_contract(DataType::Exact { precision: 9, scale: 2 }, &numbers);
}

/// Verifies: [DESIGN] RULES.md R-SAFE-3, D53: FLOAT keeps the `SqlType`
/// contract, at double and single precision: on numbers of either sign,
/// both zeros, and the largest.
#[test]
fn approximate_numbers_keep_the_type_contract() {
    let numbers = [-f64::MAX, -1.5, -0.0, 0.0, 2.5e-300, 1.0e300, f64::MAX].map(Value::Approx);
    type_contract(DataType::Approximate { precision: 53 }, &numbers);
    type_contract(DataType::Approximate { precision: 24 }, &[-3.5, 0.0, 0.1, 1.0e30].map(Value::Approx));
}

/// Verifies: [DESIGN] RULES.md R-SAFE-3, D53: a type beyond the standard,
/// a plug-in's, keeps the `SqlType` contract: the tests' IDENT.
#[test]
fn a_plug_in_type_keeps_the_type_contract() {
    let ids = [[0_u8; 16], [1; 16], [0xff; 16], [0x7f; 16]].map(crate::ident::ident);
    type_contract(crate::ident::ident_type(), &ids);
}

//! Unit tests of normalized keys (`spill.rs`).

use super::{norm_key, put_norm};
use crate::value::{Bits, Decimal, Value};

/// Verifies: [DESIGN] RULES.md D38, normalized keys: a value's key made in
/// place (`put_norm`) is the bytes of the key of its normalized value
/// (`norm_key`), for nulls, texts with and without trailing spaces, exact
/// numbers with and without trailing zeros, approximate zeros of either
/// sign and other numbers, and bit strings; and values the comparison
/// rules find equal have one key.
#[test]
fn keys_made_in_place_are_normalized_keys() {
    let values = [
        Value::Null,
        Value::Char("ab".into()),
        Value::Char("ab   ".into()),
        Value::Char(String::new()),
        Value::Exact(Decimal::new(150, 2)),
        Value::Exact(Decimal::new(15, 1)),
        Value::Exact(Decimal::new(-7, 0)),
        Value::Approx(0.0),
        Value::Approx(-0.0),
        Value::Approx(1.5),
        Value::Bits(Bits::of_bits("101").unwrap()),
    ];
    for v in &values {
        let mut made = Vec::new();
        put_norm(v, &mut made);
        assert_eq!(made, norm_key(&[v.norm()]), "{v:?}");
    }
    let key = |v: &Value| {
        let mut out = Vec::new();
        put_norm(v, &mut out);
        out
    };
    assert_eq!(key(&values[1]), key(&values[2]), "trailing spaces");
    assert_eq!(key(&values[4]), key(&values[5]), "trailing zeros");
    assert_eq!(key(&values[7]), key(&values[8]), "zeros of either sign");
    assert_ne!(key(&values[0]), key(&values[3]), "null is not empty");
}

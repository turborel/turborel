//! Unit tests of the JSON table's private rules.

use super::*;
use crate::json::Scalar;

/// Verifies: [ISO/IEC 9075-2:2023 §9.43 GR2a ii; §7.11 GR1e ii 4 A II]
/// An item has a JSON text unless it is or holds an approximate number
/// that is not finite, at any depth.
#[test]
fn chaining_item_has_a_json_text() {
    let string = Item::Scalar(Scalar::from(String::from("x")));
    assert!(has_text(&Item::Null));
    assert!(has_text(&Item::Array(vec![string.clone(), Item::Null])));
    let infinite = Scalar::new(Value::Approx(f64::INFINITY)).map(Item::Scalar).unwrap();
    assert!(!has_text(&infinite));
    let nested = Item::Object(vec![crate::json::Member { key: "a".to_owned(), value: Item::Array(vec![infinite]) }]);
    assert!(!has_text(&nested));
    assert!(has_text(&string));
}

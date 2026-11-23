//! Tests of the SQL/JSON data model: what an item is, and which are
//! equivalent.

use super::*;
use crate::value::{Binary, Decimal, Value};

pub(super) fn number(units: i128, scale: u32) -> Item {
    Item::Scalar(Scalar::new(Value::Exact(Decimal::new(units, scale))).unwrap())
}

pub(super) fn string(s: &str) -> Item {
    Item::Scalar(Scalar::new(Value::Char(s.into())).unwrap())
}

pub(super) fn truth(b: bool) -> Item {
    Item::Scalar(Scalar::new(Value::Bool(b)).unwrap())
}

pub(super) fn object(members: &[(&str, Item)]) -> Item {
    Item::Object(members.iter().map(|(key, value)| Member { key: (*key).to_owned(), value: value.clone() }).collect())
}

/// `item` within `levels` arrays.
pub(super) fn nested(item: Item, levels: usize) -> Item {
    (0..levels).fold(item, |inner, _| Item::Array(vec![inner]))
}

fn equivalent(a: &Item, b: &Item) -> bool {
    a.equivalent(b).unwrap()
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.48.3] an SQL/JSON scalar
/// is a non-null character string, number, truth value or datetime: the
/// null value and a binary string are none.
#[test]
fn a_scalar_is_of_the_types_the_model_names() {
    assert!(Scalar::new(Value::Char("a".into())).is_some());
    assert!(Scalar::new(Value::Approx(1.5)).is_some());
    assert!(Scalar::new(Value::Bool(true)).is_some());
    assert!(Scalar::new(Value::Null).is_none());
    assert!(Scalar::new(Value::Binary(Binary::default())).is_none());
    let kept = Scalar::new(Value::Bool(true)).unwrap();
    assert!(matches!(kept.value(), Value::Bool(true)));
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.48.3] scalars are
/// equivalent when equal as SQL values, 1 and 1.0 so, a number and a
/// string never; the SQL/JSON null is equivalent to itself alone.
#[test]
fn scalars_and_nulls_are_equivalent_as_their_values_are_equal() {
    assert!(equivalent(&number(1, 0), &number(10, 1)));
    assert!(!equivalent(&number(1, 0), &number(2, 0)));
    assert!(!equivalent(&number(1, 0), &string("1")));
    assert!(equivalent(&truth(true), &truth(true)));
    assert!(!equivalent(&truth(true), &truth(false)));
    assert!(equivalent(&Item::Null, &Item::Null));
    assert!(!equivalent(&Item::Null, &number(0, 0)));
    assert!(!equivalent(&number(0, 0), &Item::Null));
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.48.3] arrays are
/// equivalent when of one length with corresponding elements equivalent,
/// so their order matters; an array is equivalent to no scalar or object.
#[test]
fn arrays_are_equivalent_element_by_element() {
    let ab = Item::Array(vec![number(1, 0), string("b")]);
    assert!(equivalent(&ab, &Item::Array(vec![number(1, 0), string("b")])));
    assert!(!equivalent(&ab, &Item::Array(vec![string("b"), number(1, 0)])));
    assert!(!equivalent(&ab, &Item::Array(vec![number(1, 0)])));
    assert!(equivalent(&Item::Array(Vec::new()), &Item::Array(Vec::new())));
    assert!(!equivalent(&Item::Array(Vec::new()), &object(&[])));
    assert!(!equivalent(&ab, &number(1, 0)));
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.48.3] objects are
/// equivalent when their members pair off by equal keys and equivalent
/// bound values, whatever their order; keys are equal code point by code
/// point, so one of another case or a trailing space is another key.
#[test]
fn objects_are_equivalent_whatever_their_members_order() {
    let ab = object(&[("a", number(1, 0)), ("b", Item::Null)]);
    assert!(equivalent(&ab, &object(&[("b", Item::Null), ("a", number(1, 0))])));
    assert!(!equivalent(&ab, &object(&[("a", number(1, 0))])));
    assert!(!equivalent(&ab, &object(&[("a", number(1, 0)), ("b", number(1, 0))])));
    assert!(!equivalent(&ab, &object(&[("A", number(1, 0)), ("b", Item::Null)])));
    assert!(!equivalent(&ab, &object(&[("a ", number(1, 0)), ("b", Item::Null)])));
    assert!(!equivalent(&ab, &Item::Array(Vec::new())));
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.48.3] the bijection of
/// objects with members of one key: each member is paired with one member
/// only, so {a:1, a:1} is not {a:1, a:2}, and {a:1, a:2} is {a:2, a:1}.
/// The pairing is found by the items' comparison keys, members ordered by
/// key and value (`binary::document_key`), so it needs no transitivity.
#[test]
fn members_of_one_key_pair_one_to_one() {
    let twice = object(&[("a", number(1, 0)), ("a", number(1, 0))]);
    let both = object(&[("a", number(1, 0)), ("a", number(2, 0))]);
    assert!(!equivalent(&twice, &both));
    assert!(!equivalent(&both, &twice));
    assert!(equivalent(&both, &object(&[("a", number(2, 0)), ("a", number(1, 0))])));
}

/// Verifies: [DESIGN] RULES.md R-CF-6: items nested `MAX_NESTING` deep are
/// compared, within arrays and within objects; one level deeper is refused
/// with SQLCODE -900.
#[test]
fn equivalence_is_bounded_by_the_nesting_limit() {
    let deepest = nested(Item::Null, MAX_NESTING);
    assert!(equivalent(&deepest, &deepest.clone()));
    assert!(!equivalent(&deepest, &nested(number(1, 0), MAX_NESTING)));
    let within = object(&[("k", nested(Item::Null, MAX_NESTING - 1))]);
    assert!(equivalent(&within, &within.clone()));
    let deeper = nested(Item::Null, MAX_NESTING + 1);
    let refused = deeper.equivalent(&deeper.clone()).unwrap_err();
    assert_eq!(refused.sqlcode(), -900_i32);
}

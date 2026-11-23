//! Tests of what the constructor functions share: an input made an item,
//! a name made a key, the limits on members and elements; and the helpers
//! the tests of the constructors share.

use super::*;
use crate::json::parse;
use crate::value::datetime::Field;
use crate::value::interval::{Interval, Qualifier};
use crate::value::lob::{CHUNK_OCTETS, Lob};
use crate::value::{Binary, Decimal, compare};

/// A character string.
pub(super) fn text(s: &str) -> Value {
    Value::Char(s.into())
}

/// The JSON text `value` is, a character string.
pub(super) fn json(value: &Value) -> String {
    let Value::Char(s) = value else { panic!("{value:?}: no character string") };
    s.to_string()
}

/// An input without FORMAT JSON.
pub(super) fn plain(value: &Value) -> Input<'_> {
    Input { value, format_json: false }
}

/// An input FORMAT JSON.
pub(super) fn formatted(value: &Value) -> Input<'_> {
    Input { value, format_json: true }
}

/// The item `input` is, WITHOUT UNIQUE KEYS.
fn item(input: &Input<'_>) -> Item {
    input_item(input, Keys::Any).unwrap()
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.34 GR2d ii 1 F I, IV 1]
/// an input without FORMAT JSON is the SQL/JSON null of the null value,
/// the scalar a character string, a number or a truth value is, a
/// character large object's text; and is not parsed.
#[test]
fn an_input_is_the_scalar_its_value_is() {
    assert!(matches!(item(&plain(&Value::Null)), Item::Null));
    let number = Value::Exact(Decimal::new(15_i128, 1_u32));
    for value in [text("[1]"), number, Value::Approx(1.5_f64), Value::Bool(true)] {
        let Item::Scalar(scalar) = item(&plain(&value)) else { panic!("{value:?}") };
        assert_eq!(compare(scalar.value(), &value), Some(std::cmp::Ordering::Equal), "{value:?}");
    }
    let lob = Value::Lob(Lob::of_text("é"));
    assert!(matches!(item(&plain(&lob)).as_scalar().unwrap().value(), Value::Char(s) if s == "é"));
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.34 GR2d ii 1 F IV 2] an
/// input of another type is cast to a character string (ID100): a
/// datetime and an interval their literals' strings; one CAST takes to
/// none is the cast's exception, a binary string's; a character large
/// object longer than one chunk SQLCODE -900.
#[test]
fn an_input_of_another_type_is_cast_to_a_string() {
    let date = cast(text("2020-01-02"), &DataType::Date).unwrap();
    assert!(matches!(item(&plain(&date)).as_scalar().unwrap().value(), Value::Char(s) if s == "2020-01-02"));
    let qualifier = Qualifier { start: Field::Year, end: Field::Year, leading: 2, fraction: 0 };
    let interval = Value::Interval(Interval { units: 24, qualifier });
    let expected = cast(interval.clone(), &SDT).unwrap();
    assert_eq!(compare(item(&plain(&interval)).as_scalar().unwrap().value(), &expected), Some(std::cmp::Ordering::Equal));
    for value in [Value::Binary(Binary::default()), Value::Lob(Lob::of_octets(b"1"))] {
        assert_eq!(input_item(&plain(&value), Keys::Any).unwrap_err().sqlstate().class(), "22", "{value:?}");
    }
    let long = Value::Lob(Lob::of_text(&"a".repeat(CHUNK_OCTETS.saturating_add(1))));
    assert_eq!(input_item(&plain(&long), Keys::Any).unwrap_err().sqlcode(), -900_i32);
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.34 GR2d ii 1 F III, §9.42
/// GR3a iii] an input FORMAT JSON is the item its JSON text is, of a
/// character large object too, the SQL/JSON null of the null value; one
/// that is no JSON text SQLSTATE 22032; one of two members of one key
/// 22037 under WITH UNIQUE KEYS alone.
#[test]
fn an_input_format_json_is_parsed() {
    assert!(item(&formatted(&text(r#"{"a": [1]}"#))).equivalent(&parse(r#"{"a":[1]}"#, Keys::Any).unwrap()).unwrap());
    assert!(matches!(item(&formatted(&Value::Lob(Lob::of_text("[]")))), Item::Array(a) if a.is_empty()));
    assert!(matches!(item(&formatted(&Value::Null)), Item::Null));
    assert!(matches!(item(&formatted(&text("null"))), Item::Null));
    assert_eq!(input_item(&formatted(&text("[1,")), Keys::Any).unwrap_err().sqlstate().as_str(), "22032");
    let twice = text(r#"{"a": 1, "a": 2}"#);
    assert!(input_item(&formatted(&twice), Keys::Any).is_ok());
    assert_eq!(input_item(&formatted(&twice), Keys::Unique).unwrap_err().sqlstate().as_str(), "22037");
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.12 SR6] an input FORMAT
/// JSON is a string: of another type SQLCODE -103; a binary string
/// SQLCODE -900.
#[test]
fn an_input_format_json_is_a_string() {
    let e = input_item(&formatted(&Value::Bool(true)), Keys::Any).unwrap_err();
    assert_eq!(e.sqlcode(), -103_i32);
    assert_eq!(e.rule.as_deref(), Some("ISO/IEC 9075-2:2023 §10.12 SR6"));
    for value in [Value::Binary(Binary::default()), Value::Lob(Lob::of_octets(b"1"))] {
        assert_eq!(input_item(&formatted(&value), Keys::Any).unwrap_err().sqlcode(), -900_i32);
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.34 GR2d ii 1 C, D] a name
/// is a character string's characters, a character large object's; the
/// null value is SQLSTATE 22004; a value of another type SQLCODE -103, a
/// binary large object's too; a character large object longer than one
/// chunk -900.
#[test]
fn a_name_is_a_character_string() {
    assert_eq!(key(&text("a")).unwrap(), "a");
    assert_eq!(key(&Value::Lob(Lob::of_text("b"))).unwrap(), "b");
    let e = key(&Value::Null).unwrap_err();
    assert_eq!(e.sqlstate().as_str(), "22004");
    assert_eq!(e.rule.as_deref(), Some("ISO/IEC 9075-2:2023 §6.34 GR2d ii 1 D"));
    for value in [Value::Bool(true), Value::Lob(Lob::of_octets(b"a")), Value::Exact(Decimal::new(1_i128, 0_u32))] {
        let wrong = key(&value).unwrap_err();
        assert_eq!(wrong.sqlcode(), -103_i32, "{value:?}");
        assert_eq!(wrong.rule.as_deref(), Some("ISO/IEC 9075-2:2023 §6.34 F"));
    }
    let long = Value::Lob(Lob::of_text(&"a".repeat(CHUNK_OCTETS.saturating_add(1))));
    assert_eq!(key(&long).unwrap_err().sqlcode(), -900_i32);
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.34 GR2d ii 3, GR3c ii 2]
/// a count of members or elements up to the maximum is within it (IL051,
/// IL052, 2^20); one more is the exception given.
#[test]
fn counts_past_the_maximum_are_too_many() {
    assert_eq!(MAX_MEMBERS, 1_048_576_usize);
    assert_eq!(MAX_ELEMENTS, 1_048_576_usize);
    let state = SqlState::TOO_MANY_JSON_ARRAY_ELEMENTS;
    assert!(within(MAX_ELEMENTS, MAX_ELEMENTS, state, "[DESIGN] many").is_ok());
    assert!(within(0, MAX_ELEMENTS, state, "[DESIGN] many").is_ok());
    let e = within(MAX_ELEMENTS.saturating_add(1), MAX_ELEMENTS, state, "[ISO/IEC 9075-2:2023 §6.34 GR3c ii 2] many").unwrap_err();
    assert_eq!(e.sqlstate().as_str(), "2203D");
    assert_eq!(e.rule.as_deref(), Some("ISO/IEC 9075-2:2023 §6.34 GR3c ii 2"));
}

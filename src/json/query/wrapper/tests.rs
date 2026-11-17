//! Tests of converting an SQL/JSON sequence to an SQL/JSON item.

use super::*;
use crate::error::{Code, SqlError};
use crate::json::{Keys, parse};

/// The items of the JSON array `text`.
fn seq(text: &str) -> Vec<Item> {
    let Item::Array(items) = parse(text, Keys::Any).unwrap() else { panic!("{text}") };
    items
}

/// The item the sequence of the JSON array `text` converts to.
fn convert(text: &str, wrapper: Wrapper, on_empty: QueryBehavior, on_error: QueryBehavior) -> Result<Option<Item>> {
    sequence_to_item(Ok(seq(text)), wrapper, on_empty, on_error)
}

/// That `got` is the item of the JSON text `expected`.
fn is(got: Option<Item>, expected: &str) {
    let got = got.unwrap();
    assert!(got.equivalent(&parse(expected, Keys::Any).unwrap()).unwrap(), "{got:?}, not {expected}");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.44 GR4a ii, iii, iv, GR4b]
/// one item: WITHOUT ARRAY and WITH CONDITIONAL ARRAY leave it as it is,
/// an array, an object or a scalar alike; WITH UNCONDITIONAL ARRAY wraps
/// it.
#[test]
fn one_item_is_wrapped_unconditionally_only() {
    for text in ["[1]", "[[1]]", r#"[{"a": 1}]"#, "[null]"] {
        let only = seq(text).remove(0);
        for wrapper in [Wrapper::Without, Wrapper::Conditional] {
            assert!(convert(text, wrapper, QueryBehavior::Error, QueryBehavior::Error).unwrap().unwrap().equivalent(&only).unwrap());
        }
        is(convert(text, Wrapper::Unconditional, QueryBehavior::Error, QueryBehavior::Error).unwrap(), text);
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.44 GR4a iii, iv 2, GR4b
/// ii, GR4c iii] items more than one: WITH a wrapper, the array of them in
/// order; WITHOUT ARRAY, SQLSTATE 22034, which ON ERROR replaces.
#[test]
fn more_than_one_item_is_wrapped_or_an_exception() {
    for wrapper in [Wrapper::Conditional, Wrapper::Unconditional] {
        is(convert("[2, [1], {}]", wrapper, QueryBehavior::Error, QueryBehavior::Error).unwrap(), "[2, [1], {}]");
    }
    let e = convert("[1, 2]", Wrapper::Without, QueryBehavior::Null, QueryBehavior::Error).unwrap_err();
    assert_eq!(e.sqlstate().as_str(), "22034");
    assert_eq!(e.rule.as_deref(), Some("ISO/IEC 9075-2:2023 §9.44 GR4c iii"));
    assert!(convert("[1, 2]", Wrapper::Without, QueryBehavior::Error, QueryBehavior::Null).unwrap().is_none());
    is(convert("[1, 2]", Wrapper::Without, QueryBehavior::Error, QueryBehavior::EmptyArray).unwrap(), "[]");
    is(convert("[1, 2]", Wrapper::Without, QueryBehavior::Error, QueryBehavior::EmptyObject).unwrap(), "{}");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.44 GR4a i, GR4c i, GR5a i]
/// no item is never wrapped [NOTE 479]: ON EMPTY NULL gives the null
/// value, EMPTY ARRAY and EMPTY OBJECT theirs, ERROR SQLSTATE 22035, which
/// ON ERROR replaces, so ERROR ON EMPTY with NULL ON ERROR is the null
/// value.
#[test]
fn no_item_is_as_on_empty_says() {
    for wrapper in [Wrapper::Without, Wrapper::Conditional, Wrapper::Unconditional] {
        assert!(convert("[]", wrapper, QueryBehavior::Null, QueryBehavior::Error).unwrap().is_none());
        is(convert("[]", wrapper, QueryBehavior::EmptyArray, QueryBehavior::Error).unwrap(), "[]");
        is(convert("[]", wrapper, QueryBehavior::EmptyObject, QueryBehavior::Error).unwrap(), "{}");
        let e = convert("[]", wrapper, QueryBehavior::Error, QueryBehavior::Error).unwrap_err();
        assert_eq!(e.sqlstate().as_str(), "22035");
        assert_eq!(e.rule.as_deref(), Some("ISO/IEC 9075-2:2023 §9.44 GR4c i 1"));
        assert!(convert("[]", wrapper, QueryBehavior::Error, QueryBehavior::Null).unwrap().is_none());
        is(convert("[]", wrapper, QueryBehavior::Error, QueryBehavior::EmptyObject).unwrap(), "{}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.44 GR4, GR5b] STATUS IN
/// an exception condition is replaced as ON ERROR says; an error not one
/// is raised whatever it says.
#[test]
fn the_status_in_is_replaced_or_raised() {
    let exception = || Err(SqlError::new(Code::Data, "[DESIGN] an exception"));
    assert!(sequence_to_item(exception(), Wrapper::Without, QueryBehavior::Error, QueryBehavior::Null).unwrap().is_none());
    assert_eq!(sequence_to_item(exception(), Wrapper::Without, QueryBehavior::Null, QueryBehavior::Error).unwrap_err().sqlcode(), -301_i32);
    let rule = Err(SqlError::new(Code::SyntaxRule, "[DESIGN] a rule"));
    assert_eq!(sequence_to_item(rule, Wrapper::Without, QueryBehavior::Null, QueryBehavior::Null).unwrap_err().sqlcode(), -103_i32);
}

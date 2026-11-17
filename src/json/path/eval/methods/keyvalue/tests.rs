//! Tests of `keyvalue()`, lax and strict.

use super::*;
use crate::json::path::eval::tests::{error, gives, state};

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C VIII]
/// `keyvalue()` gives an object of `key`, `value` and `id` of each member
/// of each object, in the order kept (US040); the `id` of an object is its
/// place in the base from 0; lax: the base is unwrapped.
#[test]
fn keyvalue_gives_each_members_key_and_value() {
    gives("lax $.keyvalue()", r#"{"a": 1, "b": [true]}"#, r#"[{"key": "a", "value": 1, "id": 0}, {"key": "b", "value": [true], "id": 0}]"#);
    gives(
        "lax $.keyvalue()",
        r#"[{"x": null}, {}, {"y": {"z": 2}}]"#,
        r#"[{"key": "x", "value": null, "id": 0}, {"key": "y", "value": {"z": 2}, "id": 2}]"#,
    );
    gives("strict $.keyvalue().key", r#"{"b": 1, "a": 2}"#, r#"["b", "a"]"#);
    gives("strict $[*].keyvalue().id", r#"[{"a": 1}, {"b": 2}]"#, "[0, 1]");
    gives("lax $.keyvalue()", "{}", "[]");
    gives("lax $.none.keyvalue()", "{}", "[]");
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C VIII 1
/// b, VIII 1 c iv] (IA072) members of one key raise no exception, and
/// (UA058) none of them is deleted.
#[test]
fn keyvalue_keeps_members_of_one_key() {
    gives("lax $.keyvalue().value", r#"{"a": 1, "a": 2}"#, "[1, 2]");
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C VIII 1
/// a] an item that is no object is SQLSTATE 2203C, in either mode: a
/// scalar, null, an array within the array lax mode unwraps; strict: the
/// array itself.
#[test]
fn keyvalue_of_what_is_no_object_is_an_error() {
    for context in ["1", "\"a\"", "null", "[{}, [{}]]"] {
        assert_eq!(state("lax $.keyvalue()", context), "2203C", "{context}");
    }
    let e = error("strict $.keyvalue()", "[{}]");
    assert_eq!(e.sqlstate().as_str(), "2203C");
    assert_eq!(e.rule.as_deref(), Some("ISO/IEC 9075-2:2023 §9.46 GR11g ii 6 C VIII 1 a"));
    assert_eq!((KEY, VALUE, ID), ("key", "value", "id"));
}

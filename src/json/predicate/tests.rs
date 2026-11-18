//! Tests of IS JSON and IS NOT JSON.

use super::*;
use crate::value::lob::{CHUNK_OCTETS, Lob};
use crate::value::{Binary, Value};

/// A character string.
fn text(s: &str) -> Value {
    Value::Char(s.to_owned())
}

/// `s IS JSON kind keys`, a truth value.
fn truth(s: &str, kind: JsonType, keys: Keys) -> bool {
    let Value::Bool(b) = is_json(&text(s), kind, keys).unwrap() else { panic!("{s}: not a truth value") };
    b
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.22 SR2, GR3e] IS JSON
/// VALUE, the default, is True of every JSON text: a scalar, the literal
/// null, an array, an object, white space around them.
#[test]
fn a_json_text_is_json() {
    for s in ["1", "\"a\"", "null", "true", " [1, {}] ", r#"{"a": [null]}"#] {
        assert!(truth(s, JsonType::Value, Keys::Any), "{s}");
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.22 GR2c, GR3a, §9.42
/// GR3a iii 1] a string that is no JSON text is False: empty, unclosed,
/// two values, an SQL literal, a trailing comma.
#[test]
fn a_string_that_is_no_json_text_is_not_json() {
    for s in ["", "{", "1 2", "TRUE", "[1,]", "'a'"] {
        assert!(!truth(s, JsonType::Value, Keys::Any), "{s}");
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.22 GR3b, GR3c, GR3d] ARRAY,
/// OBJECT and SCALAR are True of an array, an object, and of neither (the
/// literal null too) alone. MC/DC of `Array | Object` for SCALAR: an
/// array and an object each False, a number True.
#[test]
fn the_type_constraint_names_the_kind_of_item() {
    let cases = [
        ("[1]", [true, true, false, false]),
        ("{}", [true, false, true, false]),
        ("1", [true, false, false, true]),
        ("null", [true, false, false, true]),
    ];
    for (s, expected) in cases {
        let kinds = [JsonType::Value, JsonType::Array, JsonType::Object, JsonType::Scalar];
        for (kind, want) in kinds.into_iter().zip(expected) {
            assert_eq!(truth(s, kind, Keys::Any), want, "{s} {kind:?}");
        }
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.22 SR3, GR3a, §9.42 GR3a
/// iii 2] WITHOUT UNIQUE KEYS, the default, is True of an object of two
/// members of one key, nested too; WITH UNIQUE KEYS is False of it, True
/// of keys that differ.
#[test]
fn the_key_uniqueness_constraint() {
    for s in [r#"{"a": 1, "a": 2}"#, r#"[{"b": {"a": 1, "a": 1}}]"#] {
        assert!(truth(s, JsonType::Value, Keys::Any), "{s}");
        assert!(!truth(s, JsonType::Value, Keys::Unique), "{s}");
    }
    assert!(truth(r#"{"a": 1, "b": {"a": 2}}"#, JsonType::Object, Keys::Unique));
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.22 GR2a, SR4] of the null
/// value IS JSON and IS NOT JSON are both Unknown, the null value,
/// whatever the constraints.
#[test]
fn the_null_value_is_unknown() {
    for kind in [JsonType::Value, JsonType::Array, JsonType::Object, JsonType::Scalar] {
        for keys in [Keys::Any, Keys::Unique] {
            assert!(is_json(&Value::Null, kind, keys).unwrap().is_null());
            assert!(is_not_json(&Value::Null, kind, keys).unwrap().is_null());
        }
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.22 SR4] IS NOT JSON is the
/// negation of IS JSON of the same constraints.
#[test]
fn is_not_json_is_the_negation() {
    let not = |s: &str, kind, keys| matches!(is_not_json(&text(s), kind, keys).unwrap(), Value::Bool(true));
    assert!(!not("[]", JsonType::Value, Keys::Any));
    assert!(not("{", JsonType::Value, Keys::Any));
    assert!(not("[]", JsonType::Object, Keys::Any));
    assert!(not(r#"{"a": 1, "a": 2}"#, JsonType::Object, Keys::Unique));
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.22 GR2c] a character large
/// object is parsed as a character string is.
#[test]
fn a_character_large_object_is_read_whole() {
    let lob = Value::Lob(Lob::of_text(r#"{"a": [1]}"#));
    assert!(matches!(is_json(&lob, JsonType::Object, Keys::Unique).unwrap(), Value::Bool(true)));
    let not = Value::Lob(Lob::of_text("{"));
    assert!(matches!(is_json(&not, JsonType::Value, Keys::Any).unwrap(), Value::Bool(false)));
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.22 F] the input is a
/// string: of another type SQLCODE -103; a binary string, a character
/// large object longer than one chunk and a text nested past turborel's
/// limit are SQLCODE -900 (RULES.md D18), not False.
#[test]
fn errors_that_are_no_answer_are_raised() {
    let e = is_json(&Value::Bool(true), JsonType::Value, Keys::Any).unwrap_err();
    assert_eq!(e.sqlcode(), -103_i32);
    assert_eq!(e.rule.as_deref(), Some("ISO/IEC 9075-2:2023 §8.22 F"));
    let deep = text(&format!("{}{}", "[".repeat(200), "]".repeat(200)));
    let long = Value::Lob(Lob::of_text(&"a".repeat(CHUNK_OCTETS.saturating_add(1))));
    for value in [Value::Binary(Binary::default()), long, deep] {
        assert_eq!(is_json(&value, JsonType::Value, Keys::Any).unwrap_err().sqlcode(), -900_i32);
        assert_eq!(is_not_json(&value, JsonType::Value, Keys::Any).unwrap_err().sqlcode(), -900_i32);
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.22 GR3a] [ISO/IEC 9075-2:2023 §9.42
/// GR3a iii 1] a number no number of turborel's holds is still of RFC
/// 8259's grammar, so IS JSON raises its SQLSTATE 22003 rather than
/// answer False; a text that is none is still False. MC/DC of the STATUS
/// that is False: 22032, 22037, and 22003 not.
#[test]
fn a_number_out_of_range_is_raised_not_false() {
    for number in ["1e400", "1e-400", &format!("1{}1", "0".repeat(40))] {
        let e = is_json(&text(number), JsonType::Value, Keys::Any).unwrap_err();
        assert_eq!(e.sqlstate().as_str(), "22003", "{number}");
    }
    assert!(matches!(is_json(&text("[1,"), JsonType::Value, Keys::Any).unwrap(), Value::Bool(false)));
    assert!(matches!(is_json(&text(r#"{"a":1,"a":2}"#), JsonType::Value, Keys::Unique).unwrap(), Value::Bool(false)));
}

//! Tests of JSON_EXISTS.

use super::*;
use crate::json::query::tests::{on, text};

/// JSON_EXISTS of the JSON text `context` and `path`, ON ERROR `on_error`.
fn exists(context: &Value, path: &str, on_error: ExistsBehavior) -> Result<Value> {
    on(context, path, &[], |common| json_exists(common, on_error))
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.23 GR4a] True if the path
/// gives an SQL/JSON item, the SQL/JSON null too; False if it gives none.
#[test]
fn whether_the_path_gives_an_item() {
    let context = text(r#"{"a": null, "b": []}"#);
    assert!(matches!(exists(&context, "lax $.a", ExistsBehavior::Error).unwrap(), Value::Bool(true)));
    assert!(matches!(exists(&context, "lax $.c", ExistsBehavior::Error).unwrap(), Value::Bool(false)));
    assert!(matches!(exists(&context, "lax $.b[*]", ExistsBehavior::Error).unwrap(), Value::Bool(false)));
    assert!(matches!(exists(&context, "strict $.b", ExistsBehavior::Error).unwrap(), Value::Bool(true)));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.23 GR1] of a null context
/// item, Unknown, the null value, whatever the path and ON ERROR.
#[test]
fn a_null_context_item_is_unknown() {
    for behavior in [ExistsBehavior::True, ExistsBehavior::False, ExistsBehavior::Unknown, ExistsBehavior::Error] {
        assert!(exists(&Value::Null, "strict $.a", behavior).unwrap().is_null());
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.23 SR1, GR4b] in place of
/// an exception condition, of the path or of a context item that is no
/// JSON text, TRUE, FALSE (the default) and UNKNOWN give their truth
/// values, ERROR raises it.
#[test]
fn an_exception_is_replaced_as_on_error_says() {
    for (context, path, state) in [(text(r#"{"a": 1}"#), "strict $.b", "2203A"), (text("{"), "lax $", "22032")] {
        assert!(matches!(exists(&context, path, ExistsBehavior::True).unwrap(), Value::Bool(true)));
        assert!(matches!(exists(&context, path, ExistsBehavior::False).unwrap(), Value::Bool(false)));
        assert!(exists(&context, path, ExistsBehavior::Unknown).unwrap().is_null());
        assert_eq!(exists(&context, path, ExistsBehavior::Error).unwrap_err().sqlstate().as_str(), state);
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 SR4] an error not an
/// exception condition is raised whatever ON ERROR says: a variable no
/// argument names, SQLCODE -103; a context item nested past turborel's
/// limit, SQLCODE -900.
#[test]
fn an_error_not_an_exception_is_raised() {
    for behavior in [ExistsBehavior::True, ExistsBehavior::False, ExistsBehavior::Unknown] {
        assert_eq!(exists(&text("1"), "lax $x", behavior).unwrap_err().sqlcode(), -103_i32);
        let deep = format!("{}1{}", "[".repeat(200), "]".repeat(200));
        assert_eq!(exists(&text(&deep), "lax $", behavior).unwrap_err().sqlcode(), -900_i32);
    }
}

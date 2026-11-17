//! Tests of processing the `<JSON API common syntax>`: the context item,
//! the PASSING clause's arguments, the path evaluated of them, which
//! errors are exception conditions; and the helpers the tests of the
//! operators share.

use super::*;
use crate::error::Code;
use crate::json::parse_path;
use crate::value::datetime::Field;
use crate::value::interval::{Interval, Qualifier};
use crate::value::lob::{CHUNK_OCTETS, Lob};
use crate::value::{Binary, Spelled, compare};

/// A character string.
pub(super) fn text(s: &str) -> Value {
    Value::Char(s.to_owned())
}

/// The value of the character string `s` cast to `t`.
pub(super) fn typed(s: &str, t: &DataType) -> Value {
    cast(text(s), t).unwrap()
}

/// `f` of the common syntax of the context item `context`, the path
/// `path` and the arguments `passing`, each its name, value and whether
/// FORMAT JSON.
pub(super) fn on<T>(context: &Value, path: &str, passing: &[(&str, Value, bool)], f: impl FnOnce(&CommonSyntax<'_>) -> T) -> T {
    let path = parse_path(path).unwrap();
    let passing: Vec<Passing<'_>> =
        passing.iter().map(|(name, value, format_json)| Passing { name, value, format_json: *format_json }).collect();
    f(&CommonSyntax { context, path: &path, passing: &passing })
}

/// What `path` gives of the JSON text `context`, `passing` as `on` takes
/// it.
fn run(context: &Value, path: &str, passing: &[(&str, Value, bool)]) -> Result<Option<Vec<Item>>> {
    on(context, path, passing, process)
}

/// That `path` gives, of `context` and `passing`, the items of the JSON
/// array `expected`.
fn gives(context: &Value, path: &str, passing: &[(&str, Value, bool)], expected: &str) {
    let got = run(context, path, passing).unwrap().unwrap();
    assert!(Item::Array(got.clone()).equivalent(&parse(expected, Keys::Any).unwrap()).unwrap(), "{path}: {got:?}, not {expected}");
}

/// The error `path` gives of `context` and `passing`.
fn error(context: &Value, path: &str, passing: &[(&str, Value, bool)]) -> SqlError {
    run(context, path, passing).unwrap_err()
}

/// The item `value` is as an argument without FORMAT JSON.
fn item(value: &Value) -> Item {
    Item::of_value(value).unwrap()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.47, §9.46 GR4, GR11a] the
/// context item, a character string or a character large object, is
/// parsed as JSON text and the path evaluated of it; of the null value
/// none is, whatever the path.
#[test]
fn the_context_item_is_parsed_and_the_path_evaluated() {
    gives(&text(r#"{"a": [1, 2]}"#), "lax $.a[*]", &[], "[1, 2]");
    gives(&Value::Lob(Lob::of_text(r#"{"a": "b"}"#)), "strict $.a", &[], r#"["b"]"#);
    gives(&text(r#"{"a": 1, "a": 2}"#), "lax $.a", &[], "[1]");
    assert!(run(&Value::Null, "lax $.a", &[]).unwrap().is_none());
    assert!(run(&Value::Null, "lax $x", &[("x", Value::Null, false)]).unwrap().is_none());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 GR4] a context item
/// that is no JSON text is SQLSTATE 22032, though the path does not read
/// it (IA050: it is parsed first); the path's exception is its own.
#[test]
fn a_context_item_that_is_no_json_text_is_an_exception() {
    for path in ["lax $", "lax 1"] {
        let e = error(&text("{"), path, &[]);
        assert_eq!(e.sqlstate().as_str(), "22032", "{path}");
        assert!(is_exception(&e));
    }
    assert_eq!(error(&text("{}"), "strict $.a", &[]).sqlstate().as_str(), "2203A");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.14 SR1] the context item
/// is a string: of another type, SQLCODE -103; a binary string, whose
/// encodings turborel does not implement, SQLCODE -900; a character large
/// object longer than one chunk, SQLCODE -900; none is an exception
/// condition.
#[test]
fn the_context_item_is_a_character_string() {
    let e = error(&Value::Bool(true), "lax $", &[]);
    assert_eq!(e.sqlcode(), -103_i32);
    assert_eq!(e.rule.as_deref(), Some("ISO/IEC 9075-2:2023 §10.14 SR1"));
    assert!(!is_exception(&e));
    let long = format!("\"{}\"", "a".repeat(CHUNK_OCTETS));
    for context in [Value::Binary(Binary::default()), Value::Lob(Lob::of_octets(b"{}")), Value::Lob(Lob::of_text(&long))] {
        let limit = error(&context, "lax $", &[]);
        assert_eq!(limit.sqlcode(), -900_i32, "{context:?}");
        assert!(!is_exception(&limit));
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 SR4] a named variable
/// no argument names is refused with SQLCODE -103 first, of a null
/// context item and one that is no JSON text too, and is no exception
/// condition.
#[test]
fn a_variable_no_argument_names_is_refused_first() {
    for context in [Value::Null, text("{"), text("1")] {
        let e = error(&context, "lax $x + $y", &[("x", text("1"), false)]);
        assert_eq!(e.sqlcode(), -103_i32);
        assert_eq!(e.rule.as_deref(), Some("ISO/IEC 9075-2:2023 §9.46 SR4"));
        assert!(!is_exception(&e));
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 GR11b i 1, §9.46 GR5]
/// an argument FORMAT JSON is the item its JSON text is, of a character
/// large object too; of the null value, the empty sequence; one that is
/// no JSON text SQLSTATE 22032, though the path does not read it (IA054).
#[test]
fn an_argument_format_json_is_parsed() {
    gives(&text("0"), "lax $x.a", &[("x", text(r#"{"a": [1]}"#), true)], "[[1]]");
    gives(&text("0"), "lax $x", &[("x", Value::Lob(Lob::of_text("[2]")), true)], "[[2]]");
    gives(&text("0"), "lax $x", &[("x", Value::Null, true)], "[]");
    gives(&text("0"), "lax $[*] ? (exists ($x))", &[("x", Value::Null, true)], "[]");
    gives(&text("0"), "lax $x", &[("x", text("\"s\""), true)], r#"["s"]"#);
    assert_eq!(error(&text("0"), "lax 1", &[("x", text("[1,"), true)]).sqlstate().as_str(), "22032");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.12 SR6] an argument
/// FORMAT JSON is a string: of another type SQLCODE -103; a binary
/// string SQLCODE -900.
#[test]
fn an_argument_format_json_is_a_string() {
    let e = error(&text("0"), "lax $x", &[("x", Value::Bool(true), true)]);
    assert_eq!(e.sqlcode(), -103_i32);
    assert_eq!(e.rule.as_deref(), Some("ISO/IEC 9075-2:2023 §10.12 SR6"));
    assert_eq!(error(&text("0"), "lax $x", &[("x", Value::Binary(Binary::default()), true)]).sqlcode(), -900_i32);
    assert_eq!(error(&text("0"), "lax $x", &[("x", Value::Lob(Lob::of_octets(b"1")), true)]).sqlcode(), -900_i32);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 GR11b i 2 A, ii 2] an
/// argument without FORMAT JSON is the scalar its value is, a character
/// string, a number, a truth value, a datetime; a character large
/// object's text; the null value the SQL/JSON null; and is not parsed.
#[test]
fn an_argument_is_the_scalar_its_value_is() {
    assert!(matches!(item(&Value::Null), Item::Null));
    let date = typed("2020-01-02", &DataType::Date);
    for value in [text("[1]"), Value::Approx(1.5_f64), Value::Bool(false), date] {
        let Item::Scalar(scalar) = item(&value) else { panic!("{value:?}") };
        assert_eq!(compare(scalar.value(), &value), Some(std::cmp::Ordering::Equal), "{value:?}");
    }
    assert!(matches!(item(&Value::Lob(Lob::of_text("é"))).as_scalar().unwrap().value(), Value::Char(s) if s == "é"));
    gives(&text("[10, 20]"), "lax $[$i]", &[("i", typed("1", &integer()), false)], "[20]");
    gives(&text("0"), "lax $s", &[("s", text("[1]"), false)], r#"["[1]"]"#);
    gives(&text("0"), "lax $n.type()", &[("n", Value::Null, false)], r#"["null"]"#);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 GR11b i 2 B] an
/// argument of another type is cast to a character string (IV165): an
/// interval its literal's string; a value CAST takes to none is the cast's
/// exception condition, SQLSTATE 22005; a character large object longer
/// than one chunk SQLCODE -900.
#[test]
fn an_argument_of_another_type_is_cast_to_a_string() {
    let qualifier = Qualifier { start: Field::Year, end: Field::Year, leading: 2, fraction: 0 };
    let interval = Value::Interval(Interval { units: 24, qualifier });
    let expected = cast(interval.clone(), &ARGUMENT_STRING).unwrap();
    assert_eq!(compare(item(&interval).as_scalar().unwrap().value(), &expected), Some(std::cmp::Ordering::Equal));
    for value in
        [Value::Binary(Binary::default()), Value::Lob(Lob::of_octets(b"1")), Value::Array(std::sync::Arc::from(vec![Value::Bool(true)]))]
    {
        let e = Item::of_value(&value).unwrap_err();
        assert!(is_exception(&e), "{value:?}: {}", e.msg);
    }
    let long = Value::Lob(Lob::of_text(&"a".repeat(CHUNK_OCTETS + 1)));
    assert_eq!(Item::of_value(&long).unwrap_err().sqlcode(), -900_i32);
}

/// Verifies: [DESIGN] an exception condition an ON ERROR behaviour
/// replaces is a data exception, SQLSTATE class 22, whatever its subclass;
/// a Syntax Rule's error and a limit's are none.
#[test]
fn an_exception_condition_is_a_data_exception() {
    assert!(is_exception(&exception(SqlState::NO_SQL_JSON_ITEM, "[DESIGN] none")));
    assert!(is_exception(&SqlError::new(Code::Data, "[DESIGN] data")));
    assert!(!is_exception(&SqlError::new(Code::SyntaxRule, "[DESIGN] rule")));
    assert!(!is_exception(&SqlError::new(Code::Unsupported, "[DESIGN] limit")));
}

/// INTEGER.
pub(super) fn integer() -> DataType {
    DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer }
}

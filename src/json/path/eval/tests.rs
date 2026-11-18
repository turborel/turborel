//! Tests of evaluating an SQL/JSON path expression: its primaries, Wrap
//! and Unwrap, the named variables and the bound of its recursion; and the
//! helpers the tests of the evaluator's parts share.

use super::*;
use crate::error::SqlError;
use crate::json::path::parse_path;
use crate::json::{Keys, parse};
use crate::value::datetime::{Clocked, Moment};
use crate::value::{Datetime, Value};
use std::borrow::Cow;

/// The item the JSON text `text` is.
pub(super) fn json(text: &str) -> Item {
    parse(text, Keys::Any).unwrap_or_else(|e| panic!("{text}: {}", e.msg))
}

/// What `path` gives of the JSON text `context`, `variables` the names and
/// JSON texts of its variables.
pub(super) fn run_with(path: &str, context: &str, variables: &[(&str, &str)]) -> Result<Vec<Item>> {
    let variables =
        variables.iter().map(|(name, text)| Ok(((*name).to_owned(), vec![parse(text, Keys::Any)?]))).collect::<Result<Vec<_>>>()?;
    parse_path(path, Clocked::default())?.evaluate(&parse(context, Keys::Any)?, &variables)
}

/// That `path` gives, of `context` and `variables` as `run_with` takes
/// them, the items of the JSON array `expected`, each equivalent.
pub(super) fn gives_with(path: &str, context: &str, variables: &[(&str, &str)], expected: &str) {
    let got = run_with(path, context, variables).unwrap_or_else(|e| panic!("{path} of {context}: {}", e.msg));
    assert!(Item::Array(got.clone()).equivalent(&json(expected)).unwrap(), "{path} of {context} gave {got:?}, not {expected}");
}

/// What `path` gives of the JSON text `context`.
pub(super) fn run(path: &str, context: &str) -> Result<Vec<Item>> {
    run_with(path, context, &[])
}

/// That `path` gives, of `context`, the items of the JSON array `expected`,
/// in order, each equivalent.
pub(super) fn gives(path: &str, context: &str, expected: &str) {
    let got = run(path, context).unwrap_or_else(|e| panic!("{path} of {context}: {}", e.msg));
    assert!(Item::Array(got.clone()).equivalent(&json(expected)).unwrap(), "{path} of {context} gave {got:?}, not {expected}");
}

/// The error `path` gives of `context`.
pub(super) fn error(path: &str, context: &str) -> SqlError {
    match run(path, context) {
        Ok(got) => panic!("{path} of {context} gave {got:?}, not an error"),
        Err(e) => e,
    }
}

/// The SQLSTATE of the error `path` gives of `context`.
pub(super) fn state(path: &str, context: &str) -> String {
    error(path, context).sqlstate().as_str().to_owned()
}

/// The one SQL value `path` gives of `context`.
pub(super) fn value(path: &str, context: &str) -> Value {
    let got = run(path, context).unwrap_or_else(|e| panic!("{path} of {context}: {}", e.msg));
    let [only] = got.as_slice() else { panic!("{path} of {context} gave {got:?}") };
    only.as_scalar().unwrap().value().clone()
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 GR11a, GR11e, GR11f]
/// `$` is the context item, whatever it is; a literal is its value; a
/// parenthesized wff is the wff's value.
#[test]
fn the_context_item_and_literals_are_their_values() {
    gives("lax $", r#"{"a": [1, 2]}"#, r#"[{"a": [1, 2]}]"#);
    gives("strict $", "[1, 2]", "[[1, 2]]");
    gives("lax $", "null", "[null]");
    gives("lax 1", "{}", "[1]");
    gives("lax \"a\"", "{}", r#"["a"]"#);
    gives("lax true", "{}", "[true]");
    gives("lax null", "{}", "[null]");
    gives("lax ($)", "7", "[7]");
    assert!(matches!(value("lax 1.5e0", "0"), Value::Approx(x) if x.to_bits() == 1.5_f64.to_bits()));
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 GR11b] a named
/// variable is the item it is passed, wherever it is in the path; of two
/// passed one name, the first is taken.
#[test]
fn a_named_variable_is_the_item_passed() {
    gives_with("lax $x", "0", &[("x", r#"{"k": [1]}"#)], r#"[{"k": [1]}]"#);
    gives_with("lax $.a[$i]", r#"{"a": [10, 20, 30]}"#, &[("i", "2")], "[30]");
    gives_with("lax $x", "0", &[("x", "1"), ("x", "2")], "[1]");
    gives_with("lax $[*] ? (@ > $min)", "[1, 5, 9]", &[("min", "4")], "[5, 9]");
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 GR11b i 1 A] a
/// variable passed the empty sequence, as a FORMAT JSON argument that is
/// the null value is, gives none: as a filter's operand too.
#[test]
fn a_variable_may_be_the_empty_sequence() {
    let variables = vec![("x".to_owned(), Vec::new())];
    assert!(parse_path("lax $x", Clocked::default()).unwrap().evaluate(&json("0"), &variables).unwrap().is_empty());
    let got = parse_path("lax $[*] ? (exists ($x))", Clocked::default()).unwrap().evaluate(&json("[1]"), &variables).unwrap();
    assert!(got.is_empty());
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 SR4] every named
/// variable is to be passed: one not is refused with SQLCODE -103 before
/// anything is evaluated, within a filter's predicate too, where an
/// exception would be Unknown, in a subscript, a range's last, an
/// operand, a sign and a starts with initial alike.
#[test]
fn a_named_variable_not_passed_is_refused_before_evaluation() {
    for path in [
        "lax $y",
        "lax $[*] ? (@ == $y)",
        "lax $[*] ? (exists ($y))",
        "lax $[*] ? ($y like_regex \"a\")",
        "lax $[*] ? (@ starts with $y)",
        "lax $[*] ? (!(@ == $y))",
        "lax $[*] ? ((@ == $y) is unknown)",
        "lax $[*] ? (@ == 1 && @ == $y)",
        "lax $[*] ? (@ == 1 || @ == $y)",
        "lax $[$y]",
        "lax $[0 to $y]",
        "lax $y + 1",
        "lax 1 * $y",
        "lax -$y",
        "lax $.a.*[*].type()[$y]",
    ] {
        let e = run_with(path, "[1, 2]", &[("x", "1")]).unwrap_err();
        assert_eq!(e.sqlcode(), -103_i32, "{path}");
        assert_eq!(e.rule.as_deref(), Some("ISO/IEC 9075-2:2023 §9.46 SR4"), "{path}");
        assert!(e.msg.contains("$y"), "{path}: {}", e.msg);
    }
    // Before anything is evaluated: the exception of 1/0 is not raised.
    let e = run_with("lax 1 / 0 + $y", "0", &[]).unwrap_err();
    assert_eq!(e.sqlcode(), -103_i32);
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 GR11c] `@` is the
/// current item of the innermost filter holding it, so a filter within
/// another's predicate has its own.
#[test]
fn at_is_the_current_item_of_the_innermost_filter() {
    gives("lax $[*] ? (@.a == 1)", r#"[{"a": 1}, {"a": 2}]"#, r#"[{"a": 1}]"#);
    gives("lax $[*] ? (exists (@.b[*] ? (@ > 2)))", r#"[{"b": [1, 3]}, {"b": [1, 2]}]"#, r#"[{"b": [1, 3]}]"#);
    gives("lax $[*] ? (@.b[*] ? (@ == 2) == 2)", r#"[{"b": [1, 2]}, {"b": [3]}]"#, r#"[{"b": [1, 2]}]"#);
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 GR11d] `last` is the
/// last subscript, N − 1, of the current array of the innermost array
/// accessor holding it, an exact number; −1 of an empty array.
#[test]
fn last_is_the_last_subscript_of_the_current_array() {
    gives("lax $[last]", "[1, 2, 3]", "[3]");
    gives("lax $[last - 1]", "[1, 2, 3]", "[2]");
    gives("lax $[*][last]", "[[1, 2], [3]]", "[2, 3]");
    gives("lax $[$[last]]", "[1, 0]", "[1]");
    gives("lax $[0 to last]", "[1, 2]", "[1, 2]");
    gives("lax $[last]", "[]", "[]");
    assert_eq!(state("strict $[last]", "[]"), "22033");
    gives("lax $[*] ? (@ == 1)[last]", "[[1], 1]", "[1, 1]");
    assert!(matches!(value("lax $[0 to 0] ? (@ == 1).size()", "[1]"), Value::Exact(d) if d.units == 1));
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.46 GR7, GR8] Wrap makes
/// an array of each item that is no array, which the array accessors see
/// as its one element; Unwrap gives each array's elements, one level only,
/// borrowed of an item borrowed and owned of one owned.
#[test]
fn wrap_and_unwrap_take_one_level() {
    let items = [json("1"), json("[2, [3]]"), json("null"), json("{}")];
    let borrowed: Seq<'_> = items.iter().map(Cow::Borrowed).collect();
    let unwrapped = unwrap(borrowed);
    assert!(
        Item::Array(unwrapped.iter().map(|each| each.clone().into_owned()).collect()).equivalent(&json("[1, 2, [3], null, {}]")).unwrap()
    );
    assert!(unwrapped.iter().all(|each| matches!(each, Cow::Borrowed(_))));
    let owned: Seq<'_> = items.iter().cloned().map(Cow::Owned).collect();
    assert!(unwrap(owned).iter().all(|each| matches!(each, Cow::Owned(_))));
    assert!(unwrap(Vec::new()).is_empty());
    gives("lax $[*]", "[1, [2], {}]", "[1, [2], {}]");
    gives("lax $.a[0]", r#"{"a": {"b": 1}}"#, r#"[{"b": 1}]"#);
    gives("lax $[*][*]", "[1, [2, [3]]]", "[1, 2, [3]]");
    assert_eq!(state("strict $[*]", "{}"), "22039");
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.48.3, §9.46 GR11b] a
/// variable may be any SQL/JSON item, a datetime too, as a PASSING
/// clause's value may be; the null value is the SQL/JSON null.
#[test]
fn a_variable_may_be_a_datetime() {
    let date = Datetime { moment: Moment::Date, utc: 0, zone: 0, zoned: false, precision: 0 };
    let variables = vec![("d".to_owned(), vec![Item::Scalar(Scalar::new(Value::Datetime(date)).unwrap())])];
    let path = parse_path("lax $d.type()", Clocked::default()).unwrap();
    let got = path.evaluate(&json("0"), &variables).unwrap();
    assert!(Item::Array(got).equivalent(&json(r#"["date"]"#)).unwrap());
    assert!(matches!(item(Value::Null), Item::Null));
}

/// Verifies: [DESIGN] RULES.md R-CF-6: the evaluator recurses over a tree
/// the parser bounds, so a path as deep as the parser takes is evaluated.
#[test]
fn a_path_as_deep_as_parsed_is_evaluated() {
    let deep = format!("lax {}1{}", "(".repeat(60), ")".repeat(60));
    gives(&deep, "0", "[1]");
    let chain = format!("lax ${}", "[0]".repeat(60));
    gives(&chain, "0", "[0]");
}

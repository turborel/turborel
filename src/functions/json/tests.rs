//! Tests of a call of an SQL/JSON query operator as a function: the name
//! it shows, the types of its `?` operands, and its arguments.

use super::*;
use crate::json::parse_path;
use crate::value::datetime::Clocked;

/// A call of `operation` of the path `lax $.a`, of the arguments `passing`.
fn call(passing: Vec<(String, bool)>, operation: JsonOperation) -> JsonCall {
    JsonCall::new("lax $.a", parse_path("lax $.a", Clocked::default()).unwrap(), passing, operation)
}

/// JSON_QUERY of every clause written as its implicit value.
fn query() -> JsonOperation {
    JsonOperation::Query {
        returning: DataType::varying(8),
        wrapper: Wrapper::Conditional,
        quotes: Quotes::Omit,
        on_empty: QueryBehavior::EmptyArray,
        on_error: QueryBehavior::EmptyObject,
    }
}

/// Verifies: [DESIGN] RULES.md D84: a call shows its operator and every
/// clause, each explicit, so two calls of other clauses are two
/// expressions; a path's quotation mark is doubled, as a literal's.
#[test]
fn a_call_shows_its_clauses() {
    let exists = JsonCall::new(
        "lax $.a ? (@ == \"it's\")",
        parse_path("lax $", Clocked::default()).unwrap(),
        vec![("X".into(), true)],
        JsonOperation::Exists(ExistsBehavior::Unknown),
    );
    assert_eq!(exists.name(), "JSON_EXISTS 'lax $.a ? (@ == \"it''s\")' PASSING FORMAT JSON AS \"X\" UNKNOWN ON ERROR");
    assert_eq!(
        call(vec![], query()).name(),
        "JSON_QUERY 'lax $.a' RETURNING CHARACTER VARYING(8) FORMAT JSON WITH CONDITIONAL ARRAY WRAPPER OMIT QUOTES EMPTY ARRAY ON EMPTY \
         EMPTY OBJECT ON ERROR"
    );
    let value = JsonOperation::Value { returning: DataType::Boolean, on_empty: ValueSlot::Error, on_error: ValueSlot::Default };
    assert_eq!(
        call(vec![("a".into(), false)], value).name(),
        "JSON_VALUE 'lax $.a' PASSING AS \"a\" RETURNING BOOLEAN ERROR ON EMPTY DEFAULT ON ERROR"
    );
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §20.7 GR5a] A `?` context
/// item or argument FORMAT JSON is CHARACTER VARYING, one without it none;
/// past the arguments, of JSON_VALUE a DEFAULT of the returned type, of
/// the others nothing.
#[test]
fn dynamic_parameters_20_7() {
    let passing = vec![("A".to_owned(), true), ("B".to_owned(), false)];
    let f = call(passing.clone(), query());
    assert_eq!(f.parameter_type(0, &[]), Some(ParameterType::Varying));
    assert_eq!(f.parameter_type(1, &[]), Some(ParameterType::Varying));
    assert_eq!(f.parameter_type(2, &[]), None);
    assert_eq!(f.parameter_type(3, &[]), None);
    let exists = call(passing.clone(), JsonOperation::Exists(ExistsBehavior::True));
    assert_eq!(exists.parameter_type(3, &[]), None);
    let value = call(passing, JsonOperation::Value { returning: DataType::Date, on_empty: ValueSlot::Default, on_error: ValueSlot::Null });
    assert_eq!(value.parameter_type(3, &[]), Some(ParameterType::Of(DataType::Date)));
}

/// Verifies: [DESIGN] RULES.md R-ERR-1: a call of fewer arguments than it
/// was bound with, of no context item, an argument or a DEFAULT missing,
/// is SQLCODE -910, never a panic; of them all, its value.
#[test]
fn a_call_short_of_arguments_is_refused() {
    let value = JsonOperation::Value { returning: DataType::varying(4), on_empty: ValueSlot::Default, on_error: ValueSlot::Error };
    let f = call(vec![("A".to_owned(), false)], value);
    let text = |t: &str| Value::Char(t.into());
    for short in [vec![], vec![text("{}")], vec![text("{}"), text("1")]] {
        assert_eq!(f.call(&short).unwrap_err().code, Code::Api, "{short:?}");
    }
    assert_eq!(f.call(&[text("{}"), text("1"), text("dflt")]).unwrap().to_string(), "dflt");
    assert_eq!(f.return_type(&[]).unwrap(), DataType::varying(4));
}

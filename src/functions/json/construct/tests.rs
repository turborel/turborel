//! Tests of a call of an SQL/JSON constructor, of IS JSON and of the JSON
//! aggregates as functions: the types of their `?` operands, their
//! arguments, and an aggregate's state that could not be made.

use super::*;
use crate::functions::{AggregateFunction, JsonObjectAggregate};

/// A character string's value.
fn text(t: &str) -> Value {
    Value::Char(t.to_owned())
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §20.7 GR5a] RULES.md R-TEST-8:
/// a `?` is CHARACTER VARYING where it is a name or a value FORMAT JSON,
/// and none where it is a value without FORMAT JSON, by the pairs of the
/// decision "a name or FORMAT JSON": a name (true, of the first
/// condition), a value FORMAT JSON (true, of the second), a value without
/// it (false of both); of JSON_ARRAY, by its element's FORMAT JSON alone.
#[test]
fn dynamic_parameters_20_7() {
    let object = JsonConstruct::new(Construction::Object(OnNull::Null, Keys::Any), vec![true, false], DataType::varying(9));
    let types: Vec<Option<ParameterType>> = (0..4).map(|p| object.parameter_type(p, &[])).collect();
    assert_eq!(types, [Some(ParameterType::Varying), Some(ParameterType::Varying), Some(ParameterType::Varying), None]);
    let array = JsonConstruct::new(Construction::Array(OnNull::Absent), vec![false, true], DataType::varying(9));
    assert_eq!((array.parameter_type(0, &[]), array.parameter_type(1, &[])), (None, Some(ParameterType::Varying)));
    assert_eq!(array.parameter_type(5, &[]), None, "past its elements");
    let is_json = IsJsonCall::new(true, JsonType::Scalar, Keys::Unique);
    assert_eq!(is_json.parameter_type(0, &[]), Some(ParameterType::Varying));
    assert_eq!(is_json.name(), "IS NOT JSON SCALAR WITH UNIQUE KEYS");
    assert_eq!(is_json.return_type(&[]).unwrap(), DataType::Boolean);
}

/// Verifies: [DESIGN] RULES.md R-ERR-1: a call of other arguments than it
/// was bound with is SQLCODE -910, never a panic; of them, its value.
#[test]
fn a_call_of_other_arguments_is_refused() {
    let object = JsonConstruct::new(Construction::Object(OnNull::Absent, Keys::Unique), vec![false], DataType::varying(20));
    for wrong in [vec![text("a")], vec![text("a"), text("b"), text("c")], vec![]] {
        assert_eq!(object.call(&wrong).unwrap_err().code, Code::Api, "{wrong:?}");
    }
    assert_eq!(object.call(&[text("a"), Value::Null]).unwrap().to_string(), "{}");
    assert!(object.name().starts_with("JSON_OBJECT[? VALUE ? ABSENT ON NULL WITH UNIQUE KEYS"), "{}", object.name());
    let array = JsonConstruct::new(Construction::Array(OnNull::Null), vec![false], DataType::varying(20));
    assert_eq!(array.call(&[]).unwrap_err().code, Code::Api);
    assert_eq!(array.call(&[Value::Null]).unwrap().to_string(), "[null]");
    assert_eq!(IsJsonCall::new(false, JsonType::Value, Keys::Any).call(&[]).unwrap_err().code, Code::Api);
}

/// Verifies: [DESIGN] RULES.md R-ERR-1: an aggregate whose state cannot
/// be made, of a type returned the binder refuses, gives that error as it
/// is given a row and finished, never a panic; and one given another
/// argument than a row is -910.
#[test]
fn an_aggregate_of_no_state_is_refused() {
    let f = JsonObjectAggregate { on_null: OnNull::Null, keys: Keys::Any, returning: DataType::Boolean, format: None };
    let mut state = f.accumulator(DataType::Boolean);
    let row = Value::Row(std::sync::Arc::from(vec![text("a"), text("b")]));
    assert_eq!(state.update(&row).unwrap_err().code, Code::SyntaxRule);
    assert_eq!(state.finish().unwrap_err().code, Code::SyntaxRule);
    let good = JsonObjectAggregate { returning: DataType::varying(9), ..f };
    let mut made = good.accumulator(DataType::varying(9));
    assert_eq!(made.update(&text("a")).unwrap_err().code, Code::Api);
    assert_eq!(made.update(&Value::Row(std::sync::Arc::from(vec![text("a")]))).unwrap_err().code, Code::Api);
    assert_eq!(good.result_type(None).unwrap_err().code, Code::Api);
}

//! \[EXTENSION\] JSON_CONTAINS(document, candidate): whether the JSON text
//! `candidate` is contained in the JSON text `document`, which a json index
//! answers (RULES.md D34). No standard defines it.
//!
//! An item contains another when:
//!
//! * the candidate is a scalar or the SQL/JSON null, and the item is an
//!   equal one: strings by their code points, numbers by value, truth
//!   values alike, the null the null;
//! * the candidate is an object, the item is an object, and each member
//!   of the candidate is contained by a member of the item of its key;
//! * the candidate is an array, and each of its elements is contained by
//!   the item, or, if the item is an array, by one of its elements;
//! * else, the item is an array and one of its elements contains it.
//!
//! So an array stands for its elements, as lax mode unwraps one, and a
//! json index, which drops array steps from its paths, finds every
//! document a candidate is contained in: each scalar of the candidate is
//! one of the document under the same member names.

use std::cmp::Ordering;

use crate::error::{Code, Result, SqlError};
use crate::functions::lobs::like::held_text;
use crate::functions::{JsonOperator, JsonProbe, OnError, ScalarFunction};
use crate::json::{Item, Keys, MAX_NESTING, parse};
use crate::params::ParameterType;
use crate::value::lob::LobKind;
use crate::value::{DataType, Value, compare};

/// The function's name.
pub const JSON_CONTAINS: &str = "JSON_CONTAINS";

/// What a large object read whole as JSON text is, for its error.
const JSON_TEXT: &str = "JSON text of JSON_CONTAINS";

/// \[EXTENSION\] JSON_CONTAINS(document, candidate), BOOLEAN.
pub struct JsonContains;

/// The JSON text `value` is: none if null.
///
/// # Errors
///
/// SQLCODE -900 of a character large object longer than one chunk; -103
/// of no character string, which the binder refuses first.
fn text_of(value: &Value) -> Result<Option<String>> {
    match value {
        Value::Null => Ok(None),
        Value::Char(text) => Ok(Some(text.clone())),
        Value::Lob(lob) => match lob.kind() {
            LobKind::Character => held_text(lob, JSON_TEXT).map(Some),
            LobKind::Binary => Err(not_text(value)),
        },
        Value::Exact(_)
        | Value::Approx(_)
        | Value::DecFloat(_)
        | Value::Binary(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Row(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Json(_)
        | Value::Extension(_) => Err(not_text(value)),
    }
}

/// The error of a value that is no character string.
fn not_text(value: &Value) -> SqlError {
    SqlError::new(Code::SyntaxRule, format!("[EXTENSION] {JSON_CONTAINS} reads JSON text of character strings, not {value}"))
}

impl ScalarFunction for JsonContains {
    fn name(&self) -> &str {
        JSON_CONTAINS
    }
    /// BOOLEAN, of two character strings.
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let texts = args.len() == 2 && args.iter().all(DataType::is_character);
        if texts {
            return Ok(DataType::Boolean);
        }
        Err(SqlError::new(Code::SyntaxRule, format!("[EXTENSION] {JSON_CONTAINS} takes two character strings of JSON text")))
    }
    /// Each `?` a CHARACTER VARYING, as JSON text is.
    fn parameter_type(&self, _position: usize, _args: &[Option<DataType>]) -> Option<ParameterType> {
        Some(ParameterType::Varying)
    }
    /// Null if either is; else whether the candidate is contained.
    ///
    /// # Errors
    ///
    /// SQLSTATE 22032 if either is no JSON text; -900 as reading it.
    fn call(&self, args: &[Value]) -> Result<Value> {
        let [doc, candidate] = args else {
            return Err(SqlError::new(Code::Api, format!("[DESIGN] {JSON_CONTAINS} is called with two arguments")));
        };
        let (Some(doc), Some(candidate)) = (text_of(doc)?, text_of(candidate)?) else { return Ok(Value::Null) };
        let (doc, candidate) = (parse(&doc, Keys::Any)?, parse(&candidate, Keys::Any)?);
        Ok(Value::Bool(contains(&doc, &candidate, 0)))
    }
    /// Its errors are of reading its texts, raised.
    fn json_probe(&self) -> Option<JsonProbe<'_>> {
        Some(JsonProbe {
            operator: JsonOperator::Contains,
            path: None,
            passing: &[],
            returning: None,
            on_error: OnError::Raised,
            empty_null: true,
        })
    }
}

/// Whether `item` contains `candidate`, `depth` arrays and objects down.
/// Parsed items nest no deeper than `MAX_NESTING`; deeper is not found.
pub(crate) fn contains(item: &Item, candidate: &Item, depth: usize) -> bool {
    if depth > MAX_NESTING.saturating_mul(2) {
        return false;
    }
    let next = depth.saturating_add(1);
    if let Item::Array(wanted) = candidate {
        return wanted.iter().all(|w| match item {
            Item::Array(elements) => elements.iter().any(|e| contains(e, w, next)),
            Item::Null | Item::Scalar(_) | Item::Object(_) => contains(item, w, next),
        });
    }
    match (item, candidate) {
        (Item::Array(elements), _) => elements.iter().any(|e| contains(e, candidate, next)),
        (Item::Object(members), Item::Object(wanted)) => {
            wanted.iter().all(|w| members.iter().any(|m| m.key == w.key && contains(&m.value, &w.value, next)))
        }
        (Item::Null, Item::Null) => true,
        (Item::Scalar(a), Item::Scalar(b)) => scalars_equal(a.value(), b.value()),
        (Item::Null | Item::Scalar(_) | Item::Object(_), _) => false,
    }
}

/// Whether two scalars are equal: strings by their code points, others as
/// SQL compares them.
fn scalars_equal(a: &Value, b: &Value) -> bool {
    if let (Value::Char(x), Value::Char(y)) = (a, b) {
        return x == y;
    }
    compare(a, b) == Some(Ordering::Equal)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn holds(doc: &str, candidate: &str) -> bool {
        let made = JsonContains.call(&[Value::Char(doc.into()), Value::Char(candidate.into())]).unwrap();
        let Value::Bool(b) = made else { panic!("not a truth value: {made:?}") };
        b
    }

    /// Verifies: [EXTENSION] JSON_CONTAINS: scalars by value, objects by
    /// their members' keys, arrays standing for their elements, and null of
    /// a null argument.
    #[test]
    fn json_contains_follows_its_rules() {
        assert!(holds(r#"{"a":1,"b":[2,3]}"#, r#"{"a":1.0}"#));
        assert!(holds(r#"{"a":1,"b":[2,3]}"#, r#"{"b":3}"#));
        assert!(holds(r#"{"a":1,"b":[2,3]}"#, r#"{"b":[3,2]}"#));
        assert!(!holds(r#"{"a":1}"#, r#"{"a":"1"}"#));
        assert!(!holds(r#"{"a":"x "}"#, r#"{"a":"x"}"#), "strings by code points");
        assert!(holds(r#"[{"a":null}]"#, r#"{"a":null}"#));
        assert!(!holds(r#"{"a":{"b":1}}"#, r#"{"b":1}"#));
        assert!(holds("{}", "[]"), "an empty array asks nothing");
        assert!(JsonContains.call(&[Value::Null, Value::Char("1".into())]).unwrap().is_null());
        assert!(JsonContains.call(&[Value::Char("{".into()), Value::Char("1".into())]).is_err());
    }
}

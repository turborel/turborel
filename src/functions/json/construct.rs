//! [ISO/IEC 9075-2:2023 §6.34, §8.22] JSON_OBJECT and JSON_ARRAY by
//! enumeration, and the `<JSON predicate>` IS JSON, each call a function
//! of its own that holds its clauses, explicit or implicit, and applies
//! their General Rules (`json/construct.rs`, `json/predicate.rs`) to its
//! operands' values. A JSON_OBJECT's arguments are each member's name and
//! value in turn, a JSON_ARRAY's its elements, an IS JSON's its input.

use super::super::ScalarFunction;
use crate::error::{Code, Result, SqlError};
use crate::json::{
    ArrayOptions, Input, JsonType, Keys, NameAndValue, ObjectOptions, OnNull, is_json, is_not_json, json_array, json_object,
};
use crate::params::ParameterType;
use crate::value::{DataType, Value};

// [ISO/IEC 9075-2:2023 §5.2] The functions' names, reserved words of
// SQL:2023 and no key words of SQL-92.
/// JSON_OBJECT [§6.34].
pub const JSON_OBJECT: &str = "JSON_OBJECT";
/// JSON_ARRAY [§6.34].
pub const JSON_ARRAY: &str = "JSON_ARRAY";
/// JSON_OBJECTAGG [§10.11].
pub const JSON_OBJECTAGG: &str = "JSON_OBJECTAGG";
/// JSON_ARRAYAGG [§10.11].
pub const JSON_ARRAYAGG: &str = "JSON_ARRAYAGG";

/// [§6.34 F] How many operands a `<JSON name and value>` of JSON_OBJECT is
/// bound to: its `<JSON name>` and its `<JSON input expression>`, in turn.
pub(crate) const OPERANDS_PER_MEMBER: usize = 2;

/// [§6.34] What a constructor makes, of its clauses explicit or implicit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Construction {
    /// JSON_OBJECT, of its null clause and key uniqueness constraint.
    Object(OnNull, Keys),
    /// JSON_ARRAY by enumeration, of its null clause.
    Array(OnNull),
}

/// A call of JSON_OBJECT or of JSON_ARRAY by enumeration: whether each
/// member's value, or each element, is FORMAT JSON, what it makes, and the
/// type it returns.
pub struct JsonConstruct {
    /// What the call shows as its name: the function's and every clause,
    /// so two calls of other clauses are never one expression.
    label: String,
    /// Per member or element, whether its `<JSON input clause>` is FORMAT
    /// JSON.
    formats: Vec<bool>,
    /// What it makes.
    construction: Construction,
    /// The type of its `<JSON output clause>`.
    returning: DataType,
}

impl JsonConstruct {
    /// The call of `construction` returning `returning`, of members or
    /// elements each FORMAT JSON as `formats` says.
    pub fn new(construction: Construction, formats: Vec<bool>, returning: DataType) -> JsonConstruct {
        let shown: Vec<&str> = formats.iter().map(|f| if *f { "? FORMAT JSON" } else { "?" }).collect();
        let label = match construction {
            Construction::Object(on_null, keys) => {
                let members: Vec<String> = shown.iter().map(|value| format!("? VALUE {value}")).collect();
                format!("{JSON_OBJECT}[{} {} {} RETURNING {returning}]", members.join(", "), null_words(on_null), keys_words(keys))
            }
            Construction::Array(on_null) => format!("{JSON_ARRAY}[{} {} RETURNING {returning}]", shown.join(", "), null_words(on_null)),
        };
        JsonConstruct { label, formats, construction, returning }
    }
}

/// [§6.34] A `<JSON constructor null clause>` as written.
pub(super) fn null_words(on_null: OnNull) -> &'static str {
    match on_null {
        OnNull::Null => "NULL ON NULL",
        OnNull::Absent => "ABSENT ON NULL",
    }
}

/// [§8.22] A `<JSON key uniqueness constraint>` as written.
pub(super) fn keys_words(keys: Keys) -> &'static str {
    match keys {
        Keys::Any => "WITHOUT UNIQUE KEYS",
        Keys::Unique => "WITH UNIQUE KEYS",
    }
}

/// The error of a call given other arguments than the binder gives it.
fn arity() -> SqlError {
    SqlError::new(Code::Api, "[DESIGN] an SQL/JSON constructor is called with other arguments than it was bound with")
}

impl ScalarFunction for JsonConstruct {
    fn name(&self) -> &str {
        &self.label
    }
    /// [§6.34 SR6d, SR7b] The type of its `<JSON output clause>`. The
    /// binder checks the operands' types.
    fn return_type(&self, _args: &[DataType]) -> Result<DataType> {
        Ok(self.returning.clone())
    }
    /// \[EXTENSION\] [§20.7 GR5a] No rule types a `?` here: a name, a
    /// `<character value expression>`, or a value FORMAT JSON, JSON text,
    /// is CHARACTER VARYING; a value without FORMAT JSON none, so it is
    /// refused \[GR5b\].
    fn parameter_type(&self, position: usize, _args: &[Option<DataType>]) -> Option<ParameterType> {
        let (member, name) = match self.construction {
            Construction::Object(..) => (position.checked_div(OPERANDS_PER_MEMBER)?, position.checked_rem(OPERANDS_PER_MEMBER)? == 0),
            Construction::Array(_) => (position, false),
        };
        let formatted = self.formats.get(member).copied().unwrap_or(false);
        (name || formatted).then_some(ParameterType::Varying)
    }
    /// [§6.34 GR2, GR3, GR5a] The JSON text of the object or array made.
    fn call(&self, args: &[Value]) -> Result<Value> {
        match self.construction {
            Construction::Object(on_null, keys) => {
                let (pairs, rest) = args.as_chunks::<OPERANDS_PER_MEMBER>();
                if !rest.is_empty() || pairs.len() != self.formats.len() {
                    return Err(arity());
                }
                let members: Vec<NameAndValue<'_>> = pairs
                    .iter()
                    .zip(&self.formats)
                    .map(|([name, value], format_json)| NameAndValue { name, value: Input { value, format_json: *format_json } })
                    .collect();
                json_object(&members, &ObjectOptions { on_null, keys, returning: &self.returning })
            }
            Construction::Array(on_null) => {
                if args.len() != self.formats.len() {
                    return Err(arity());
                }
                let elements: Vec<Input<'_>> =
                    args.iter().zip(&self.formats).map(|(value, format_json)| Input { value, format_json: *format_json }).collect();
                json_array(&elements, &ArrayOptions { on_null, returning: &self.returning })
            }
        }
    }
}

/// A `<JSON predicate>`: IS \[NOT\] JSON of its type constraint and key
/// uniqueness constraint, each explicit or implicit, of one argument, its
/// input.
pub struct IsJsonCall {
    /// What the call shows as its name.
    label: String,
    /// IS NOT JSON.
    not: bool,
    /// The `<JSON predicate type constraint>`.
    kind: JsonType,
    /// The `<JSON key uniqueness constraint>`.
    keys: Keys,
}

impl IsJsonCall {
    /// IS JSON, or IS NOT JSON if `not`, of the type `kind` and the
    /// uniqueness `keys`.
    pub fn new(not: bool, kind: JsonType, keys: Keys) -> IsJsonCall {
        let kind_word = match kind {
            JsonType::Value => "VALUE",
            JsonType::Array => "ARRAY",
            JsonType::Object => "OBJECT",
            JsonType::Scalar => "SCALAR",
        };
        let label = format!("IS {}JSON {kind_word} {}", if not { "NOT " } else { "" }, keys_words(keys));
        IsJsonCall { label, not, kind, keys }
    }
}

impl ScalarFunction for IsJsonCall {
    fn name(&self) -> &str {
        &self.label
    }
    /// BOOLEAN, of the truth value of the predicate [§8.22 GR3].
    fn return_type(&self, _args: &[DataType]) -> Result<DataType> {
        Ok(DataType::Boolean)
    }
    /// \[EXTENSION\] [§20.7 GR5a] No rule types a `?` here: the input, JSON
    /// text, is CHARACTER VARYING.
    fn parameter_type(&self, _position: usize, _args: &[Option<DataType>]) -> Option<ParameterType> {
        Some(ParameterType::Varying)
    }
    /// [§8.22 GR, SR4] True, False or Unknown, the null value.
    fn call(&self, args: &[Value]) -> Result<Value> {
        let [input] = args else { return Err(arity()) };
        if self.not { is_not_json(input, self.kind, self.keys) } else { is_json(input, self.kind, self.keys) }
    }
}

#[cfg(test)]
mod tests;

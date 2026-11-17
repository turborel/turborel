//! [ISO/IEC 9075-2:2023 §4.48.4] The SQL/JSON query operators: JSON_EXISTS
//! [§8.23], JSON_VALUE [§6.28] and JSON_QUERY [§6.35], each call a
//! function of its own (`JsonCall`), as each regular expression function's
//! is (RULES.md D61), holding its path expression parsed once, as the
//! statement is bound (RULES.md D84), and its clauses, each explicit or
//! implicit. It applies the operator's General Rules (`json.rs`) to its
//! operands' values.
//!
//! A call's arguments are its operands' values in this order: the context
//! item's, each PASSING argument's, then the DEFAULT values of ON EMPTY
//! and of ON ERROR, of those that are DEFAULT.

use super::ScalarFunction;
use crate::error::{Code, Result, SqlError};
use crate::json::{
    CommonSyntax, ExistsBehavior, Passing, Path, QueryBehavior, QueryOptions, Quotes, ValueBehavior, Warned, Wrapper, json_exists,
    json_query, json_value,
};
use crate::params::ParameterType;
use crate::value::{DataType, Value};

// [ISO/IEC 9075-2:2023 §5.2] The operators' names, reserved words of
// SQL:2023 and no key words of SQL-92.
/// JSON_EXISTS [§8.23].
pub const JSON_EXISTS: &str = "JSON_EXISTS";
/// JSON_VALUE [§6.28].
pub const JSON_VALUE: &str = "JSON_VALUE";
/// JSON_QUERY [§6.35].
pub const JSON_QUERY: &str = "JSON_QUERY";

/// [§6.28] A `<JSON value empty behavior>` or `<JSON value error
/// behavior>`, bound: a DEFAULT's value is an argument of the call, after
/// the PASSING arguments', ON EMPTY's first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueSlot {
    /// ERROR.
    Error,
    /// NULL.
    Null,
    /// DEFAULT `<value expression>`.
    Default,
}

/// The operator a call is, and its clauses, each explicit or implicit.
#[derive(Debug, Clone)]
pub enum JsonOperation {
    /// [§8.23] JSON_EXISTS, of its behavior ON ERROR.
    Exists(ExistsBehavior),
    /// [§6.28] JSON_VALUE.
    Value {
        /// The `<data type>` of the `<JSON returning clause>`.
        returning: DataType,
        /// The behavior ON EMPTY.
        on_empty: ValueSlot,
        /// The behavior ON ERROR.
        on_error: ValueSlot,
    },
    /// [§6.35] JSON_QUERY.
    Query {
        /// The `<data type>` of the `<JSON output clause>`.
        returning: DataType,
        /// The `<JSON query wrapper behavior>`.
        wrapper: Wrapper,
        /// The `<JSON query quotes behavior>`.
        quotes: Quotes,
        /// The behavior ON EMPTY.
        on_empty: QueryBehavior,
        /// The behavior ON ERROR.
        on_error: QueryBehavior,
    },
}

/// A call of an SQL/JSON query operator: its path expression parsed, the
/// identifier and FORMAT JSON of each PASSING argument, and its
/// operation.
pub struct JsonCall {
    /// What the call shows as its name: the operator's, and every clause
    /// but its operands, so that two calls of the same operands are one
    /// expression only if they are one call (as a set function's label
    /// tells them apart).
    label: String,
    /// The `<JSON path specification>`, parsed.
    path: Path,
    /// Each `<JSON argument>`'s identifier, and whether it is FORMAT JSON.
    passing: Vec<(String, bool)>,
    /// The operator, and its clauses.
    operation: JsonOperation,
}

impl JsonCall {
    /// The call of `operation`, of the path expression `text` parsed to
    /// `path`, and of the PASSING arguments `passing`, each its
    /// identifier's value and whether it is FORMAT JSON.
    pub fn new(text: &str, path: Path, passing: Vec<(String, bool)>, operation: JsonOperation) -> JsonCall {
        let label = label(text, &passing, &operation);
        JsonCall { label, path, passing, operation }
    }
}

/// The name a call of `operation` shows, of the path expression `text` and
/// of the arguments `passing`: as it is written, each clause explicit.
fn label(text: &str, passing: &[(String, bool)], operation: &JsonOperation) -> String {
    let name = match operation {
        JsonOperation::Exists(_) => JSON_EXISTS,
        JsonOperation::Value { .. } => JSON_VALUE,
        JsonOperation::Query { .. } => JSON_QUERY,
    };
    let mut shown = format!("{name} '{}'", text.replace('\'', "''"));
    let arguments: Vec<String> =
        passing.iter().map(|(id, format_json)| format!("{}AS \"{}\"", if *format_json { "FORMAT JSON " } else { "" }, id)).collect();
    if !arguments.is_empty() {
        shown = format!("{shown} PASSING {}", arguments.join(", "));
    }
    let clauses = match operation {
        JsonOperation::Exists(on_error) => format!("{} ON ERROR", exists_word(*on_error)),
        JsonOperation::Value { returning, on_empty, on_error } => {
            format!("RETURNING {returning} {} ON EMPTY {} ON ERROR", slot_word(*on_empty), slot_word(*on_error))
        }
        JsonOperation::Query { returning, wrapper, quotes, on_empty, on_error } => format!(
            "RETURNING {returning} FORMAT JSON {} WRAPPER {} QUOTES {} ON EMPTY {} ON ERROR",
            wrapper_words(*wrapper),
            quotes_word(*quotes),
            query_words(*on_empty),
            query_words(*on_error)
        ),
    };
    format!("{shown} {clauses}")
}

/// [§8.23] A `<JSON exists error behavior>` as written.
fn exists_word(behavior: ExistsBehavior) -> &'static str {
    match behavior {
        ExistsBehavior::True => "TRUE",
        ExistsBehavior::False => "FALSE",
        ExistsBehavior::Unknown => "UNKNOWN",
        ExistsBehavior::Error => "ERROR",
    }
}

/// [§6.28] A `<JSON value empty behavior>` as written, DEFAULT without
/// its value, an argument.
fn slot_word(slot: ValueSlot) -> &'static str {
    match slot {
        ValueSlot::Error => "ERROR",
        ValueSlot::Null => "NULL",
        ValueSlot::Default => "DEFAULT",
    }
}

/// [§6.35] A `<JSON query wrapper behavior>` as written.
fn wrapper_words(wrapper: Wrapper) -> &'static str {
    match wrapper {
        Wrapper::Without => "WITHOUT ARRAY",
        Wrapper::Conditional => "WITH CONDITIONAL ARRAY",
        Wrapper::Unconditional => "WITH UNCONDITIONAL ARRAY",
    }
}

/// [§6.35] A `<JSON query quotes behavior>` as written.
fn quotes_word(quotes: Quotes) -> &'static str {
    match quotes {
        Quotes::Keep => "KEEP",
        Quotes::Omit => "OMIT",
    }
}

/// [§6.35] A `<JSON query empty behavior>` as written.
fn query_words(behavior: QueryBehavior) -> &'static str {
    match behavior {
        QueryBehavior::Error => "ERROR",
        QueryBehavior::Null => "NULL",
        QueryBehavior::EmptyArray => "EMPTY ARRAY",
        QueryBehavior::EmptyObject => "EMPTY OBJECT",
    }
}

/// The error of a call given other arguments than the binder gives it.
fn arity() -> SqlError {
    SqlError::new(Code::Api, "[DESIGN] an SQL/JSON query operator is called with other arguments than it was bound with")
}

/// [§9.48 GR4c iii, GR5d] The behavior `slot` with its DEFAULT value, the
/// next of `defaults` if it is DEFAULT.
///
/// # Errors
///
/// SQLCODE -910 if `defaults` has no value left.
fn behavior<'a>(slot: ValueSlot, defaults: &mut impl Iterator<Item = &'a Value>) -> Result<ValueBehavior> {
    match slot {
        ValueSlot::Error => Ok(ValueBehavior::Error),
        ValueSlot::Null => Ok(ValueBehavior::Null),
        ValueSlot::Default => defaults.next().cloned().map(ValueBehavior::Default).ok_or_else(arity),
    }
}

impl ScalarFunction for JsonCall {
    fn name(&self) -> &str {
        &self.label
    }
    /// [§8.23] BOOLEAN; [§6.28 SR3, §6.35 SR2] the returned type. The
    /// binder checks the operands' types, by the operator's Syntax Rules.
    fn return_type(&self, _args: &[DataType]) -> Result<DataType> {
        Ok(match &self.operation {
            JsonOperation::Exists(_) => DataType::Boolean,
            JsonOperation::Value { returning, .. } | JsonOperation::Query { returning, .. } => returning.clone(),
        })
    }
    /// \[EXTENSION\] [§20.7 GR5a] No rule types a `?` here, and a JSON text
    /// is a character string: the context item, or an argument FORMAT
    /// JSON, CHARACTER VARYING; a DEFAULT the returned type, as CAST's
    /// operand takes its target's \[GR5a i\]; an argument without FORMAT
    /// JSON none, so it is refused \[GR5b\].
    fn parameter_type(&self, position: usize, _args: &[Option<DataType>]) -> Option<ParameterType> {
        let Some(argument) = position.checked_sub(1) else { return Some(ParameterType::Varying) };
        if let Some((_, format_json)) = self.passing.get(argument) {
            return format_json.then_some(ParameterType::Varying);
        }
        match &self.operation {
            JsonOperation::Value { returning, .. } => Some(ParameterType::Of(returning.clone())),
            JsonOperation::Exists(_) | JsonOperation::Query { .. } => None,
        }
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        self.call_warned(args, None).map(|(value, _)| value)
    }
    /// A cast the operator makes may cut a string, warning 01004.
    fn warns(&self) -> bool {
        true
    }
    /// [§8.23 GR, §6.28 GR, §6.35 GR] The operator's result, and the
    /// warning a cast it made raised, if any.
    fn call_warned(&self, args: &[Value], _temp: Option<&dyn crate::value::lob::TempLobs>) -> Result<Warned> {
        let (context, rest) = args.split_first().ok_or_else(arity)?;
        let (values, defaults) = rest.split_at_checked(self.passing.len()).ok_or_else(arity)?;
        let passing: Vec<Passing<'_>> = self
            .passing
            .iter()
            .zip(values)
            .map(|((name, format_json), value)| Passing { name, value, format_json: *format_json })
            .collect();
        let common = CommonSyntax { context, path: &self.path, passing: &passing };
        match &self.operation {
            JsonOperation::Exists(on_error) => json_exists(&common, *on_error).map(|value| (value, None)),
            JsonOperation::Value { returning, on_empty, on_error } => {
                let mut defaults = defaults.iter();
                let on_empty = behavior(*on_empty, &mut defaults)?;
                let on_error = behavior(*on_error, &mut defaults)?;
                json_value(&common, returning, &on_empty, &on_error)
            }
            JsonOperation::Query { returning, wrapper, quotes, on_empty, on_error } => {
                let options = QueryOptions { returning, wrapper: *wrapper, quotes: *quotes, on_empty: *on_empty, on_error: *on_error };
                json_query(&common, &options)
            }
        }
    }
}

#[cfg(test)]
mod tests;

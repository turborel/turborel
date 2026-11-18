//! The SQL/JSON query operators [ISO/IEC 9075-2:2023 §4.48.4]:
//! JSON_EXISTS [§8.23], JSON_VALUE [§6.28] and JSON_QUERY [§6.35], by
//! their General Rules, of inputs already evaluated; and the rules they
//! share: processing the `<JSON API common syntax>` [§9.47], casting an
//! SQL/JSON sequence to an SQL type [§9.48], converting one to an
//! SQL/JSON item [§9.44] and serializing one to a character string type
//! [§9.49].
//!
//! This file processes the common syntax: the context item and the
//! PASSING clause's arguments made SQL/JSON items, and the path evaluated
//! of them [§9.46 GR1 to GR5, GR11a, GR11b]. JSON_EXISTS is in
//! `query/exists.rs`, JSON_VALUE and §9.48 in `query/value.rs`, the
//! wrapper behaviours and §9.44 in `query/wrapper.rs`, JSON_QUERY and
//! §9.49 in `query/json_query.rs`.
//!
//! The General Rules pass an exception condition on as a STATUS, which
//! an ON ERROR behaviour may replace by a value. \[DESIGN\] An exception
//! condition is an error of SQLSTATE class 22, data exception, whatever
//! raised it: the context item's or an argument's JSON text parsed
//! \[§9.42\], the path evaluated \[§9.46 GR11\], an item cast \[§9.48 GR4b
//! iii 2\]. Every other error is raised as it is, ON ERROR or not: a
//! Syntax Rule broken, as of a named variable no argument names (SQLCODE
//! -103) [§9.46 SR4], which no evaluation could make good, and a limit of
//! turborel's reached (SQLCODE -900, RULES.md D18), which is no exception
//! condition of the standard's.

mod exists;
mod json_query;
mod value;
mod wrapper;

pub use exists::{ExistsBehavior, json_exists};
pub use json_query::{QueryOptions, Quotes, json_query, serialize_sequence};
pub use value::{Taken, ValueBehavior, cast_sequence, json_value, json_value_taken};
pub use wrapper::{QueryBehavior, Wrapper, sequence_to_item};

use super::{Item, Keys, Path, Scalar, parse};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::functions::lobs::like::held_text;
use crate::value::lob::LobKind;
use crate::value::{DataType, MAX_CHARACTER_LENGTH, Value, cast};

/// \[DESIGN\] The implementation-defined string type JSON_VALUE returns
/// without a `<JSON returning clause>` (ID074) [§6.28 SR1], and JSON_QUERY
/// without a `<JSON output clause>`, FORMAT JSON (ID101) [§6.35 SR1b]:
/// CHARACTER VARYING of the most characters turborel's has.
pub const IMPLICIT_RETURNING: DataType = DataType::varying(MAX_CHARACTER_LENGTH);

/// \[DESIGN\] The implementation-defined character string type a
/// `<JSON argument>` without FORMAT JSON is cast to if it is of none of
/// the types an SQL/JSON scalar has (IV165) [§9.46 GR11b i 2 B]: as
/// `IMPLICIT_RETURNING`.
const ARGUMENT_STRING: DataType = DataType::varying(MAX_CHARACTER_LENGTH);

/// The value of the SQL/JSON operator and the completion condition, a
/// warning, a cast it made raised, if one did: string data, right
/// truncation, of a string cut to a CHARACTER VARYING type.
pub type Warned = (Value, Option<SqlState>);

/// [§10.14] A `<JSON API common syntax>`, its values evaluated.
#[derive(Debug, Clone, Copy)]
pub struct CommonSyntax<'a> {
    /// The value of the `<value expression>` of the `<JSON context item>`:
    /// a character string, its JSON text, FORMAT JSON [§10.14 SR1], or the
    /// null value. \[DESIGN\] turborel has no JSON type.
    pub context: &'a Value,
    /// The `<JSON path specification>`, parsed (`parse_path`).
    pub path: &'a Path,
    /// The `<JSON argument>`s of the `<JSON passing clause>`, in order;
    /// none without one.
    pub passing: &'a [Passing<'a>],
}

/// [§10.14] A `<JSON argument>`: `<JSON input expression> AS
/// <identifier>`.
#[derive(Debug, Clone, Copy)]
pub struct Passing<'a> {
    /// The `<identifier>`, as the `<JSON path identifier>` of a named
    /// variable is written after its `$`: \[DESIGN\] they are equivalent
    /// \[§9.46 SR4\] if they are the same string, so the SQL layer passes
    /// the name the path is to use.
    pub name: &'a str,
    /// The value of the `<value expression>`.
    pub value: &'a Value,
    /// Whether the `<JSON input expression>` has the `<JSON input clause>`
    /// FORMAT JSON [§10.12]: its value is then a character string's JSON
    /// text, parsed.
    pub format_json: bool,
}

/// [§9.47] The SQL/JSON sequence the path of `common` gives of its
/// context item and arguments; none if the context item is the null
/// value, of which each operator's General Rules give their own result.
///
/// \[§9.46 SR4\] That every named variable of the path has an argument is
/// checked first, as a Syntax Rule, even of a null context item.
/// \[§9.46 GR4, GR5\] It is implementation-defined (IA050, IA054) whether
/// the context item and the FORMAT JSON arguments are parsed before the
/// path is evaluated: turborel parses them first, each as JSON text
/// WITHOUT UNIQUE KEYS (IV185), so a text that is no JSON text is an
/// exception whether or not the path reads it.
///
/// # Errors
///
/// SQLCODE -103 if a named variable has no argument \[§9.46 SR4\], the
/// context item is no character string \[§10.14 SR1\], or a FORMAT JSON
/// argument is none \[§10.12 SR6\]; -900 if either is a binary string,
/// whose encodings turborel does not implement, or a character large
/// object longer than one chunk; else an exception condition: SQLSTATE
/// 22032 if a text is no JSON text [§9.42 GR3a iii 1], that of a cast of
/// an argument to a character string \[§9.46 GR11b i 2 B\], or that of
/// the path's evaluation \[§9.46 GR11\].
pub fn process(common: &CommonSyntax<'_>) -> Result<Option<Vec<Item>>> {
    let names: Vec<&str> = common.passing.iter().map(|argument| argument.name).collect();
    common.path.check_variables(&names)?;
    // [§4.48.4] A value of the JSON type is read in place, its path
    // evaluated on its stored form if it is of the shapes most are.
    if let Value::Json(j) = common.context
        && let Some(seq) = super::binary::fast_path(common.path, j.root()?)?
    {
        return Ok(Some(seq));
    }
    let Some(context) = json_input(common.context, CONTEXT_RULE, Keys::Any)? else { return Ok(None) };
    let mut variables = Vec::with_capacity(common.passing.len());
    for argument in common.passing {
        variables.push((argument.name.to_owned(), passed(argument)?));
    }
    common.path.evaluate(&context, &variables).map(Some)
}

/// \[DESIGN\] Whether `error` is an exception condition an ON ERROR
/// behaviour replaces, as the module's documentation says: a data
/// exception, SQLSTATE class 22.
pub fn is_exception(error: &SqlError) -> bool {
    error.sqlstate().class() == SqlState::DATA_EXCEPTION.class()
}

impl Item {
    /// [§9.46 GR11b i 2, ii 2] The SQL/JSON item the SQL value `value` is
    /// as a `<JSON argument>` without FORMAT JSON: the null value the
    /// SQL/JSON null; a character string, a number, a truth value or a
    /// datetime the scalar it is, a character large object's read whole;
    /// a value of another type cast to a character string (IV165,
    /// `ARGUMENT_STRING`). \[§10.12 SR3\] Its type is to be one a
    /// character string type can be cast from, which the SQL layer checks.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 if it is a character large object longer than one
    /// chunk; that of the cast, as SQLSTATE 22005 of a value CAST does not
    /// take to a character string.
    pub fn of_value(value: &Value) -> Result<Item> {
        let scalar = match value {
            Value::Null => return Ok(Item::Null),
            Value::Lob(lob) => match lob.kind() {
                LobKind::Character => Value::Char(held_text(lob, ARGUMENT_TEXT)?),
                LobKind::Binary => cast(value.clone(), &ARGUMENT_STRING)?,
            },
            Value::Char(_) | Value::Exact(_) | Value::Approx(_) | Value::DecFloat(_) | Value::Bool(_) | Value::Datetime(_) => value.clone(),
            Value::Binary(_)
            | Value::Interval(_)
            | Value::Array(_)
            | Value::Multiset(_)
            | Value::Row(_)
            | Value::Structured(_)
            | Value::Reference(_)
            | Value::Json(_)
            | Value::Extension(_) => cast(value.clone(), &ARGUMENT_STRING)?,
        };
        Ok(Scalar::new(scalar).map_or(Item::Null, Item::Scalar))
    }
}

/// The Syntax Rule a context item's type breaks if it is no string.
const CONTEXT_RULE: &str = "[ISO/IEC 9075-2:2023 §10.14 SR1] the context item of an SQL/JSON query operator is a string";
/// The Syntax Rule a FORMAT JSON argument's type breaks if it is no
/// string.
const ARGUMENT_RULE: &str = "[ISO/IEC 9075-2:2023 §10.12 SR6] an argument FORMAT JSON is a string";
/// What a character large object read whole as JSON text is, for the
/// error of one too long.
const JSON_TEXT: &str = "JSON text";
/// What a character large object read whole as an argument is.
const ARGUMENT_TEXT: &str = "an argument of an SQL/JSON query operator";

/// [§9.46 GR11b] The SQL/JSON sequence `argument` is: \[GR11b i 1\] of a
/// FORMAT JSON one, the item its JSON text is, none of the null value
/// \[GR11b i 1 A\]; \[GR11b i 2\] else the item its value is
/// (`Item::of_value`).
///
/// # Errors
///
/// As `process`, of the argument.
fn passed(argument: &Passing<'_>) -> Result<Vec<Item>> {
    if !argument.format_json && !matches!(argument.value, Value::Json(_)) {
        return Ok(vec![Item::of_value(argument.value)?]);
    }
    Ok(json_input(argument.value, ARGUMENT_RULE, Keys::Any)?.into_iter().collect())
}

/// [§10.12] The SQL/JSON item `value` is as JSON: \[SR5\] of the JSON
/// type, its item, FORMAT JSON implicit; else its JSON text parsed, keys
/// as `keys` says [§9.42]. None if it is the null value; `rule` is the
/// Syntax Rule a value of another type breaks.
///
/// # Errors
///
/// As `json_text`; SQLSTATE 22032 or 22037 of the parse; SQLCODE -916 if
/// a value of the JSON type is damaged.
pub(super) fn json_input(value: &Value, rule: &str, keys: Keys) -> Result<Option<Item>> {
    if let Value::Json(j) = value {
        return j.item().map(Some);
    }
    json_text(value, rule)?.map(|text| parse(&text, keys)).transpose()
}

/// [§10.12] The JSON text `value` is, a character string, FORMAT JSON;
/// none if it is the null value. `rule` is the Syntax Rule a value of
/// another type breaks.
///
/// # Errors
///
/// SQLCODE -103 of a value of another type than a string's; -900 of a
/// binary string, or a character large object longer than one chunk.
pub(super) fn json_text(value: &Value, rule: &str) -> Result<Option<String>> {
    match value {
        Value::Null => Ok(None),
        Value::Char(text) => Ok(Some(text.clone())),
        Value::Lob(lob) => match lob.kind() {
            LobKind::Character => held_text(lob, JSON_TEXT).map(Some),
            LobKind::Binary => Err(binary_text()),
        },
        Value::Binary(_) => Err(binary_text()),
        Value::Exact(_)
        | Value::Approx(_)
        | Value::DecFloat(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Row(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Json(_)
        | Value::Extension(_) => Err(SqlError::new(Code::SyntaxRule, format!("{rule}, not {value}"))),
    }
}

/// The error of JSON text in a binary string, whose encodings turborel
/// does not implement (RULES.md D18).
fn binary_text() -> SqlError {
    SqlError::new(
        Code::Unsupported,
        "[DESIGN] JSON text in a binary string, FORMAT JSON ENCODING, is valid SQL but is not implemented (RULES.md D18)",
    )
}

/// The data exception `state` of a General Rule, its message `cited`
/// beginning with the rule.
fn exception(state: SqlState, cited: &str) -> SqlError {
    SqlError::new(Code::Data, cited).with_state(state)
}

/// [§9.48 GR5, §9.44 GR5b, §9.49 GR6a] The value of TEMPST, a General
/// Rule's result or its exception condition: the result; an exception
/// condition, as `on_error` replaces it; any other error raised as it is.
///
/// # Errors
///
/// An error not an exception condition; else as `on_error`.
fn caught<T>(temp: Result<T>, on_error: impl FnOnce(SqlError) -> Result<T>) -> Result<T> {
    match temp {
        Ok(result) => Ok(result),
        Err(error) => {
            if is_exception(&error) {
                on_error(error)
            } else {
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests;

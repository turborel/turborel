//! JSON_EXISTS [ISO/IEC 9075-2:2023 §8.23], `<JSON exists predicate>`:
//! whether a path gives any SQL/JSON item of JSON text.

use super::{CommonSyntax, caught, process};
use crate::error::Result;
use crate::value::Value;

/// [§8.23] A `<JSON exists error behavior>`: the truth value JSON_EXISTS
/// gives in place of an exception condition. \[SR1\] Without one, FALSE
/// ON ERROR is implicit, `ExistsBehavior::False`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExistsBehavior {
    /// `TRUE`: True.
    True,
    /// `FALSE`: False; the default \[SR1\].
    False,
    /// `UNKNOWN`: Unknown, the null value.
    Unknown,
    /// `ERROR`: the exception condition, raised.
    Error,
}

/// [§8.23 GR] JSON_EXISTS of the `<JSON API common syntax>` `common` and
/// its `<JSON exists error behavior>` `on_error`, a truth value: Unknown,
/// the null value, if the context item is the null value \[GR1\]; else
/// whether the path gives an SQL/JSON item \[GR4a\], or, in place of an
/// exception condition, the truth value of `on_error` \[GR4b\]. An error
/// not an exception condition is raised (`query::is_exception`).
///
/// # Errors
///
/// As `process`: an error not an exception condition; the exception
/// condition if `on_error` is ERROR \[GR4b i\].
pub fn json_exists(common: &CommonSyntax<'_>, on_error: ExistsBehavior) -> Result<Value> {
    let Some(status) = process(common).transpose() else { return Ok(Value::Null) };
    caught(status.map(|seq| Value::Bool(!seq.is_empty())), |error| match on_error {
        ExistsBehavior::True => Ok(Value::Bool(true)),
        ExistsBehavior::False => Ok(Value::Bool(false)),
        ExistsBehavior::Unknown => Ok(Value::Null),
        ExistsBehavior::Error => Err(error),
    })
}

#[cfg(test)]
mod tests;

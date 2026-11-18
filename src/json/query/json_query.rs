//! JSON_QUERY [ISO/IEC 9075-2:2023 §6.35], `<JSON query>`: JSON text
//! taken of JSON text; and an SQL/JSON sequence serialized to an SQL
//! string type [§9.49], as it is serialized of a character string type.
//! \[DESIGN\] turborel has no JSON type, so the declared type is always a
//! string type [§6.35 SR1b] and §6.35 GR5b applies.

use super::wrapper::{QueryBehavior, Wrapper, only, wrapped};
use super::{CommonSyntax, Warned, caught, process};
use crate::error::{Result, SqlState};
use crate::json::Item;
use crate::json::serialize::{serialize_as, string_type};
use crate::value::{DataType, Value, cast_warned};

/// [§6.35] A `<JSON query quotes behavior>`: whether a character string
/// JSON_QUERY gives alone is serialized, in quotation marks, or cast.
/// \[SR9\] Without one, KEEP is implicit, `Quotes::Keep`; \[SR8\] none is
/// to be written with WITH a wrapper, which the SQL layer checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quotes {
    /// `KEEP [ QUOTES [ ON SCALAR STRING ] ]`: serialized; the default
    /// \[SR9\].
    Keep,
    /// `OMIT [ QUOTES [ ON SCALAR STRING ] ]`: cast [§9.49 GR5b].
    Omit,
}

/// [§6.35] What follows the `<JSON API common syntax>` of a JSON_QUERY:
/// each clause explicit or implicit, as its Syntax Rules make one.
#[derive(Debug, Clone, Copy)]
pub struct QueryOptions<'a> {
    /// The `<data type>` of the `<JSON output clause>`, FORMAT JSON, a
    /// character string type; without one, `query::IMPLICIT_RETURNING`
    /// \[SR1b\].
    pub returning: &'a DataType,
    /// The `<JSON query wrapper behavior>`.
    pub wrapper: Wrapper,
    /// The `<JSON query quotes behavior>`.
    pub quotes: Quotes,
    /// The `<JSON query empty behavior>`.
    pub on_empty: QueryBehavior,
    /// The `<JSON query error behavior>`.
    pub on_error: QueryBehavior,
}

/// [§6.35 GR] JSON_QUERY of the `<JSON API common syntax>` `common` and
/// its clauses `options`: the null value if the context item is \[GR1\];
/// else the SQL/JSON sequence of the common syntax \[GR3\] serialized to
/// `options.returning` (`serialize_sequence`) \[GR5b\], and the warning a
/// cast raised, if any.
///
/// # Errors
///
/// SQLCODE -103 if `options.returning` is no string type [§10.13 SR2];
/// -900 if it is a binary string type (RULES.md D18); else as `process`
/// and `serialize_sequence` \[GR6\].
pub fn json_query(common: &CommonSyntax<'_>, options: &QueryOptions<'_>) -> Result<Warned> {
    string_type(options.returning)?;
    let Some(status) = process(common).transpose() else { return Ok((Value::Null, None)) };
    serialize_sequence(status, options)
}

/// [§9.49 GR4a iv 1] Whether `seq` is a single SQL/JSON item that is an
/// array or an object.
fn single_array_or_object(seq: &[Item]) -> bool {
    matches!(seq, [Item::Array(_) | Item::Object(_)])
}

/// [§9.49] The SQL/JSON sequence of `status`, STATUS IN and SQL/JSON
/// SEQUENCE, serialized as FORMAT JSON to the character string type
/// `options.returning`, and the warning a cast raised, if any: wrapped as
/// `options.wrapper` says \[GR4a, GR4b\]; its one item, of none as
/// `options.on_empty` says \[GR4c\], cast if it is a character string and
/// `options.quotes` is OMIT \[GR5b i\], else serialized [§9.43] \[GR5c\];
/// the null value if empty and `options.on_empty` is NULL \[GR5a\]. An
/// exception condition, of `status`, the rules, a cast or the
/// serialization's STATUS, is replaced as `options.on_error` says
/// \[GR6\]; an error not one is raised (`query::is_exception`), and so is
/// the serialization's 22001 [§9.43 GR2a iii].
///
/// # Errors
///
/// SQLCODE -103 if `options.returning` is no string type [§10.13 SR2];
/// -900 if it is a binary string type (RULES.md D18); an error not an
/// exception condition; if `options.on_error` is ERROR \[GR6a i\], the
/// exception condition: of `status`; SQLSTATE 22035 if the sequence is
/// empty and `options.on_empty` is ERROR \[GR4c i 1\], 22034 if it has
/// more than one item and is not wrapped \[GR4c iii\], 22032 if its item
/// has no JSON text [§9.43 GR2a ii], or that of a cast \[GR5b i\]; if
/// `options.on_error` is EMPTY ARRAY or EMPTY OBJECT, that of its
/// serialization \[GR6a iii 2\]. Whatever `options.on_error` is, SQLSTATE
/// 22001 if the JSON text is longer than the type, which §9.43 GR2a iii
/// raises rather than returns as a STATUS.
pub fn serialize_sequence(status: Result<Vec<Item>>, options: &QueryOptions<'_>) -> Result<Warned> {
    // [§6.35 GR5a] Of RETURNING JSON, the sequence converted to an item
    // [§9.44], a value of the JSON type, no text.
    if let DataType::Json = options.returning {
        let item = super::sequence_to_item(status, options.wrapper, options.on_empty, options.on_error)?;
        let value = item.map(|item| crate::value::json::Json::of_item(&item)).transpose()?;
        return Ok(unwarned(value.map_or(Value::Null, Value::Json)));
    }
    string_type(options.returning)?;
    let temp = match status.and_then(|seq| {
        let wrapit = options.wrapper.wraps(&seq, single_array_or_object);
        only(wrapped(seq, wrapit), options.on_empty, "§9.49 GR4")
    }) {
        Ok(None) => Ok((Value::Null, None)),
        Ok(Some(item)) => serialized(&item, options)?,
        Err(error) => Err(error),
    };
    caught(temp, |error| match options.on_error {
        QueryBehavior::Error => Err(error),
        QueryBehavior::Null => Ok((Value::Null, None)),
        QueryBehavior::EmptyArray => serialize_as(&Item::Array(Vec::new()), options.returning).map(unwarned),
        QueryBehavior::EmptyObject => serialize_as(&Item::Object(Vec::new()), options.returning).map(unwarned),
    })
}

/// [§9.49 GR5b, GR5c] JT: the item `item`, V, as a value of
/// `options.returning`, as TEMPST, the inner result: a cast's exception
/// condition is one \[GR5b i\], and so is a serialization's STATUS
/// [§9.43 GR2a ii]. A text longer than the type is not: §9.43 GR2a iii
/// raises it, where GR2a ii lets ST be one, so it is no STATUS ON ERROR
/// replaces, and is the outer error.
///
/// # Errors
///
/// SQLSTATE 22001 if the JSON text is longer than `options.returning`
/// [§9.43 GR2a iii].
fn serialized(item: &Item, options: &QueryOptions<'_>) -> Result<Result<Warned>> {
    if options.quotes == Quotes::Omit
        && let Item::Scalar(scalar) = item
        && let Value::Char(_) = scalar.value()
    {
        return Ok(cast_warned(scalar.value().clone(), options.returning));
    }
    match serialize_as(item, options.returning) {
        Ok(value) => Ok(Ok(unwarned(value))),
        Err(error) => {
            if error.sqlstate() == SqlState::STRING_DATA_RIGHT_TRUNCATION {
                Err(error)
            } else {
                Ok(Err(error))
            }
        }
    }
}

/// A value no cast made, so with no warning.
fn unwarned(value: Value) -> Warned {
    (value, None)
}

#[cfg(test)]
mod tests;

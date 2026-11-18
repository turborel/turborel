//! The SQL/JSON constructor functions [ISO/IEC 9075-2:2023 §4.48.4]:
//! JSON_OBJECT and JSON_ARRAY [§6.34], `<JSON value constructor>`, and
//! JSON_OBJECTAGG and JSON_ARRAYAGG [§10.11], `<JSON aggregate
//! function>`, by their General Rules, of inputs already evaluated.
//!
//! This file holds what they share: the `<JSON constructor null clause>`,
//! a `<JSON input expression>`'s value made the SQL/JSON item a member is
//! bound to or an element is [§6.34 GR2d ii 1 F, GR3c ii 1 B, GR4b ii 1
//! B, §10.11 GR4b ii 2 D, GR5f ii 2], a `<JSON name>`'s value made a key
//! [§6.34 GR2d ii 1 C, D], the limits on members and elements, and the
//! JSON text of the item made, serialized to the type returned [§6.34
//! GR5a, §10.11 GR6b]. JSON_OBJECT is in `construct/object.rs`,
//! JSON_ARRAY in `construct/array.rs`, the aggregates in
//! `construct/aggregate.rs`.
//!
//! \[DESIGN\] turborel has no JSON type (Feature T801): no input is of one
//! \[GR2d ii 1 F II\], and the type returned is a character string type,
//! FORMAT JSON, so \[GR5a\] applies and the result is JSON text. A
//! `<JSON input expression>` FORMAT JSON is a string, its JSON text
//! (Feature T803). Every exception condition is raised: none of these
//! has an ON ERROR clause.

mod aggregate;
mod array;
mod object;

pub use aggregate::JsonObjectAgg;
pub use array::{ArrayOptions, json_array, json_array_by_query};
pub use object::{NameAndValue, ObjectOptions, json_object};

use super::query::json_input;
use super::{IMPLICIT_RETURNING, Item, Keys, Scalar};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::functions::lobs::like::held_text;
use crate::value::lob::LobKind;
use crate::value::{DataType, Value, cast};

/// [§6.34] A `<JSON constructor null clause>`: what a member bound to the
/// SQL/JSON null, or an element that is it, becomes. Its default differs:
/// \[§6.34 SR6a, §10.11 SR4a\] NULL ON NULL, `OnNull::Null`, for
/// JSON_OBJECT and JSON_OBJECTAGG; \[§6.34 SR7a, §10.11 SR5a\] ABSENT ON
/// NULL, `OnNull::Absent`, for JSON_ARRAY and JSON_ARRAYAGG.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnNull {
    /// `NULL ON NULL`: kept; the default of the object constructors.
    Null,
    /// `ABSENT ON NULL`: left out; the default of the array constructors.
    Absent,
}

/// [§10.12] A `<JSON input expression>` of a constructor, its value
/// evaluated.
#[derive(Debug, Clone, Copy)]
pub struct Input<'a> {
    /// The value of the `<value expression>`.
    pub value: &'a Value,
    /// Whether the `<JSON input clause>` FORMAT JSON is written: the value
    /// is then a character string's JSON text \[§10.12 SR6\], parsed.
    pub format_json: bool,
}

/// \[DESIGN\] The implementation-defined maximum number of members of a
/// JSON object constructed (IL051) [§6.34 GR2d ii 3, §10.11 GR4b ii 4],
/// 2^20, so that what an aggregate holds is bounded; one more is SQLSTATE
/// 2203E.
pub const MAX_MEMBERS: usize = 1 << 20;

/// \[DESIGN\] The implementation-defined maximum number of elements of a
/// JSON array constructed (IL052) [§6.34 GR3c ii 2, GR4b ii 2, §10.11
/// GR5g], as `MAX_MEMBERS`; one more is SQLSTATE 2203D.
pub const MAX_ELEMENTS: usize = 1 << 20;

/// \[DESIGN\] The implementation-defined character string type SDT, of
/// Unicode, a value of none of the types an input keeps is cast to
/// (ID100, IV179) [§6.34 GR2d ii 1 F IV 2, §10.11 GR4b ii 2 D IV 2]: as
/// `IMPLICIT_RETURNING`.
const SDT: DataType = IMPLICIT_RETURNING;

/// The Syntax Rule a `<JSON name>`'s value breaks if it is no character
/// string.
const NAME_RULE: &str = "[ISO/IEC 9075-2:2023 §6.34 F] a JSON name is a character string";
/// The Syntax Rule an input FORMAT JSON breaks if it is no string.
const INPUT_RULE: &str = "[ISO/IEC 9075-2:2023 §10.12 SR6] an input FORMAT JSON is a string";
/// What a character large object read whole as an input is, for the
/// error of one too long.
const INPUT_TEXT: &str = "an input of an SQL/JSON constructor function";
/// What a character large object read whole as a name is.
const NAME_TEXT: &str = "a JSON name";

/// [§6.34 GR2d ii 1 F, GR3c ii 1 B, GR4b ii 1 B, §10.11 GR4b ii 2 D, GR5f
/// ii 2] The SQL/JSON item `input` is: \[I\] the SQL/JSON null of the null
/// value; \[III\] FORMAT JSON, the item its JSON text is, parsed under
/// `keys` [§9.42]; \[IV 1\] else a character string, a number or a truth
/// value the scalar it is, a character large object's read whole; \[IV 2\]
/// a value of another type, a datetime too, cast to `SDT`.
///
/// \[DESIGN\] This is not `Item::of_value`'s rule [§9.46 GR11b i 2], which
/// keeps a datetime a scalar: here it is cast to a string, which a JSON
/// text can hold.
///
/// # Errors
///
/// SQLCODE -103 if `input` is FORMAT JSON and no string \[§10.12 SR6\];
/// -900 if it is a binary string FORMAT JSON, a character large object
/// longer than one chunk, or a text beyond turborel's limits; SQLSTATE
/// 22032 if its text is no JSON text, 22037 if two members of an object of
/// it have one key under `Keys::Unique` [§9.42 GR3a iii]; that of the cast
/// \[IV 2\].
fn input_item(input: &Input<'_>, keys: Keys) -> Result<Item> {
    // [§6.34 GR2d ii 1 F II] a value of the JSON type, its item.
    if input.format_json || matches!(input.value, Value::Json(_)) {
        return Ok(json_input(input.value, INPUT_RULE, keys)?.unwrap_or(Item::Null));
    }
    let kept = match input.value {
        Value::Null => return Ok(Item::Null),
        Value::Char(_) | Value::Exact(_) | Value::Approx(_) | Value::DecFloat(_) | Value::Bool(_) => input.value.clone(),
        Value::Lob(lob) => match lob.kind() {
            LobKind::Character => Value::Char(held_text(lob, INPUT_TEXT)?),
            LobKind::Binary => cast(input.value.clone(), &SDT)?,
        },
        Value::Datetime(_)
        | Value::Binary(_)
        | Value::Interval(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Row(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Json(_)
        | Value::Extension(_) => cast(input.value.clone(), &SDT)?,
    };
    Ok(Scalar::new(kept).map_or(Item::Null, Item::Scalar))
}

/// [§6.34 GR2d ii 1 C, D, §10.11 GR4b ii 2 A, B] The key the value `name`
/// of a `<JSON name>` is: a character string's characters, a character
/// large object's read whole. \[GR2a\] The implementation-defined maximum
/// length of a name (IL049) is \[DESIGN\] that of its type, which no
/// value of it exceeds.
///
/// # Errors
///
/// SQLSTATE 22004 if `name` is the null value \[GR2d ii 1 D\]; SQLCODE
/// -103 if it is no character string \[F\]; -900 if it is a character
/// large object longer than one chunk.
fn key(name: &Value) -> Result<String> {
    match name {
        Value::Null => Err(SqlError::new(
            Code::Data,
            "[ISO/IEC 9075-2:2023 §6.34 GR2d ii 1 D] the name of a member of a JSON object is the null value",
        )
        .with_state(SqlState::NULL_VALUE_NOT_ALLOWED)),
        Value::Char(s) => Ok(s.clone()),
        Value::Lob(lob) => match lob.kind() {
            LobKind::Character => held_text(lob, NAME_TEXT),
            LobKind::Binary => Err(not_a_name(name)),
        },
        Value::Exact(_)
        | Value::Approx(_)
        | Value::DecFloat(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Binary(_)
        | Value::Interval(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Row(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Json(_)
        | Value::Extension(_) => Err(not_a_name(name)),
    }
}

/// The error of the value `name` of a `<JSON name>` that is no character
/// string.
fn not_a_name(name: &Value) -> SqlError {
    SqlError::new(Code::SyntaxRule, format!("{NAME_RULE}, not {name}"))
}

/// [§6.34 GR2d ii 3, GR3c ii 2, GR4b ii 2, §10.11 GR4b ii 4, GR5g] That
/// `count`, of members or elements, is at most `most`: else the exception
/// `state` of the rule `cited`.
///
/// # Errors
///
/// SQLSTATE `state` if `count` is greater than `most`.
fn within(count: usize, most: usize, state: SqlState, cited: &str) -> Result<()> {
    if count > most {
        return Err(SqlError::new(Code::Data, format!("{cited}: {count}, more than {most}")).with_state(state));
    }
    Ok(())
}

#[cfg(test)]
mod tests;

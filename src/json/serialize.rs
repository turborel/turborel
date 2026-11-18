//! An SQL/JSON item serialized as JSON text [ISO/IEC 9075-2:2023 §9.43],
//! FORMAT JSON, to a character string type.
//!
//! [§9.43 GR2a ii] The text is implementation-dependent (UV097), so long
//! as it is a JSON text that parses [§9.42], WITHOUT UNIQUE KEYS, to an
//! item equivalent to the one serialized. \[DESIGN\] turborel writes:
//!
//! - no insignificant white space [RFC 8259 §2];
//! - an object's members in the order the item keeps them, those of one
//!   key all, so a text parsed and serialized keeps its members' order;
//! - a string's quotation marks and reverse solidi as `\"` and `\\`, and
//!   the control characters U+0000 to U+001F, which RFC 8259 §7 makes
//!   escaped, as the two-character escape it has of one (`\b`, `\f`, `\n`,
//!   `\r`, `\t`), else as a `\u` escape of four lower case hexadecimal
//!   digits; no other character is escaped, a solidus neither;
//! - an exact or approximate number as CAST writes it as a character
//!   string [§6.13 GR11, GR12]: an exact number's digits and its scale's,
//!   an approximate number's shortest `<approximate numeric literal>`,
//!   both JSON numbers [RFC 8259 §6] that parse to equal numbers
//!   [§9.42 GR3a iii 3 A IV]; a decimal floating-point number plain,
//!   without an exponent, where CAST may write one (IV110), since a JSON
//!   number with an exponent parses to an approximate number, which may
//!   not equal it;
//! - true, false and the SQL/JSON null as their literal names.
//!
//! \[GR2a ii\] No JSON text parses to a datetime, nor to an approximate
//! number that is not finite, so an item that is or holds one has no JSON
//! text: SQLSTATE 22032.

use std::fmt::Write as _;

use super::{Item, MAX_NESTING, Member, beyond};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::value::cast::approximate_literal;
use crate::value::lob::LobKind;
use crate::value::{CharUnits, DataType, Value, cast};

// [RFC 8259 §3, §7] The literal names, and the escapes of a string.
/// `false`.
const FALSE: &str = "false";
/// `null`.
const NULL: &str = "null";
/// `true`.
const TRUE: &str = "true";
/// The last of "the control characters (U+0000 through U+001F)", which
/// are escaped.
const LAST_CONTROL: char = '\u{1F}';

/// [§9.43 GR2a ii] The JSON text the SQL/JSON item `item` is, as the
/// module's documentation says it is written.
///
/// # Errors
///
/// SQLSTATE 22032 if `item` is or holds a datetime or an approximate
/// number that is not finite, of which no JSON text is \[GR2a ii\];
/// SQLCODE -900 if arrays and objects nest deeper than `MAX_NESTING`.
pub fn serialize(item: &Item) -> Result<String> {
    let mut text = String::new();
    write_item(item, &mut text, 0)?;
    Ok(text)
}

/// [§9.43] `item` serialized \[GR2a ii\] as a value of the character
/// string type `target`, its TARGET TYPE: of CHARACTER, padded with
/// spaces to its length, which are insignificant white space
/// [RFC 8259 §2].
///
/// # Errors
///
/// SQLCODE -103 if `target` is no string type \[SR2a\]; -900 if it is a
/// binary string type, whose encodings turborel does not implement
/// (RULES.md D18); SQLSTATE 22001 if the text is longer than `target`'s
/// length or maximum length \[GR2a iii\]; else as `serialize`.
pub fn serialize_as(item: &Item, target: &DataType) -> Result<Value> {
    // [§6.34 GR5b, §6.35 GR5a, §10.11 GR] Of RETURNING JSON, the item
    // itself, a value of the JSON type, no text.
    if let DataType::Json = target {
        return crate::value::json::Json::of_item(item).map(Value::Json);
    }
    let (length, units) = character_length(target)?;
    let text = serialize(item)?;
    let long = u64::try_from(units.count(&text)).unwrap_or(u64::MAX);
    if long > length {
        return Err(SqlError::new(
            Code::Data,
            format!("[ISO/IEC 9075-2:2023 §9.43 GR2a iii] the JSON text of {long} characters or octets is longer than {target}"),
        )
        .with_state(SqlState::STRING_DATA_RIGHT_TRUNCATION));
    }
    cast(Value::Char(text), target)
}

/// [§9.43 SR2a] That `target` is a character string type, which JSON text
/// is serialized as.
///
/// # Errors
///
/// As `serialize_as`, of a type that is not one.
pub(crate) fn string_type(target: &DataType) -> Result<()> {
    if let DataType::Json = target {
        return Ok(());
    }
    character_length(target).map(|_| ())
}

/// [§9.43 SR2a] The length or maximum length of the character string type
/// `target`, and what it counts.
///
/// # Errors
///
/// As `serialize_as`, of a type that is not one.
fn character_length(target: &DataType) -> Result<(u64, CharUnits)> {
    match target {
        DataType::Character { length, units } | DataType::Varying { length, units } => Ok((u64::from(*length), *units)),
        DataType::Lob { kind: LobKind::Character, length, units } => Ok((*length, *units)),
        DataType::Lob { kind: LobKind::Binary, .. } | DataType::Binary { .. } | DataType::VarBinary { .. } => Err(SqlError::new(
            Code::Unsupported,
            format!("[DESIGN] JSON text serialized as the binary string type {target} is valid SQL but is not implemented (RULES.md D18)"),
        )),
        DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::DecFloat { .. }
        | DataType::Boolean
        | DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Interval(_)
        | DataType::Array(_)
        | DataType::Multiset(_)
        | DataType::Row(_)
        | DataType::Distinct(_)
        | DataType::Structured(_)
        | DataType::Reference(_)
        | DataType::Json
        | DataType::Xml(_)
        | DataType::Extension(_) => Err(SqlError::new(
            Code::SyntaxRule,
            format!("[ISO/IEC 9075-2:2023 §9.43 SR2a] JSON text is serialized as a character or binary string type, not {target}"),
        )),
    }
}

/// The error of `what`, of which no JSON text is \[GR2a ii\].
fn no_text(what: &str) -> SqlError {
    SqlError::new(Code::Data, format!("[ISO/IEC 9075-2:2023 §9.43 GR2a ii] no JSON text parses to {what}"))
        .with_state(SqlState::INVALID_JSON_TEXT)
}

/// `item`, within `depth` arrays and objects, written to `out`.
fn write_item(item: &Item, out: &mut String, depth: usize) -> Result<()> {
    if depth > MAX_NESTING {
        return Err(beyond("arrays and objects nest too deep"));
    }
    match item {
        Item::Null => out.push_str(NULL),
        Item::Scalar(scalar) => write_scalar(scalar.value(), out)?,
        Item::Array(elements) => write_array(elements, out, depth.saturating_add(1))?,
        Item::Object(members) => write_object(members, out, depth.saturating_add(1))?,
    }
    Ok(())
}

/// [RFC 8259 §5] `array`, of `elements`.
fn write_array(elements: &[Item], out: &mut String, depth: usize) -> Result<()> {
    out.push('[');
    for (i, element) in elements.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        write_item(element, out, depth)?;
    }
    out.push(']');
    Ok(())
}

/// [RFC 8259 §4] `object`, of `members`.
fn write_object(members: &[Member], out: &mut String, depth: usize) -> Result<()> {
    out.push('{');
    for (i, member) in members.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        write_string(&member.key, out);
        out.push(':');
        write_item(&member.value, out, depth)?;
    }
    out.push('}');
    Ok(())
}

/// The JSON value of the SQL/JSON scalar `value`.
fn write_scalar(value: &Value, out: &mut String) -> Result<()> {
    match value {
        Value::Char(s) => write_string(s, out),
        Value::Bool(truth) => out.push_str(if *truth { TRUE } else { FALSE }),
        Value::Exact(d) => out.push_str(&d.to_string()),
        Value::Approx(x) => out.push_str(&approximate(*x)?),
        Value::DecFloat(d) => out.push_str(&d.numeral()),
        Value::Datetime(_) => return Err(no_text("a datetime")),
        // [§4.48.3] No SQL/JSON scalar is of another type (`Scalar::new`).
        Value::Null
        | Value::Binary(_)
        | Value::Interval(_)
        | Value::Lob(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Row(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Json(_)
        | Value::Xml(_)
        | Value::Extension(_) => return Err(no_text("a value of no SQL/JSON scalar's type")),
    }
    Ok(())
}

/// CAST's `<approximate numeric literal>` of `x` [§6.13 GR12b].
///
/// # Errors
///
/// SQLSTATE 22032 if `x` is not finite \[GR2a ii\].
fn approximate(x: f64) -> Result<String> {
    if x.is_finite() { Ok(approximate_literal(x)) } else { Err(no_text("an approximate number that is not finite")) }
}

/// [RFC 8259 §7] `string`, of the characters of `s`.
fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{C}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0'..=LAST_CONTROL => {
                // Writing to a String cannot fail.
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            _ => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests;

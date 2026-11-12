//! [ISO/IEC 9075-2:2023 §4.6] BOOLEAN: the truth values True and False,
//! and Unknown as its null value [§4.6.1]. Its values are `Value::Bool`,
//! True greater than False [§4.6.2].
//!
//! A value is one byte in a record and in a key, 0 for False and 1 for
//! True, so keys order as the values do.

use std::cmp::Ordering;

use super::{Descriptor, SqlType, take};
use crate::bytes::{Put, Reader};
use crate::error::{Code, Result, SqlError, SqlState, fail_with};
use crate::lexer::kw;
use crate::value::cast::uncastable;
use crate::value::{Bucket, DataType, Value, unassignable};

/// The kind of BOOLEAN.
#[derive(Debug)]
pub struct Boolean;

/// The one value of it.
pub static BOOLEAN: Boolean = Boolean;

/// [ISO/IEC 9075-2:2023 §6.1] `<boolean type>` ::= BOOLEAN, its name. Not
/// a key word here, so a column of SQL-92 named BOOLEAN keeps its name.
pub const BOOLEAN_NAME: &str = "BOOLEAN";

/// Bytes of a value in a record or a key.
const WIDTH: usize = 1;
/// The byte of False, and of True.
const FALSE_BYTE: u8 = 0;
const TRUE_BYTE: u8 = 1;

/// The truth value of `v`, if it is one.
fn truth(v: &Value) -> Option<bool> {
    if let Value::Bool(b) = v { Some(*b) } else { None }
}

/// A value not of a column of `t`'s type.
fn mismatch(v: &Value, t: &DataType) -> SqlError {
    SqlError::new(Code::Api, format!("[DESIGN] {v} is not a value of a column of type {t}"))
}

/// The byte of `b`.
fn byte(b: bool) -> u8 {
    if b { TRUE_BYTE } else { FALSE_BYTE }
}

/// The truth value of `byte`, if it is one.
fn of_byte(byte: u8) -> Option<bool> {
    match byte {
        FALSE_BYTE => Some(false),
        TRUE_BYTE => Some(true),
        _ => None,
    }
}

/// [ISO/IEC 9075-2:2023 §6.13 GR21a] The value of `s`, its spaces trimmed,
/// if it is a `<boolean literal>` [§5.3], whose key words are in any case:
/// UNKNOWN being the null value [§4.6.1].
///
/// # Errors
///
/// SQLSTATE 22018 if it is none.
pub(crate) fn boolean_literal(s: &str) -> Result<Value> {
    let word = s.trim_matches(' ').to_ascii_uppercase();
    match word.as_str() {
        kw::TRUE => Ok(Value::Bool(true)),
        kw::FALSE => Ok(Value::Bool(false)),
        kw::UNKNOWN => Ok(Value::Null),
        _ => fail_with(
            Code::Data,
            SqlState::INVALID_CHARACTER_VALUE_FOR_CAST,
            format!("[ISO/IEC 9075-2:2023 §6.13 GR21a ii] '{s}' is not a <boolean literal>"),
        ),
    }
}

/// [ISO/IEC 9075-2:2023 §6.13 GR11f, GR12f] `b` cast to `t`, a character
/// string type: 'TRUE' or 'FALSE', extended on the right with spaces if
/// `t` is of fixed length.
///
/// # Errors
///
/// SQLSTATE 22018 if the word is longer than `t`'s length.
pub(crate) fn as_text(b: bool, t: &DataType) -> Result<Value> {
    let word = if b { kw::TRUE } else { kw::FALSE };
    let (length, fixed) = match *t {
        DataType::Character { length } => (length, true),
        DataType::Varying { length } => (length, false),
        DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::DecFloat { .. }
        | DataType::Binary { .. }
        | DataType::VarBinary { .. }
        | DataType::Boolean
        | DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Interval(_)
        | DataType::Extension(_) => return uncastable(&Value::Bool(b), t),
    };
    let rule = if fixed { "GR11f iii" } else { "GR12f iii" };
    let Some(spaces) = usize::try_from(length).ok().and_then(|l| l.checked_sub(word.len())) else {
        let msg = format!("[ISO/IEC 9075-2:2023 §6.13 {rule}] {word} is longer than {length} characters");
        return fail_with(Code::Data, SqlState::INVALID_CHARACTER_VALUE_FOR_CAST, msg);
    };
    let pad = if fixed { " ".repeat(spaces) } else { String::new() };
    Ok(Value::Char(format!("{word}{pad}")))
}

impl SqlType for Boolean {
    fn name(&self) -> &'static str {
        BOOLEAN_NAME
    }

    fn written(&self, _: &DataType) -> String {
        BOOLEAN_NAME.to_owned()
    }

    /// [ISO/IEC 9075-2:2023 §4.6.2] Booleans are comparable with Booleans
    /// alone.
    fn comparable(&self, other: &DataType) -> bool {
        matches!(other, DataType::Boolean)
    }

    /// [ISO/IEC 9075-2:2023 §4.6.2] True is greater than False.
    fn compare(&self, a: &Value, b: &Value) -> Option<Ordering> {
        Some(truth(a)?.cmp(&truth(b)?))
    }

    fn bucket(&self, v: &Value) -> Option<Bucket> {
        truth(v).map(Bucket::Bool)
    }

    /// [ISO/IEC 9075-2:2023 §4.6.2] A truth value, to any site of the type.
    fn assign(&self, v: Value, t: &DataType, rule: &str) -> Result<Value> {
        match v {
            Value::Bool(_) => Ok(v),
            Value::Null
            | Value::Char(_)
            | Value::Exact(_)
            | Value::Approx(_)
            | Value::DecFloat(_)
            | Value::Binary(_)
            | Value::Datetime(_)
            | Value::Interval(_)
            | Value::Extension(_) => unassignable(&v, t, rule),
        }
    }

    /// [ISO/IEC 9075-2:2023 §6.13 GR21] A character string's `<boolean
    /// literal>`, its spaces trimmed \[GR21a\]; a truth value itself
    /// \[GR21b\].
    fn cast(&self, v: Value, t: &DataType) -> Result<Value> {
        match v {
            Value::Null | Value::Bool(_) => Ok(v),
            Value::Char(s) => boolean_literal(&s),
            Value::Exact(_)
            | Value::Approx(_)
            | Value::DecFloat(_)
            | Value::Binary(_)
            | Value::Datetime(_)
            | Value::Interval(_)
            | Value::Extension(_) => uncastable(&v, t),
        }
    }

    /// [ISO/IEC 9075-2:2023 §5.3] `<boolean literal>` ::= TRUE | FALSE.
    fn literal(&self, v: &Value) -> String {
        v.to_string()
    }

    fn text(&self, v: &Value) -> String {
        v.to_string()
    }

    fn position(&self, v: &Value) -> Option<f64> {
        truth(v).map(|b| f64::from(byte(b)))
    }

    /// [ISO/IEC 9075-2:2023 §4.6.1] Its name alone.
    fn descriptor(&self, _: &DataType) -> Descriptor {
        Descriptor::of(BOOLEAN_NAME)
    }

    fn put_value(&self, v: &Value, out: &mut Vec<u8>) {
        out.put_u8(truth(v).map_or(FALSE_BYTE, byte));
    }

    fn value(&self, r: &mut Reader<'_>) -> Result<Value> {
        let b = r.u8()?;
        of_byte(b).map(Value::Bool).ok_or_else(|| SqlError::new(Code::Corrupt, format!("[DESIGN] {b} is not a truth value")))
    }

    fn put_params(&self, _: &DataType, _: &mut Vec<u8>) {}

    fn params(&self, _: &mut Reader<'_>) -> Result<DataType> {
        Ok(DataType::Boolean)
    }

    fn record_width(&self, _: &DataType) -> Option<usize> {
        Some(WIDTH)
    }

    fn encode_record(&self, v: &Value, t: &DataType) -> Result<Vec<u8>> {
        let Some(b) = truth(v) else { return Err(mismatch(v, t)) };
        Ok(vec![byte(b)])
    }

    fn decode_record(&self, field: &[u8], _: &DataType) -> Option<Value> {
        let [b] = field else { return None };
        of_byte(*b).map(Value::Bool)
    }

    fn encode_key(&self, v: &Value, t: &DataType, out: &mut Vec<u8>) -> Result<()> {
        let Some(b) = truth(v) else { return Err(mismatch(v, t)) };
        out.push(byte(b));
        Ok(())
    }

    fn decode_key(&self, rest: &mut &[u8], _: &DataType) -> Result<Value> {
        let [b] = take(rest, WIDTH)? else { return Err(super::malformed()) };
        of_byte(*b).map(Value::Bool).ok_or_else(super::malformed)
    }

    /// Itself: every truth value is one of the column's.
    fn anchor(&self, v: &Value, _: &DataType) -> Option<(Value, bool)> {
        truth(v).map(|_| (v.clone(), true))
    }
}

// Unit tests: literals, casts to text, bytes and keys (RULES.md R-TEST-2).
#[cfg(test)]
mod tests;

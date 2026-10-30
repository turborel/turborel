//! [X3.135-1986 §5.5] CHARACTER(length): a character string of fixed
//! length, padded with spaces [X3.135-1986 §8.7 GR7b].
//!
//! Its record bytes are the UTF-8 of the value without its trailing
//! spaces, of variable length; its key the UTF-8 padded to the length, so
//! keys of the same column are of the same number of characters and none
//! begins another (`engine::key`).

use std::cmp::Ordering;

use super::{Descriptor, SqlType};
use crate::bytes::{Put, Reader};
use crate::error::{Code, Result, SqlError, SqlState, fail_with};
use crate::value::cast::{approximate_literal, padded};
use crate::value::{BYTE_BITS, Bits, Bucket, DataType, Value, assignment_rule, compare_strings, unassignable};

/// The kind of CHARACTER(length).
#[derive(Debug)]
pub struct Character;

/// The one value of it.
pub static CHARACTER: Character = Character;

/// [§21.3.5 Description 3] Octets a character takes at most: four, in
/// UTF-8.
pub(super) const OCTETS_PER_CHARACTER: usize = 4;

/// The length of `t`, a character string type.
fn length_of(t: &DataType) -> u32 {
    if let DataType::Character { length } = t { *length } else { 0 }
}

/// The value's string, if it is one.
pub(super) fn string(v: &Value) -> Option<&str> {
    if let Value::Char(s) = v { Some(s) } else { None }
}

/// [ISO/IEC 9075:1992 §6.10 GR5d, GR6d] The characters `b`'s bits are,
/// of the form-of-use UTF-8, whose least character is of 8 bits: its
/// bytes, its last extended with 0 bits.
///
/// # Errors
///
/// SQLSTATE 22018 if they are not UTF-8: a string holds characters, so
/// is of a valid encoding.
pub(super) fn of_bits(b: &Bits) -> Result<String> {
    String::from_utf8(b.bytes().to_vec()).or_else(|_| {
        fail_with(
            Code::Data,
            SqlState::INVALID_CHARACTER_VALUE_FOR_CAST,
            format!("[ISO/IEC 9075:1992 §6.10 GR5d, GR6d] the bit string B'{b}' is not characters of UTF-8"),
        )
    })
}

/// A value cast with no warning.
pub(super) fn unwarned(v: Value) -> (Value, Option<SqlState>) {
    (v, None)
}

/// [ISO/IEC 9075:1992 §6.10 GR5c ii, GR6c ii] The first `length`
/// characters of `s`, and the warning string data, right truncation if
/// any after them is not a space.
pub(super) fn cut(s: &str, length: u32) -> (String, Option<SqlState>) {
    let fits = usize::try_from(length).unwrap_or(usize::MAX);
    let lost = s.chars().skip(fits).any(|c| c != ' ');
    (s.chars().take(fits).collect(), lost.then_some(SqlState::STRING_TRUNCATED))
}

/// [ISO/IEC 9075:1992 §6.10 GR5d, GR6d] The bit string `b` cast to a
/// character string of length `length`, `fixed`, or of that maximum
/// length: its characters (`of_bits`), cut to the length, or, `fixed`,
/// extended to it with characters of 0 bits. The warning raised is the
/// first of: implicit zero-bit padding, its last character extended with
/// 0 bits, its bits not a multiple of 8; string data, right truncation,
/// characters cut \[GR5d i, GR6d ii\]; implicit zero-bit padding,
/// characters added \[GR5d ii\].
///
/// # Errors
///
/// As `of_bits`.
pub(super) fn bits_as_characters(b: &Bits, length: u32, fixed: bool) -> Result<(Value, Option<SqlState>)> {
    let text = of_bits(b)?;
    let (count, fits) = (text.chars().count(), usize::try_from(length).unwrap_or(usize::MAX));
    let added = if fixed { fits.saturating_sub(count) } else { 0 };
    let warning = if !b.len().is_multiple_of(BYTE_BITS) || added > 0 {
        Some(SqlState::ZERO_BIT_PADDING)
    } else {
        (count > fits).then_some(SqlState::STRING_TRUNCATED)
    };
    let cast = text.chars().chain(std::iter::repeat_n('\0', added)).take(fits).collect();
    Ok((Value::Char(cast), warning))
}

/// A value not of a column of `t`'s type.
fn mismatch(v: &Value, t: &DataType) -> SqlError {
    SqlError::new(Code::Api, format!("[DESIGN] {v} is not a value of a column of type {t}"))
}

impl SqlType for Character {
    fn name(&self) -> &'static str {
        "CHARACTER"
    }

    fn written(&self, t: &DataType) -> String {
        format!("CHARACTER({})", length_of(t))
    }

    /// [X3.135-1986 §4.2] "A character string and a number are not
    /// comparable values."
    fn comparable(&self, other: &DataType) -> bool {
        other.is_character()
    }

    /// [X3.135-1986 §5.11 GR5] after padding the shorter with spaces.
    fn compare(&self, a: &Value, b: &Value) -> Option<Ordering> {
        Some(compare_strings(string(a)?, string(b)?))
    }

    /// A string without its trailing spaces.
    fn bucket(&self, v: &Value) -> Option<Bucket> {
        Some(Bucket::Char(string(v)?.trim_end_matches(' ').to_owned()))
    }

    /// [ISO/IEC 9075:1992 §9.2 GR3b, GR3c] padded with spaces to the
    /// length, or without trailing spaces beyond it.
    fn assign(&self, v: Value, t: &DataType, rule: &str) -> Result<Value> {
        let Value::Char(s) = v else { return unassignable(&v, t, rule) };
        let length = length_of(t);
        let (count, fits) = (s.chars().count(), usize::try_from(length).unwrap_or(usize::MAX));
        if count <= fits {
            return Ok(Value::Char(format!("{s}{}", " ".repeat(fits.saturating_sub(count)))));
        }
        if s.chars().skip(fits).all(|c| c == ' ') {
            return Ok(Value::Char(s.chars().take(fits).collect()));
        }
        fail_with(
            Code::Data,
            SqlState::STRING_DATA_RIGHT_TRUNCATION,
            format!(
                "[{}] a value of {count} characters, not spaces beyond {length}, does not fit {t}",
                assignment_rule(rule, "§9.2 GR3b ii")
            ),
        )
    }

    /// [ISO/IEC 9075:1992 §6.10 GR5] As `cast_warned`, its warning not
    /// raised.
    fn cast(&self, v: Value, t: &DataType) -> Result<Value> {
        self.cast_warned(v, t).map(|(cast, _)| cast)
    }

    /// [ISO/IEC 9075:1992 §6.10 GR5] A number's shortest literal, padded
    /// \[GR5a, GR5b\]; a string cut or padded \[GR5c\], with the warning
    /// string data, right truncation if a character cut is not a space
    /// \[GR5c ii\]; a bit string's characters, cut or padded, with their
    /// warnings \[GR5d\] (`bits_as_characters`).
    fn cast_warned(&self, v: Value, t: &DataType) -> Result<(Value, Option<SqlState>)> {
        let length = length_of(t);
        match v {
            Value::Null => Ok((Value::Null, None)),
            Value::Exact(d) => padded(&d.to_string(), length).map(unwarned),
            Value::Approx(x) => padded(&approximate_literal(x), length).map(unwarned),
            // A value of a kind beyond the standard, as its kind shows it.
            Value::Extension(ref e) => padded(&e.kind.text(&v), length).map(unwarned),
            Value::Char(s) => {
                let (kept, warning) = cut(&s, length);
                Ok((self.assign(Value::Char(kept), t, "ISO/IEC 9075:1992 §6.10 GR5c")?, warning))
            }
            // [GR5e] its literal's string, as it shows, padded.
            Value::Datetime(d) => super::datetime::as_string(&d, length, true).map(unwarned),
            Value::Interval(i) => super::datetime::fitted(i.to_string(), length, true).map(unwarned),
            Value::Bits(b) => bits_as_characters(&b, length, true),
        }
    }

    fn literal(&self, v: &Value) -> String {
        string(v).map_or_else(|| v.to_string(), |s| format!("'{}'", s.replace('\'', "''")))
    }

    fn text(&self, v: &Value) -> String {
        v.to_string()
    }

    fn position(&self, _: &Value) -> Option<f64> {
        None
    }

    /// Its collation SQL_TEXT's; four octets to a character, the most
    /// UTF-8 takes.
    fn descriptor(&self, t: &DataType) -> Descriptor {
        let length = length_of(t);
        let octets = usize::try_from(length).unwrap_or(usize::MAX).saturating_mul(OCTETS_PER_CHARACTER);
        Descriptor { length: Some(length), octets: Some(octets), collated: true, ..Descriptor::of(self.name()) }
    }

    fn put_value(&self, v: &Value, out: &mut Vec<u8>) {
        out.put_str(string(v).unwrap_or_default());
    }

    fn value(&self, r: &mut Reader<'_>) -> Result<Value> {
        Ok(Value::Char(r.str()?))
    }

    fn put_params(&self, t: &DataType, out: &mut Vec<u8>) {
        out.put_u32(length_of(t));
    }

    fn params(&self, r: &mut Reader<'_>) -> Result<DataType> {
        Ok(DataType::Character { length: r.u32()? })
    }

    /// Of variable length.
    fn record_width(&self, _: &DataType) -> Option<usize> {
        None
    }

    /// Without its trailing spaces.
    fn encode_record(&self, v: &Value, t: &DataType) -> Result<Vec<u8>> {
        Ok(string(v).ok_or_else(|| mismatch(v, t))?.trim_end_matches(' ').as_bytes().to_vec())
    }

    /// Padded again to the length.
    fn decode_record(&self, field: &[u8], t: &DataType) -> Option<Value> {
        let s = std::str::from_utf8(field).ok()?;
        let pad = usize::try_from(length_of(t)).ok()?.checked_sub(s.chars().count())?;
        Some(Value::Char(format!("{s}{}", " ".repeat(pad))))
    }

    /// Padded with spaces to the length [X3.135-1986 §5.11 GR5]: Unicode
    /// scalar order [GR6, implementor-defined] is the order of the UTF-8
    /// bytes.
    fn encode_key(&self, v: &Value, t: &DataType, out: &mut Vec<u8>) -> Result<()> {
        let s = string(v).ok_or_else(|| mismatch(v, t))?;
        let pad = usize::try_from(length_of(t)).ok().and_then(|n| n.checked_sub(s.chars().count())).ok_or_else(|| mismatch(v, t))?;
        out.extend_from_slice(s.as_bytes());
        out.resize(out.len().saturating_add(pad), b' ');
        Ok(())
    }

    /// The value ends where the character after its `length`-th begins: at
    /// a byte that does not continue a character.
    fn decode_key(&self, rest: &mut &[u8], t: &DataType) -> Result<Value> {
        let length = usize::try_from(length_of(t)).map_err(|_| super::malformed())?;
        let starts = rest.iter().enumerate().filter(|&(_, b)| b & 0xC0 != 0x80).map(|(i, _)| i);
        let end = starts.clone().nth(length).unwrap_or(rest.len());
        let s = std::str::from_utf8(super::take(rest, end)?).map_err(|_| super::malformed())?;
        if s.chars().count() != length {
            return Err(super::malformed());
        }
        Ok(Value::Char(s.to_owned()))
    }

    /// Cut to the length.
    fn anchor(&self, v: &Value, t: &DataType) -> Option<(Value, bool)> {
        let s = string(v)?;
        let length = usize::try_from(length_of(t)).ok()?;
        Some((Value::Char(s.chars().take(length).collect()), s.chars().count() <= length))
    }
}

//! [ISO/IEC 9075:1992 §6.1] CHARACTER VARYING(length), CHAR VARYING and
//! VARCHAR: a character string of variable length, at most `length`
//! characters, kept as assigned, trailing spaces and all. It compares as
//! CHARACTER does, the shorter string padded with spaces [§8.2 GR3a], so
//! its values are `Value::Char`, as CHARACTER's are.
//!
//! Its record bytes are its UTF-8, as it is. Its key is not padded to the
//! length, which may be long: the UTF-8 without its trailing spaces, in
//! chunks of `CHUNK` bytes, the last padded with spaces, each followed by
//! a marker of what follows it: `END`, or the rest beginning, once its
//! spaces are passed over, below a space (`LESS`) or above one
//! (`GREATER`). So the keys order as the strings compare padded with
//! spaces (a chunk orders by its bytes; equal chunks by what follows
//! them, against more spaces), strings equal so have one key, and no key
//! begins another, ending at its first `END`. A key reads back as the
//! string without its trailing spaces, equal to it: a value is read from
//! its row, never from its key.
//!
//! ```text
//! key: { chunk of CHUNK bytes | LESS (1) | GREATER (3) }... chunk | END (2)
//! ```

use std::cmp::Ordering;

use super::character::{OCTETS_PER_CHARACTER, bits_as_characters, cut, string, unwarned};
use super::{Descriptor, SqlType, malformed, take};
use crate::bytes::{Put, Reader};
use crate::error::{Code, Result, SqlError, SqlState, fail_with};
use crate::value::cast::approximate_literal;
use crate::value::{Bucket, DataType, Value, assignment_rule, compare_strings, unassignable};

/// The kind of CHARACTER VARYING(length).
#[derive(Debug)]
pub struct Varying;

/// The one value of it.
pub static VARYING: Varying = Varying;

/// Bytes of the string in each chunk of a key.
const CHUNK: usize = 8;
/// A key's marker: the rest begins below a space.
const LESS: u8 = 1;
/// A key's marker: the key ends.
const END: u8 = 2;
/// A key's marker: the rest begins above a space.
const GREATER: u8 = 3;
/// The byte of a space, U+0020, in UTF-8: no other character's UTF-8
/// holds it.
const SPACE: u8 = b' ';

/// The maximum length of `t`, a variable-length character string type.
fn length_of(t: &DataType) -> u32 {
    if let DataType::Varying { length } = t { *length } else { 0 }
}

/// A value not of a column of `t`'s type.
fn mismatch(v: &Value, t: &DataType) -> SqlError {
    SqlError::new(Code::Api, format!("[DESIGN] {v} is not a value of a column of type {t}"))
}

/// `text` as a value of `t`, if it is no longer [§6.10 GR6a ii, GR6b iii 2].
///
/// # Errors
///
/// SQLSTATE 22001 if it is [§6.10 GR6a iii, GR6b iii 3].
fn within(text: String, t: &DataType) -> Result<Value> {
    if u32::try_from(text.chars().count()).is_ok_and(|n| n <= length_of(t)) {
        return Ok(Value::Char(text));
    }
    fail_with(
        Code::Data,
        SqlState::STRING_DATA_RIGHT_TRUNCATION,
        format!("[ISO/IEC 9075:1992 §6.10 GR6a iii, GR6b iii 3] '{text}' is longer than {t}"),
    )
}

/// Appends the key of `s` to `out`, as the module's documentation says.
fn encode(s: &str, out: &mut Vec<u8>) {
    let bytes = s.trim_end_matches(' ').as_bytes();
    // Where the first byte not a space at or after a chunk's end is: found
    // once for each run of spaces, so the key is made in linear time.
    let (mut at, mut next) = (0_usize, 0_usize);
    loop {
        let end = at.saturating_add(CHUNK).min(bytes.len());
        out.extend_from_slice(bytes.get(at..end).unwrap_or_default());
        out.resize(out.len().saturating_add(CHUNK.saturating_sub(end.saturating_sub(at))), SPACE);
        if next < end {
            let after = bytes.get(end..).unwrap_or_default();
            next = end.saturating_add(after.iter().position(|&b| b != SPACE).unwrap_or(after.len()));
        }
        match bytes.get(next) {
            // The string trimmed, the rest is none.
            None => {
                out.push(END);
                return;
            }
            Some(&b) => out.push(if b < SPACE { LESS } else { GREATER }),
        }
        at = end;
    }
}

/// The string whose key begins `rest`, without its trailing spaces.
fn decode(rest: &mut &[u8]) -> Result<String> {
    let mut bytes = Vec::new();
    loop {
        bytes.extend_from_slice(take(rest, CHUNK)?);
        match take(rest, 1)? {
            [END] => break,
            [LESS | GREATER] => {}
            _ => return Err(malformed()),
        }
    }
    let kept = bytes.iter().rposition(|&b| b != SPACE).map_or(0, |at| at.saturating_add(1));
    bytes.truncate(kept);
    String::from_utf8(bytes).map_err(|_| malformed())
}

impl SqlType for Varying {
    fn name(&self) -> &'static str {
        "CHARACTER VARYING"
    }

    fn written(&self, t: &DataType) -> String {
        format!("CHARACTER VARYING({})", length_of(t))
    }

    /// [X3.135-1986 §4.2] Character strings are comparable, of fixed or
    /// variable length.
    fn comparable(&self, other: &DataType) -> bool {
        other.is_character()
    }

    /// [ISO/IEC 9075:1992 §8.2 GR3a] after padding the shorter with spaces.
    fn compare(&self, a: &Value, b: &Value) -> Option<Ordering> {
        Some(compare_strings(string(a)?, string(b)?))
    }

    /// A string without its trailing spaces.
    fn bucket(&self, v: &Value) -> Option<Bucket> {
        Some(Bucket::Char(string(v)?.trim_end_matches(' ').to_owned()))
    }

    /// [ISO/IEC 9075:1992 §9.2 GR3d, GR3e] kept as it is, if no longer
    /// than the maximum length; else without trailing spaces beyond it.
    fn assign(&self, v: Value, t: &DataType, rule: &str) -> Result<Value> {
        let Value::Char(s) = v else { return unassignable(&v, t, rule) };
        let (count, most) = (s.chars().count(), usize::try_from(length_of(t)).unwrap_or(usize::MAX));
        if count <= most {
            return Ok(Value::Char(s));
        }
        if s.chars().skip(most).all(|c| c == ' ') {
            return Ok(Value::Char(s.chars().take(most).collect()));
        }
        fail_with(
            Code::Data,
            SqlState::STRING_DATA_RIGHT_TRUNCATION,
            format!(
                "[{}] a value of {count} characters, not spaces beyond {most}, does not fit {t}",
                assignment_rule(rule, "§9.2 GR3e ii")
            ),
        )
    }

    /// [ISO/IEC 9075:1992 §6.10 GR6] As `cast_warned`, its warning not
    /// raised.
    fn cast(&self, v: Value, t: &DataType) -> Result<Value> {
        self.cast_warned(v, t).map(|(cast, _)| cast)
    }

    /// [ISO/IEC 9075:1992 §6.10 GR6] A number's shortest literal, if no
    /// longer than the maximum length \[GR6a, GR6b\]; a string cut to it,
    /// with the warning string data, right truncation if a character cut is
    /// not a space \[GR6c\]; a bit string's characters, cut to it, with
    /// their warnings \[GR6d\] (`bits_as_characters`); a value beyond the
    /// standard, its text, as a string.
    fn cast_warned(&self, v: Value, t: &DataType) -> Result<(Value, Option<SqlState>)> {
        let length = length_of(t);
        let cut_to = |s: &str| {
            let (kept, warning) = cut(s, length);
            (Value::Char(kept), warning)
        };
        match v {
            Value::Null => Ok((Value::Null, None)),
            Value::Exact(d) => within(d.to_string(), t).map(unwarned),
            Value::Approx(x) => within(approximate_literal(x), t).map(unwarned),
            Value::Char(s) => Ok(cut_to(&s)),
            Value::Bits(b) => bits_as_characters(&b, length, false),
            // [GR6e] its literal's string, as it shows.
            Value::Datetime(d) => super::datetime::as_string(&d, length, false).map(unwarned),
            Value::Interval(i) => super::datetime::fitted(i.to_string(), length, false).map(unwarned),
            Value::Extension(ref e) => Ok(cut_to(&e.kind.text(&v))),
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
        Ok(DataType::Varying { length: r.u32()? })
    }

    /// Of variable length.
    fn record_width(&self, _: &DataType) -> Option<usize> {
        None
    }

    /// As it is, trailing spaces and all.
    fn encode_record(&self, v: &Value, t: &DataType) -> Result<Vec<u8>> {
        Ok(string(v).ok_or_else(|| mismatch(v, t))?.as_bytes().to_vec())
    }

    fn decode_record(&self, field: &[u8], _: &DataType) -> Option<Value> {
        std::str::from_utf8(field).ok().map(|s| Value::Char(s.to_owned()))
    }

    fn encode_key(&self, v: &Value, t: &DataType, out: &mut Vec<u8>) -> Result<()> {
        encode(string(v).ok_or_else(|| mismatch(v, t))?, out);
        Ok(())
    }

    fn decode_key(&self, rest: &mut &[u8], _: &DataType) -> Result<Value> {
        decode(rest).map(Value::Char)
    }

    /// Itself: a string of any length has a key.
    fn anchor(&self, v: &Value, _: &DataType) -> Option<(Value, bool)> {
        string(v).map(|_| (v.clone(), true))
    }
}

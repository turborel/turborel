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
use crate::value::cast::{approximate_literal, padded, uncastable};
use crate::value::{Bucket, CharUnits, DataType, Value, assignment_rule, compare_strings, unassignable};

/// The kind of CHARACTER(length).
#[derive(Debug)]
pub struct Character;

/// The one value of it.
pub static CHARACTER: Character = Character;

/// [§21.3.5 Description 3] Octets a character takes at most: four, in
/// UTF-8.
pub(super) const OCTETS_PER_CHARACTER: usize = 4;

/// The length of `t`, a character string type, and what it counts.
fn length_of(t: &DataType) -> (u32, CharUnits) {
    if let DataType::Character { length, units } = t { (*length, *units) } else { (0, CharUnits::Characters) }
}

/// The tag of `units` in a type's parameters.
pub(super) fn put_units(units: CharUnits, out: &mut Vec<u8>) {
    out.put_u8(match units {
        CharUnits::Characters => 0,
        CharUnits::Octets => 1,
    });
}

/// The units whose tag `r` reads.
pub(super) fn read_units(r: &mut Reader<'_>) -> Result<CharUnits> {
    match r.u8()? {
        0 => Ok(CharUnits::Characters),
        1 => Ok(CharUnits::Octets),
        _ => Err(super::malformed()),
    }
}

/// `t` written: its key word and length, and OCTETS if it counts them
/// [ISO/IEC 9075-2:2023 §6.1].
pub(super) fn written_as(name: &str, length: u32, units: CharUnits) -> String {
    match units {
        CharUnits::Characters => format!("{name}({length})"),
        CharUnits::Octets => format!("{name}({length} OCTETS)"),
    }
}

/// Octets a value of `length` in `units` takes at most, as UTF-8.
pub(super) fn octets_of(length: u32, units: CharUnits) -> usize {
    let per = if units == CharUnits::Octets { 1 } else { OCTETS_PER_CHARACTER };
    usize::try_from(length).unwrap_or(usize::MAX).saturating_mul(per)
}

/// The value's string, if it is one.
pub(super) fn string(v: &Value) -> Option<&str> {
    if let Value::Char(s) = v { Some(s) } else { None }
}

/// A value cast with no warning.
pub(super) fn unwarned(v: Value) -> (Value, Option<SqlState>) {
    (v, None)
}

/// [ISO/IEC 9075:1992 §6.10 GR5c ii, GR6c ii] The first `length`
/// characters of `s`, or whole characters of no more than `length` octets
/// [ISO/IEC 9075-2:2023 §6.1 SR16b], and the warning string data, right
/// truncation if any after them is not a space.
pub(super) fn cut(s: &str, length: u32, units: CharUnits) -> (String, Option<SqlState>) {
    let kept = units.prefix(s, usize::try_from(length).unwrap_or(usize::MAX));
    let lost = s.get(kept.len()..).unwrap_or_default().chars().any(|c| c != ' ');
    (kept.to_owned(), lost.then_some(SqlState::STRING_TRUNCATED))
}

/// A value not of a column of `t`'s type.
/// `s` followed by `pad` spaces. As every CHARACTER read takes this path,
/// it allocates once and formats nothing.
fn spaced(s: &str, pad: usize) -> String {
    let mut text = String::with_capacity(s.len().saturating_add(pad));
    text.push_str(s);
    text.extend(std::iter::repeat_n(' ', pad));
    text
}

fn mismatch(v: &Value, t: &DataType) -> SqlError {
    SqlError::new(Code::Api, format!("[DESIGN] {v} is not a value of a column of type {t}"))
}

impl SqlType for Character {
    fn name(&self) -> &'static str {
        "CHARACTER"
    }

    fn written(&self, t: &DataType) -> String {
        let (length, units) = length_of(t);
        written_as("CHARACTER", length, units)
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
    /// length, or without trailing spaces beyond it; in octets, of a
    /// length in octets [ISO/IEC 9075-2:2023 §6.1 SR16b].
    fn assign(&self, v: Value, t: &DataType, rule: &str) -> Result<Value> {
        let Value::Char(s) = v else { return unassignable(&v, t, rule) };
        let (length, units) = length_of(t);
        let (count, fits) = (units.count(&s), usize::try_from(length).unwrap_or(usize::MAX));
        if count <= fits {
            return Ok(Value::Char(spaced(&s, fits.saturating_sub(count))));
        }
        let kept = units.prefix(&s, fits);
        if s.get(kept.len()..).unwrap_or_default().chars().all(|c| c == ' ') {
            return Ok(Value::Char(spaced(kept, fits.saturating_sub(units.count(kept)))));
        }
        let unit = if units == CharUnits::Octets { "octets" } else { "characters" };
        fail_with(
            Code::Data,
            SqlState::STRING_DATA_RIGHT_TRUNCATION,
            format!("[{}] a value of {count} {unit}, not spaces beyond {length}, does not fit {t}", assignment_rule(rule, "§9.2 GR3b ii")),
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
    /// \[GR5c ii\]; no binary string [ISO/IEC 9075-2:2023 §6.13 SR7].
    fn cast_warned(&self, v: Value, t: &DataType) -> Result<(Value, Option<SqlState>)> {
        let (length, units) = length_of(t);
        match v {
            Value::Null => Ok((Value::Null, None)),
            Value::Exact(d) => padded(&d.to_string(), length).map(unwarned),
            Value::Approx(x) => padded(&approximate_literal(x), length).map(unwarned),
            // [ISO/IEC 9075-2:2023 §6.13 GR11c] its literal (IV110).
            Value::DecFloat(d) => padded(&d.to_string(), length).map(unwarned),
            // A value of a kind beyond the standard, as its kind shows it.
            Value::Extension(ref e) => padded(&e.kind.text(&v), length).map(unwarned),
            Value::Char(s) => {
                let (kept, warning) = cut(&s, length, units);
                Ok((self.assign(Value::Char(kept), t, "ISO/IEC 9075:1992 §6.10 GR5c")?, warning))
            }
            // [GR5e] its literal's string, as it shows, padded.
            Value::Datetime(d) => super::datetime::as_string(&d, length, true).map(unwarned),
            Value::Interval(i) => super::datetime::fitted(i.to_string(), length, true).map(unwarned),
            Value::Binary(_) => uncastable(&v, t).map(unwarned),
            Value::Bool(b) => super::boolean_text(b, t).map(unwarned),
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
    /// UTF-8 takes, or its length of a length in octets.
    fn descriptor(&self, t: &DataType) -> Descriptor {
        let (length, units) = length_of(t);
        Descriptor { length: Some(length), octets: Some(octets_of(length, units)), collated: true, ..Descriptor::of(self.name()) }
    }

    fn put_value(&self, v: &Value, out: &mut Vec<u8>) {
        out.put_str(string(v).unwrap_or_default());
    }

    fn value(&self, r: &mut Reader<'_>) -> Result<Value> {
        Ok(Value::Char(r.str()?))
    }

    fn put_params(&self, t: &DataType, out: &mut Vec<u8>) {
        let (length, units) = length_of(t);
        out.put_u32(length);
        put_units(units, out);
    }

    fn params(&self, r: &mut Reader<'_>) -> Result<DataType> {
        Ok(DataType::Character { length: r.u32()?, units: read_units(r)? })
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
        let (length, units) = length_of(t);
        let pad = usize::try_from(length).ok()?.checked_sub(units.count(s))?;
        Some(Value::Char(spaced(s, pad)))
    }

    /// Padded with spaces to the length [X3.135-1986 §5.11 GR5]: Unicode
    /// scalar order [GR6, implementor-defined] is the order of the UTF-8
    /// bytes.
    fn encode_key(&self, v: &Value, t: &DataType, out: &mut Vec<u8>) -> Result<()> {
        let s = string(v).ok_or_else(|| mismatch(v, t))?;
        let (length, units) = length_of(t);
        let pad = usize::try_from(length).ok().and_then(|n| n.checked_sub(units.count(s))).ok_or_else(|| mismatch(v, t))?;
        out.extend_from_slice(s.as_bytes());
        out.resize(out.len().saturating_add(pad), b' ');
        Ok(())
    }

    /// The value ends where the character after its `length`-th begins: at
    /// a byte that does not continue a character; of a length in octets,
    /// after so many.
    fn decode_key(&self, rest: &mut &[u8], t: &DataType) -> Result<Value> {
        let (length, units) = length_of(t);
        let length = usize::try_from(length).map_err(|_| super::malformed())?;
        if units == CharUnits::Octets {
            let s = std::str::from_utf8(super::take(rest, length)?).map_err(|_| super::malformed())?;
            return Ok(Value::Char(s.to_owned()));
        }
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
        let (length, units) = length_of(t);
        let length = usize::try_from(length).ok()?;
        Some((Value::Char(units.prefix(s, length).to_owned()), units.count(s) <= length))
    }
}

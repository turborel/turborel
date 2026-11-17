//! The mapping of SQL values to XML [ISO/IEC 9075-14:2006 draft §9.8]:
//! the lexical form of an SQL value of a predefined type, as XMLCAST
//! [§6.6 GR3a] and the publishing functions write it.

use std::fmt::Write;

use super::{Input, input};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::value::datetime::{Field, MICROS_PER_SECOND};
use crate::value::interval::{Interval, Qualifier};
use crate::value::{CharUnits, DataType, MAX_CHARACTER_LENGTH, Value};
use crate::xml::{Version, hex_text, is_char};

/// [§9.5 GR3, §9.8 GR3] How binary strings are encoded: in base64 or in
/// hex [Schema2 §3.2.16, §3.2.15].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryEncoding {
    /// BASE64: `xs:base64Binary`.
    Base64,
    /// HEX: `xs:hexBinary`.
    Hex,
}

/// [Schema2 §3.2.16] The 64 characters of base64, each standing for six
/// bits, in order [RFC 2045 §6.8].
const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
/// [Schema2 §3.2.16] The character that pads base64 to a whole group.
const PAD: char = '=';
/// [Schema2 §3.2.16] The octets of a group, which four characters write.
const GROUP: usize = 3;
/// The bits each character of base64 stands for.
const SIX_BITS: u32 = 6;
/// The low six bits.
const SIX_MASK: u32 = 0x3F;

impl BinaryEncoding {
    /// `octets` in this encoding [§9.8 GR9b]: hex, upper case [Schema2
    /// §3.2.15.2]; or base64, of no line breaks, padded [Schema2 §3.2.16].
    #[must_use]
    pub fn encode(self, octets: &[u8]) -> String {
        match self {
            BinaryEncoding::Hex => hex_text(octets),
            BinaryEncoding::Base64 => base64(octets),
        }
    }
}

/// `octets` in base64: each three octets four characters of six bits, a
/// last group of fewer padded with `=` [Schema2 §3.2.16].
fn base64(octets: &[u8]) -> String {
    let mut out = String::new();
    for group in octets.chunks(GROUP) {
        let mut word = [0_u8; 4];
        for (w, o) in word.iter_mut().skip(1).zip(group) {
            *w = *o;
        }
        let word = u32::from_be_bytes(word);
        for k in 0..=GROUP {
            if k > group.len() {
                out.push(PAD);
                continue;
            }
            let shift = u32::try_from(GROUP.saturating_sub(k)).unwrap_or(0).saturating_mul(SIX_BITS);
            let six = word.checked_shr(shift).unwrap_or(0) & SIX_MASK;
            out.push(char::from(BASE64.get(usize::try_from(six).unwrap_or(0)).copied().unwrap_or(b'A')));
        }
    }
    out
}

/// [§9.8 GR9] The lexical form of `v`, not null, of an SQL type, as XML
/// text: a character string's characters, `&`, `<`, `>` and a carriage
/// return as references if `charmapping` \[GR4, GR9a iii\]; a binary
/// string in hex or base64 as `encoding` says \[GR3, GR9b\]; a number as CAST to a character string writes it \[GR7,
/// GR9c\]; a truth value in lower case \[GR9d\]; a date, time or timestamp
/// as its literal's string, T between date and time \[GR9e to GR9g\]; an
/// interval as a duration \[GR9h\].
///
/// # Errors
///
/// SQLSTATE 0N002 of a character no XML character [GR9a ii]; the errors
/// of reading a large object.
pub fn xml_lexical(v: &Value, encoding: BinaryEncoding, charmapping: bool) -> Result<String> {
    match v {
        Value::Char(s) => characters(s, charmapping),
        Value::Binary(b) => Ok(encoding.encode(b.bytes())),
        Value::Lob(_) => match input(v)? {
            Some(Input::Text(s)) => characters(&s, charmapping),
            Some(Input::Octets(o)) => Ok(encoding.encode(&o)),
            None => Ok(String::new()),
        },
        Value::Exact(_) | Value::Approx(_) => as_string(v),
        Value::Bool(b) => Ok(b.to_string()),
        Value::Datetime(d) => Ok(d.to_string().replacen(' ', "T", 1)),
        Value::Interval(i) => Ok(duration_text(i)),
        Value::Null
        | Value::DecFloat(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Row(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Xml(_)
        | Value::Extension(_) => Err(SqlError::new(Code::Api, format!("[DESIGN] the value {v} is of no type §9.8 maps to XML"))),
    }
}

/// [§9.8 GR7] `v` CAST to CHARACTER VARYING of the most characters.
fn as_string(v: &Value) -> Result<String> {
    let varying = DataType::Varying { length: MAX_CHARACTER_LENGTH, units: CharUnits::default() };
    let cast = crate::value::cast(v.clone(), &varying)?;
    if let Value::Char(s) = cast { Ok(s) } else { Ok(cast.to_string()) }
}

/// [§9.8 GR9a] The string `s` as XML text: each character an XML 1.1
/// character, else SQLSTATE 0N002 \[GR9a ii 1\], and, if `charmapping`,
/// `&`, `<`, `>` and a carriage return written as references \[GR9a iii
/// 1\].
fn characters(s: &str, charmapping: bool) -> Result<String> {
    if let Some(bad) = s.chars().find(|&c| !is_char(Version::V11, c)) {
        return Err(SqlError::new(
            Code::Data,
            format!("[ISO/IEC 9075-14:2006 draft §9.8 GR9a ii] U+{:04X} is no XML 1.1 character", u32::from(bad)),
        )
        .with_state(SqlState::INVALID_XML_CHARACTER));
    }
    if !charmapping {
        return Ok(s.to_owned());
    }
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\r' => out.push_str("&#x0d;"),
            other => out.push(other),
        }
    }
    Ok(out)
}

/// [§9.8 GR9h] The interval `i` as a duration: its sign, P, then each of
/// its fields and its designator, the leading one of the digits of its
/// precision and the others of two, T before the first of hours, minutes
/// and seconds, the seconds' fraction of the digits of theirs.
fn duration_text(i: &Interval) -> String {
    let q: Qualifier = i.qualifier;
    let second: u128 = MICROS_PER_SECOND.unsigned_abs().into();
    let mut out = String::from(if i.units < 0 { "-P" } else { "P" });
    for field in q.fields() {
        let n = i.field(field).unwrap_or(0).unsigned_abs();
        let width = if field == q.start { usize::from(q.leading) } else { 2 };
        let (designator, whole) = match field {
            Field::Year => ('Y', n),
            Field::Month | Field::Minute => ('M', n),
            Field::Day => ('D', n),
            Field::Hour => ('H', n),
            Field::Second => ('S', n.checked_div(second).unwrap_or(0)),
        };
        if matches!(field, Field::Hour | Field::Minute | Field::Second) && !out.contains('T') {
            out.push('T');
        }
        let _ = write!(out, "{whole:0width$}");
        if field == Field::Second && q.fraction > 0 {
            let fraction = format!("{:06}", n.checked_rem(second).unwrap_or(0));
            out.push('.');
            out.push_str(fraction.get(..usize::from(q.fraction)).unwrap_or_default());
        }
        out.push(designator);
    }
    out
}

#[cfg(test)]
mod tests;

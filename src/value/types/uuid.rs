//! \[EXTENSION\] UUID, a universally unique identifier of RFC 9562: 16
//! octets, built in (RULES.md D180). A value of it is an extension value
//! (RULES.md D53) whose bytes are its 16 octets in the order RFC 9562 §4
//! writes them, so its values order as their octets do, unsigned, from
//! the first, and its text orders the same way.
//!
//! Its text is 32 hexadecimal digits in five groups, 8-4-4-4-12, joined
//! by hyphens, lower case (RULES.md D181). CAST from a character string
//! reads that form in either case, with or without the hyphens, braced or
//! not; from a binary string, 16 octets. CAST to a character string gives
//! the text, to a binary string the octets. A character string is stored
//! in it as CAST makes one, so a UUID column takes text in INSERT.
//!
//! Its record is its 16 octets, fixed, and so is its index key, whose
//! order is its values', so a btree and a hash index both index it. Its
//! format is at version 1 (VERSIONING.md). UUID_V4 and UUID_V7 make
//! values of it (`uuid/generate.rs`).

use std::cmp::Ordering;
use std::sync::Arc;

use super::{Descriptor, ExtensionType, ExtensionValue, TypeParams, take};
use crate::bytes::{Put, Reader};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::value::{Binary, Bucket, DataType, SqlType, Value};

/// The name of the UUID type.
pub const UUID_NAME: &str = "UUID";
/// [RFC 9562 §4] The octets of a UUID.
pub const UUID_OCTETS: usize = 16;
/// [RFC 9562 §4] The octets of each group of its text, in order.
const GROUPS: [usize; 5] = [4, 2, 2, 2, 6];
/// The hexadecimal digits of an octet.
const HEXITS_PER_OCTET: usize = 2;
/// The hexadecimal digits of a UUID.
const HEXITS_PER_UUID: usize = UUID_OCTETS * HEXITS_PER_OCTET;
/// [RFC 9562 §4] The characters of a UUID's text: its digits, and a
/// hyphen between each two groups.
pub const UUID_TEXT_LENGTH: usize = HEXITS_PER_UUID + GROUPS.len() - 1;
/// The hexadecimal digits after which its text may have a hyphen: each
/// group of four.
const HEXITS_PER_HYPHEN: usize = 4;
/// The hexadecimal digits, lower case, by value.
const HEXITS: &[u8; 16] = b"0123456789abcdef";
/// The radix of a hexadecimal digit.
const HEX_RADIX: u32 = 16;
/// The bits of half an octet.
const NIBBLE: u32 = 4;
/// [RFC 9562 §4.2] The octet whose high half is the version.
const VERSION_OCTET: usize = 6;

/// The UUID type.
#[derive(Debug)]
pub struct UuidType;

/// The kind of UUID values.
pub static UUID_TYPE: UuidType = UuidType;

/// It, as a type of it holds it.
static UUID_KIND: &dyn SqlType = &UUID_TYPE;

/// The kind of UUID, as the type registry holds it.
pub(crate) fn kind() -> &'static &'static dyn SqlType {
    &UUID_KIND
}

impl UuidType {
    /// The UUID type.
    pub fn data_type() -> DataType {
        DataType::Extension(ExtensionType { kind: &UUID_KIND, params: TypeParams::default() })
    }
}

/// Whether `t` is the UUID type.
pub fn is_uuid(t: &DataType) -> bool {
    matches!(t, DataType::Extension(e) if e.kind.name() == UUID_NAME)
}

/// The UUID value of `octets`.
pub fn uuid_value(octets: [u8; UUID_OCTETS]) -> Value {
    Value::Extension(ExtensionValue { kind: &UUID_TYPE, bytes: Arc::from(octets.as_slice()) })
}

/// The octets of `v`, if it is a UUID value.
pub fn octets_of(v: &Value) -> Option<[u8; UUID_OCTETS]> {
    let Value::Extension(e) = v else { return None };
    if e.kind.name() != UUID_NAME {
        return None;
    }
    <[u8; UUID_OCTETS]>::try_from(e.bytes.as_ref()).ok()
}

/// [RFC 9562 §4.2] The version of `octets`: the high half of octet 6.
pub fn version(octets: &[u8; UUID_OCTETS]) -> u8 {
    octets.get(VERSION_OCTET).map_or(0, |o| o.checked_shr(NIBBLE).unwrap_or(0))
}

/// [RFC 9562 §4] The text of `octets`: five groups of lower-case
/// hexadecimal digits, joined by hyphens.
pub fn text(octets: &[u8; UUID_OCTETS]) -> String {
    let mut out = String::with_capacity(UUID_TEXT_LENGTH);
    let mut rest = octets.as_slice();
    for (i, n) in GROUPS.iter().enumerate() {
        if i > 0 {
            out.push('-');
        }
        let (group, after) = rest.split_at((*n).min(rest.len()));
        for octet in group {
            for half in [octet.checked_shr(NIBBLE).unwrap_or(0), octet & 0x0f] {
                out.push(char::from(HEXITS.get(usize::from(half)).copied().unwrap_or(b'0')));
            }
        }
        rest = after;
    }
    out
}

/// The octets `text` writes, as CAST reads it: 32 hexadecimal digits in
/// either case, its spaces trimmed, braced or not, a hyphen allowed after
/// each group of four digits but the last; none if it writes none.
pub fn parse(text: &str) -> Option<[u8; UUID_OCTETS]> {
    let t = text.trim();
    let t = t.strip_prefix('{').and_then(|b| b.strip_suffix('}')).unwrap_or(t);
    let mut octets = [0_u8; UUID_OCTETS];
    let mut digits = 0_usize;
    let mut chars = t.chars().peekable();
    while let Some(c) = chars.next() {
        let value = u8::try_from(c.to_digit(HEX_RADIX)?).ok()?;
        let octet = octets.get_mut(digits.checked_div(HEXITS_PER_OCTET)?)?;
        *octet = octet.checked_shl(NIBBLE).unwrap_or(0) | value;
        digits = digits.checked_add(1)?;
        let grouped = digits.checked_rem(HEXITS_PER_HYPHEN) == Some(0);
        if chars.peek() == Some(&'-') && grouped && digits < HEXITS_PER_UUID {
            chars.next();
        }
    }
    (digits == HEXITS_PER_UUID).then_some(octets)
}

/// The UUID a character string writes, else SQLSTATE 22018, as text
/// that writes no value of a type is cast (RULES.md D181).
fn from_text(text: &str) -> Result<Value> {
    parse(text).map(uuid_value).ok_or_else(|| {
        SqlError::new(Code::Data, format!("[EXTENSION] '{text}' is not a UUID's text"))
            .with_state(SqlState::INVALID_CHARACTER_VALUE_FOR_CAST)
    })
}

/// `v` as a UUID: itself, or of a character string its text, or of a
/// binary string of 16 octets those octets; none of another value.
fn of_value(v: Value) -> Option<Result<Value>> {
    match &v {
        Value::Null => Some(Ok(v)),
        Value::Char(s) => Some(from_text(s)),
        Value::Binary(b) => Some(<[u8; UUID_OCTETS]>::try_from(b.bytes()).map(uuid_value).map_err(|_| {
            SqlError::new(Code::Data, format!("[EXTENSION] a binary string of {} octets is no UUID, of 16", b.bytes().len()))
                .with_state(SqlState::INVALID_CHARACTER_VALUE_FOR_CAST)
        })),
        Value::Extension(_) => octets_of(&v).map(|_| Ok(v)),
        Value::Exact(_)
        | Value::Approx(_)
        | Value::DecFloat(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Lob(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Row(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Json(_)
        | Value::Xml(_) => None,
    }
}

/// That `v` is not a UUID value.
fn mismatch(v: &Value) -> SqlError {
    SqlError::new(Code::Api, format!("[DESIGN] {v} is not a UUID value"))
}

/// The octets of `v`, a UUID value, else the error of one that is not.
fn octets(v: &Value) -> Result<[u8; UUID_OCTETS]> {
    octets_of(v).ok_or_else(|| mismatch(v))
}

/// `octets`, its first eight as a number, high octet first: where a value
/// sits on a line, for a histogram.
fn line_position(octets: &[u8; UUID_OCTETS]) -> f64 {
    let half = |range: std::ops::Range<usize>| {
        octets.get(range).map_or(0.0_f64, |b| f64::from(b.iter().fold(0_u32, |n, o| n.checked_shl(8).unwrap_or(0) | u32::from(*o))))
    };
    half(0..4) * (f64::from(u32::MAX) + 1.0) + half(4..8)
}

impl SqlType for UuidType {
    fn name(&self) -> &'static str {
        UUID_NAME
    }

    fn written(&self, _: &DataType) -> String {
        UUID_NAME.to_owned()
    }

    fn comparable(&self, other: &DataType) -> bool {
        is_uuid(other)
    }

    fn compare(&self, a: &Value, b: &Value) -> Option<Ordering> {
        Some(octets_of(a)?.cmp(&octets_of(b)?))
    }

    fn bucket(&self, v: &Value) -> Option<Bucket> {
        Some(Bucket::Extension(octets_of(v)?.into()))
    }

    fn assign(&self, v: Value, _: &DataType, rule: &str) -> Result<Value> {
        if v.is_null() || octets_of(&v).is_some() {
            return Ok(v);
        }
        Err(SqlError::new(Code::Data, format!("[{rule}] {v} is not a UUID value")).with_state(SqlState::ERROR_IN_ASSIGNMENT))
    }

    /// A character string is stored in it as CAST makes it one.
    fn assigns_text(&self) -> bool {
        true
    }

    fn cast(&self, v: Value, t: &DataType) -> Result<Value> {
        let shown = v.to_string();
        of_value(v).unwrap_or_else(|| {
            Err(SqlError::new(Code::Data, format!("[ISO/IEC 9075:1992 §6.10 SR4] the value {shown} cannot be cast to {t}"))
                .with_state(SqlState::ERROR_IN_ASSIGNMENT))
        })
    }

    /// To a character string its text, to a binary string its octets.
    fn lowered(&self, v: &Value, to: &DataType) -> Option<Value> {
        let octets = octets_of(v)?;
        if to.is_character() {
            return Some(Value::Char(text(&octets).into()));
        }
        to.is_binary().then(|| Value::Binary(Binary::of(octets)))
    }

    fn literal(&self, v: &Value) -> String {
        octets_of(v).map_or_else(String::new, |o| format!("{UUID_NAME} '{}'", text(&o)))
    }

    fn text(&self, v: &Value) -> String {
        octets_of(v).map_or_else(String::new, |o| text(&o))
    }

    fn position(&self, v: &Value) -> Option<f64> {
        octets_of(v).map(|o| line_position(&o))
    }

    fn descriptor(&self, _: &DataType) -> Descriptor {
        Descriptor::of(UUID_NAME)
    }

    fn put_value(&self, v: &Value, out: &mut Vec<u8>) {
        out.put_bytes(&octets_of(v).unwrap_or_default());
    }

    fn value(&self, r: &mut Reader<'_>) -> Result<Value> {
        let raw = r.bytes()?;
        <[u8; UUID_OCTETS]>::try_from(raw).map(uuid_value).map_err(|_| r.corrupt("a UUID is 16 octets"))
    }

    fn put_params(&self, _: &DataType, _: &mut Vec<u8>) {}

    fn params(&self, _: &mut Reader<'_>) -> Result<DataType> {
        Ok(UuidType::data_type())
    }

    fn record_width(&self, _: &DataType) -> Option<usize> {
        Some(UUID_OCTETS)
    }

    fn encode_record(&self, v: &Value, _: &DataType) -> Result<Vec<u8>> {
        octets(v).map(|o| o.to_vec())
    }

    fn decode_record(&self, field: &[u8], _: &DataType) -> Option<Value> {
        <[u8; UUID_OCTETS]>::try_from(field).ok().map(uuid_value)
    }

    /// Its 16 octets: of one width, so no key begins another, and ordered
    /// as the values are.
    fn encode_key(&self, v: &Value, _: &DataType, out: &mut Vec<u8>) -> Result<()> {
        out.extend_from_slice(&octets(v)?);
        Ok(())
    }

    fn decode_key(&self, rest: &mut &[u8], _: &DataType) -> Result<Value> {
        let key = take(rest, UUID_OCTETS)?;
        <[u8; UUID_OCTETS]>::try_from(key).map(uuid_value).map_err(|_| super::malformed())
    }

    fn anchor(&self, v: &Value, _: &DataType) -> Option<(Value, bool)> {
        octets_of(v).map(|_| (v.clone(), true))
    }
}

pub mod generate;

// Unit tests: text, casts, order and bytes of UUID (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;

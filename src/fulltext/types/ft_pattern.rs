//! The FT_Pattern type [ISO/IEC 13249-2:2002 (CD, SC32 N0640) §5.3.1]: a
//! linear search pattern, `CREATE TYPE FT_Pattern AS CHARACTER
//! VARYING(FT_MaxPatternLength) FINAL`, a distinct type of a character
//! string. An extension type's values are its own (RULES.md D53), so an
//! FT_Pattern value is a version octet then its text in UTF-8.
//!
//! A character string is stored in it by store assignment, as
//! Contains(CHARACTER VARYING) and Rank(CHARACTER VARYING) cast their
//! pattern to it [§5.1.2, §5.1.3 Definition], so one function of each name
//! takes either. It casts from and to a character string, its text, and
//! compares with its own values alone, as their texts do in code-point
//! order, as a distinct type's values compare as their source's.

use std::cmp::Ordering;
use std::sync::Arc;

use super::{FT_MAX_PATTERN_LENGTH, fits, mismatch, payload, put_key, read_payload, take_key, too_long, type_of};
use crate::bytes::{Put, Reader};
use crate::error::{Code, Result, SqlError};
use crate::value::types::{Descriptor, ExtensionValue};
use crate::value::{Bucket, DataType, SqlType, Value};

/// The name of the FT_Pattern type.
pub const FT_PATTERN: &str = "FT_PATTERN";

/// The payload's version.
const VERSION: u8 = 1;

/// The FT_PATTERN type.
#[derive(Debug)]
pub struct FtPatternType;

/// The kind of FT_Pattern values.
pub static FT_PATTERN_TYPE: FtPatternType = FtPatternType;

/// It, as a type of it holds it.
static FT_PATTERN_KIND: &dyn SqlType = &FT_PATTERN_TYPE;

/// The kind of FT_PATTERN, as the type registry holds it.
pub(super) fn kind() -> &'static &'static dyn SqlType {
    &FT_PATTERN_KIND
}

impl FtPatternType {
    /// The FT_PATTERN type.
    pub fn data_type() -> DataType {
        type_of(&FT_PATTERN_KIND)
    }
}

/// The FT_Pattern value of `text`.
///
/// # Errors
///
/// SQLSTATE 22001 if `text` is longer than FT_MaxPatternLength.
pub fn ft_pattern(text: &str) -> Result<Value> {
    if !fits(text, FT_MAX_PATTERN_LENGTH) {
        return Err(too_long("FT_Pattern value", FT_MAX_PATTERN_LENGTH));
    }
    let mut bytes = Vec::with_capacity(text.len().saturating_add(1));
    bytes.push(VERSION);
    bytes.extend_from_slice(text.as_bytes());
    Ok(Value::Extension(ExtensionValue { kind: &FT_PATTERN_TYPE, bytes: bytes.into() }))
}

/// The text of payload `bytes`, if they are one.
fn text_of(bytes: &[u8]) -> Option<&str> {
    let (&VERSION, text) = bytes.split_first()? else { return None };
    std::str::from_utf8(text).ok()
}

/// The text of `v`, an FT_Pattern value, if it is one.
pub(crate) fn pattern_text(v: &Value) -> Option<&str> {
    text_of(payload(v, FT_PATTERN)?)
}

/// The value of payload `bytes`, checked.
fn of_payload(bytes: &[u8]) -> Option<Value> {
    text_of(bytes)?;
    Some(Value::Extension(ExtensionValue { kind: &FT_PATTERN_TYPE, bytes: Arc::from(bytes) }))
}

impl SqlType for FtPatternType {
    fn name(&self) -> &'static str {
        FT_PATTERN
    }

    fn written(&self, _: &DataType) -> String {
        FT_PATTERN.to_owned()
    }

    fn comparable(&self, other: &DataType) -> bool {
        super::is_type(other, FT_PATTERN)
    }

    fn compare(&self, a: &Value, b: &Value) -> Option<Ordering> {
        Some(payload(a, FT_PATTERN)?.cmp(payload(b, FT_PATTERN)?))
    }

    fn bucket(&self, v: &Value) -> Option<Bucket> {
        Some(Bucket::Extension(payload(v, FT_PATTERN)?.into()))
    }

    fn assign(&self, v: Value, _: &DataType, rule: &str) -> Result<Value> {
        if v.is_null() || payload(&v, FT_PATTERN).is_some() {
            return Ok(v);
        }
        Err(SqlError::new(Code::Data, format!("[{rule}] {v} is not an FT_PATTERN value")))
    }

    /// A character string is stored in it as CAST makes it one [§5.1.2
    /// Definition].
    fn assigns_text(&self) -> bool {
        true
    }

    /// A character string of at most FT_MaxPatternLength characters casts
    /// to the FT_Pattern value of its text; an FT_Pattern value to itself.
    /// The text is not read as a `<search expression>` here: Contains and
    /// Rank read it [§5.1.2 Description 3a].
    fn cast(&self, v: Value, _: &DataType) -> Result<Value> {
        if v.is_null() || payload(&v, FT_PATTERN).is_some() {
            return Ok(v);
        }
        let Value::Char(text) = &v else {
            return Err(SqlError::new(Code::Data, format!("[DESIGN] {v} does not cast to FT_PATTERN")));
        };
        ft_pattern(text)
    }

    fn literal(&self, v: &Value) -> String {
        pattern_text(v).map_or_else(String::new, |text| format!("CAST({} AS {FT_PATTERN})", super::fulltext::quoted(text)))
    }

    fn text(&self, v: &Value) -> String {
        pattern_text(v).map_or_else(String::new, str::to_owned)
    }

    fn position(&self, _: &Value) -> Option<f64> {
        None
    }

    fn descriptor(&self, _: &DataType) -> Descriptor {
        Descriptor::of(FT_PATTERN)
    }

    fn put_value(&self, v: &Value, out: &mut Vec<u8>) {
        out.put_bytes(payload(v, FT_PATTERN).unwrap_or_default());
    }

    fn value(&self, r: &mut Reader<'_>) -> Result<Value> {
        let raw = read_payload(r, |b| text_of(b).is_some(), FT_PATTERN)?;
        Ok(Value::Extension(ExtensionValue { kind: &FT_PATTERN_TYPE, bytes: Arc::from(raw) }))
    }

    fn put_params(&self, _: &DataType, _: &mut Vec<u8>) {}

    fn params(&self, _: &mut Reader<'_>) -> Result<DataType> {
        Ok(FtPatternType::data_type())
    }

    fn record_width(&self, _: &DataType) -> Option<usize> {
        None
    }

    fn encode_record(&self, v: &Value, _: &DataType) -> Result<Vec<u8>> {
        payload(v, FT_PATTERN).map(<[u8]>::to_vec).ok_or_else(|| mismatch(v, FT_PATTERN))
    }

    fn decode_record(&self, field: &[u8], _: &DataType) -> Option<Value> {
        of_payload(field)
    }

    fn encode_key(&self, v: &Value, _: &DataType, out: &mut Vec<u8>) -> Result<()> {
        put_key(payload(v, FT_PATTERN).ok_or_else(|| mismatch(v, FT_PATTERN))?, out);
        Ok(())
    }

    fn decode_key(&self, rest: &mut &[u8], _: &DataType) -> Result<Value> {
        let key = take_key(rest, FT_PATTERN)?;
        of_payload(key.bytes()).ok_or_else(|| SqlError::new(Code::Corrupt, "[DESIGN] an FT_PATTERN index key holds no FT_Pattern value"))
    }

    fn anchor(&self, v: &Value, _: &DataType) -> Option<(Value, bool)> {
        payload(v, FT_PATTERN).map(|_| (v.clone(), true))
    }
}

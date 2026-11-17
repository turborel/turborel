//! The FullText type [ISO/IEC 13249-2:2002 (CD, SC32 N0640) §5.1.1]: a
//! document's text and its language, a value of an extension type (RULES.md
//! D53) whose payload `fulltext/value.rs` writes.
//!
//! The draft gives FullText no ordering. \[DESIGN\] Values of it compare as
//! their payloads do, octet by octet: by language, in an order of their
//! names' lengths then their names, then by text in code-point order. Two
//! are so equal if they are of one language and one text, and every pair
//! is ordered, so a column of it may be grouped, sorted and keyed. Its
//! text is what it shows and what CAST to a character string gives
//! [§5.1.1 Definition, §5.1.10].

use std::cmp::Ordering;
use std::sync::Arc;

use super::{FT_DEFAULT_LANGUAGE, FT_MAX_TEXT_LENGTH, fits, mismatch, payload, put_key, read_payload, take_key, too_long, type_of};
use crate::bytes::{Put, Reader};
use crate::error::{Code, Result, SqlError};
use crate::fulltext::Language;
use crate::fulltext::value::{FULLTEXT, decode, document, encode};
use crate::value::types::{Descriptor, ExtensionValue};
use crate::value::{Bucket, DataType, SqlType, Value};

/// The FULLTEXT type.
#[derive(Debug)]
pub struct FullTextType;

/// The kind of FullText values.
pub static FULLTEXT_TYPE: FullTextType = FullTextType;

/// It, as a type of it holds it.
static FULLTEXT_KIND: &dyn SqlType = &FULLTEXT_TYPE;

/// The kind of FULLTEXT, as the type registry holds it.
pub(super) fn kind() -> &'static &'static dyn SqlType {
    &FULLTEXT_KIND
}

impl FullTextType {
    /// The FULLTEXT type.
    pub fn data_type() -> DataType {
        type_of(&FULLTEXT_KIND)
    }
}

/// The FullText value of `text` in `language` [§5.1.9].
///
/// # Errors
///
/// SQLSTATE 22001 if `text` is longer than FT_MaxTextLength.
pub fn full_text(language: Language, text: &str) -> Result<Value> {
    if !fits(text, FT_MAX_TEXT_LENGTH) {
        return Err(too_long("FullText value", FT_MAX_TEXT_LENGTH));
    }
    Ok(Value::Extension(ExtensionValue { kind: &FULLTEXT_TYPE, bytes: encode(language, text).into() }))
}

/// Whether `bytes` are a payload `encode` writes: one `decode` reads, its
/// language's name as `encode` spells it, so equal values have equal
/// bytes.
fn canonical(bytes: &[u8]) -> bool {
    decode(bytes).is_ok_and(|(language, _)| bytes.starts_with(&encode(language, "")))
}

/// The language and text of `v`, a FullText value.
///
/// # Errors
///
/// SQLCODE -910 if it is not one; -916 if its payload is malformed.
pub(super) fn parts(v: &Value) -> Result<(Language, &str)> {
    decode(payload(v, FULLTEXT).ok_or_else(|| mismatch(v, FULLTEXT))?)
}

/// The value of payload `bytes`, checked.
fn of_payload(bytes: &[u8]) -> Option<Value> {
    canonical(bytes).then(|| Value::Extension(ExtensionValue { kind: &FULLTEXT_TYPE, bytes: Arc::from(bytes) }))
}

/// A character string, or a character large object read whole, as a
/// FullText value in the default language.
fn from_text(v: &Value) -> Result<Value> {
    let chars = document(v)?.ok_or_else(|| mismatch(v, FULLTEXT))?.chars;
    let most = usize::try_from(FT_MAX_TEXT_LENGTH).unwrap_or(usize::MAX);
    let mut text = String::new();
    for (n, c) in chars.enumerate() {
        if n >= most {
            return Err(too_long("FullText value", FT_MAX_TEXT_LENGTH));
        }
        text.push(c?);
    }
    full_text(FT_DEFAULT_LANGUAGE, &text)
}

/// `text` as a character string literal.
pub(super) fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

impl SqlType for FullTextType {
    fn name(&self) -> &'static str {
        FULLTEXT
    }

    fn written(&self, _: &DataType) -> String {
        FULLTEXT.to_owned()
    }

    fn comparable(&self, other: &DataType) -> bool {
        super::is_type(other, FULLTEXT)
    }

    fn compare(&self, a: &Value, b: &Value) -> Option<Ordering> {
        Some(payload(a, FULLTEXT)?.cmp(payload(b, FULLTEXT)?))
    }

    fn bucket(&self, v: &Value) -> Option<Bucket> {
        Some(Bucket::Extension(payload(v, FULLTEXT)?.into()))
    }

    fn assign(&self, v: Value, _: &DataType, rule: &str) -> Result<Value> {
        if v.is_null() || payload(&v, FULLTEXT).is_some() {
            return Ok(v);
        }
        Err(SqlError::new(Code::Data, format!("[{rule}] {v} is not a FULLTEXT value")))
    }

    /// A character string, or a character large object of at most
    /// FT_MaxTextLength characters, casts to the FullText value of its
    /// text in the default language, as FullText(CHARACTER VARYING) makes
    /// one [§5.1.9]; a FullText value casts to itself.
    fn cast(&self, v: Value, _: &DataType) -> Result<Value> {
        if v.is_null() || payload(&v, FULLTEXT).is_some() {
            return Ok(v);
        }
        if matches!(v, Value::Char(_) | Value::Lob(_)) {
            return from_text(&v);
        }
        Err(SqlError::new(Code::Data, format!("[DESIGN] {v} does not cast to FULLTEXT")))
    }

    fn literal(&self, v: &Value) -> String {
        parts(v).map_or_else(|_| String::new(), |(language, text)| format!("{FULLTEXT}({}, '{}')", quoted(text), language.name()))
    }

    fn text(&self, v: &Value) -> String {
        parts(v).map_or_else(|_| String::new(), |(_, text)| text.to_owned())
    }

    fn position(&self, _: &Value) -> Option<f64> {
        None
    }

    fn descriptor(&self, _: &DataType) -> Descriptor {
        Descriptor::of(FULLTEXT)
    }

    fn put_value(&self, v: &Value, out: &mut Vec<u8>) {
        out.put_bytes(payload(v, FULLTEXT).unwrap_or_default());
    }

    fn value(&self, r: &mut Reader<'_>) -> Result<Value> {
        let raw = read_payload(r, canonical, FULLTEXT)?;
        Ok(Value::Extension(ExtensionValue { kind: &FULLTEXT_TYPE, bytes: Arc::from(raw) }))
    }

    fn put_params(&self, _: &DataType, _: &mut Vec<u8>) {}

    fn params(&self, _: &mut Reader<'_>) -> Result<DataType> {
        Ok(FullTextType::data_type())
    }

    fn record_width(&self, _: &DataType) -> Option<usize> {
        None
    }

    fn encode_record(&self, v: &Value, _: &DataType) -> Result<Vec<u8>> {
        payload(v, FULLTEXT).map(<[u8]>::to_vec).ok_or_else(|| mismatch(v, FULLTEXT))
    }

    fn decode_record(&self, field: &[u8], _: &DataType) -> Option<Value> {
        of_payload(field)
    }

    fn encode_key(&self, v: &Value, _: &DataType, out: &mut Vec<u8>) -> Result<()> {
        put_key(payload(v, FULLTEXT).ok_or_else(|| mismatch(v, FULLTEXT))?, out);
        Ok(())
    }

    fn decode_key(&self, rest: &mut &[u8], _: &DataType) -> Result<Value> {
        let key = take_key(rest, FULLTEXT)?;
        of_payload(key.bytes()).ok_or_else(|| SqlError::new(Code::Corrupt, "[DESIGN] a FULLTEXT index key holds no FullText value"))
    }

    fn anchor(&self, v: &Value, _: &DataType) -> Option<(Value, bool)> {
        payload(v, FULLTEXT).map(|_| (v.clone(), true))
    }
}

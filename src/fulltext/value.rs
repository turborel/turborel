//! A document as Contains, Rank and a full-text index read it: the text
//! of a FullText value [ISO/IEC 13249-2:2002 (CD, SC32 N0640) §5.1.1],
//! with its language, or a character string's or character large
//! object's, of none (RULES.md D87).
//!
//! A FullText value's payload is a version byte, its language's name, a
//! byte long then its bytes, and the text in UTF-8.

use crate::bytes::Reader;
use crate::error::{Code, Result, SqlError};
use crate::fulltext::Language;
use crate::index::fulltext::{DocChars, chars_of};
use crate::value::Value;

/// The name of the FullText type.
pub const FULLTEXT: &str = "FULLTEXT";

/// The payload's version.
const VERSION: u8 = 1;

/// A document's characters, read as they are taken, and its language if
/// it names one.
pub(crate) struct Document<'v> {
    /// The language of a FullText value; none of a character string.
    pub language: Option<Language>,
    /// Its characters.
    pub chars: DocChars<'v>,
}

/// The document `value` holds: of a FullText value, its text and
/// language; of a character string or character large object, its
/// characters; none if it is null.
///
/// # Errors
///
/// SQLCODE -900 if it is no text; -916 if a FullText payload is
/// malformed.
pub(crate) fn document(value: &Value) -> Result<Option<Document<'_>>> {
    let Value::Extension(e) = value else {
        return Ok(chars_of(value)?.map(|chars| Document { language: None, chars }));
    };
    if e.kind.name() != FULLTEXT {
        return Err(SqlError::new(Code::Unsupported, format!("[DESIGN] a {} value is no document", e.kind.name())));
    }
    let (language, text) = decode(&e.bytes)?;
    Ok(Some(Document { language: Some(language), chars: DocChars::Text(text.chars()) }))
}

/// The payload of a FullText value of `text` in `language`.
pub fn encode(language: Language, text: &str) -> Vec<u8> {
    let name = language.name().as_bytes();
    let mut bytes = Vec::with_capacity(text.len().saturating_add(name.len()).saturating_add(2));
    bytes.push(VERSION);
    bytes.push(u8::try_from(name.len()).unwrap_or(u8::MAX));
    bytes.extend_from_slice(name);
    bytes.extend_from_slice(text.as_bytes());
    bytes
}

/// The language and text of a FullText payload.
///
/// # Errors
///
/// SQLCODE -916 if it is malformed.
pub fn decode(bytes: &[u8]) -> Result<(Language, &str)> {
    let mut r = Reader::new(bytes, "FullText value");
    if r.u8()? != VERSION {
        return Err(r.corrupt("unknown FullText version"));
    }
    let length = usize::from(r.u8()?);
    let name = std::str::from_utf8(r.take(length)?).map_err(|_| r.corrupt("a language name not in UTF-8"))?;
    let language = Language::named(name).map_err(|_| r.corrupt("a language the tokenizer does not know"))?;
    let text = std::str::from_utf8(r.rest()).map_err(|_| r.corrupt("text not in UTF-8"))?;
    Ok((language, text))
}

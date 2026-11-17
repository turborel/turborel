//! The public types and routines of full text [ISO/IEC 13249-2:2002 (CD,
//! SC32 N0640) §4.6.1, §12.1]: FullText (`types/fulltext.rs`) and
//! FT_Pattern (`types/ft_pattern.rs`), types beyond SQL's own through its
//! one type contract (RULES.md D53); the FullText methods and
//! FullText_to_Character (`types/routines.rs`); and Contains and Rank
//! (`types/search.rs`). The routines are scalar functions called as
//! `f(SELF, ...)`. All are built in, as the full-text index method is
//! (RULES.md D87), so a database knows them without registration.
//!
//! A FullText value is kept inline, as every extension type's value is.
//! The `SqlType` contract hands a value's whole bytes to a record and a
//! key, and a table keeps large objects out of line only for columns of a
//! large object type, so FT_MaxTextLength is the longest character string.

mod ft_pattern;
mod fulltext;
mod routines;
mod search;
mod tokenizing;

use std::sync::Arc;

pub use ft_pattern::{FT_PATTERN, FT_PATTERN_TYPE, FtPatternType, ft_pattern};
pub use fulltext::{FULLTEXT_TYPE, FullTextType, full_text};
pub use routines::{FULLTEXT_TO_CHARACTER, FullTextRoutine, LANGUAGE, ROUTINES};
pub use search::{Contains, Rank};

/// The text of a pattern: a character string's, or an FT_Pattern value's.
pub(crate) fn pattern_of(v: &Value) -> Option<&str> {
    if let Value::Char(text) = v { Some(text.as_str()) } else { ft_pattern::pattern_text(v) }
}

use crate::bytes::Reader;
use crate::error::{Code, Result, SqlError, SqlState};
use crate::fulltext::Language;
use crate::functions::ScalarFunction;
use crate::value::types::{ExtensionType, TypeParams};
use crate::value::{Binary, DataType, MAX_CHARACTER_LENGTH, SqlType, Value};

// ---------------------------------------------------------------------------
// Implementation-defined values [ISO/IEC 13249-2:2002 §5.1.1 DR2-DR5, §5.3.1
// DR1]
// ---------------------------------------------------------------------------

/// FT_MaxTextLength [§5.1.1 DR2], implementation-defined: the longest
/// character string, in characters, as the value is kept inline.
pub const FT_MAX_TEXT_LENGTH: u32 = MAX_CHARACTER_LENGTH;

/// FT_MaxPatternLength [§5.3.1 DR1], implementation-dependent: the longest
/// character string, in characters, so every CHARACTER VARYING a
/// Contains(CHARACTER VARYING) is given casts to FT_Pattern [§5.1.2].
pub const FT_MAX_PATTERN_LENGTH: u32 = MAX_CHARACTER_LENGTH;

/// FT_MaxLanguageLength [§5.1.1 DR4], implementation-defined: a
/// language's name, as LANGUAGE gives it, is at most this long.
pub const FT_MAX_LANGUAGE_LENGTH: u32 = 32;

/// FT_DefaultLanguage [§5.1.1 DR5], implementation-defined: ENGLISH.
pub const FT_DEFAULT_LANGUAGE: Language = Language::English;

/// The full-text types, built in as the full-text index is (RULES.md D87):
/// FULLTEXT and FT_PATTERN.
pub fn types() -> [&'static &'static dyn SqlType; 2] {
    [fulltext::kind(), ft_pattern::kind()]
}

/// The functions of full text, built in: the FullText routines, those
/// that tokenize, Contains and Rank.
pub fn functions() -> Vec<Arc<dyn ScalarFunction>> {
    let mut out: Vec<Arc<dyn ScalarFunction>> =
        Vec::with_capacity(ROUTINES.len().saturating_add(SEARCHES).saturating_add(tokenizing::TOKENIZING.len()));
    for routine in ROUTINES {
        out.push(Arc::new(routine));
    }
    for routine in tokenizing::TOKENIZING {
        out.push(Arc::new(routine));
    }
    out.push(Arc::new(Contains));
    out.push(Arc::new(Rank));
    out
}

/// The search functions, Contains and Rank.
const SEARCHES: usize = 2;

/// The type of `kind`, which has no parameters.
fn type_of(kind: &'static &'static dyn SqlType) -> DataType {
    DataType::Extension(ExtensionType { kind, params: TypeParams::default() })
}

/// The bytes of `v` if it is a value of the kind named `name`.
fn payload<'v>(v: &'v Value, name: &str) -> Option<&'v [u8]> {
    let Value::Extension(e) = v else { return None };
    (e.kind.name() == name).then_some(e.bytes.as_ref())
}

/// Whether `t` is the extension type named `name`.
fn is_type(t: &DataType, name: &str) -> bool {
    if let DataType::Extension(e) = t { e.kind.name() == name } else { false }
}

/// The error of a value not of the kind named `name`.
fn mismatch(v: &Value, name: &str) -> SqlError {
    SqlError::new(Code::Api, format!("[DESIGN] {v} is not a {name} value"))
}

/// The error of `text`, longer than `most` characters [ISO/IEC 9075:1992
/// §6.10 GR5c ii].
fn too_long(what: &str, most: u32) -> SqlError {
    SqlError::new(Code::Data, format!("[ISO/IEC 9075:1992 §6.10 GR5c ii] a {what} is longer than {most} characters"))
        .with_state(SqlState::STRING_DATA_RIGHT_TRUNCATION)
}

/// Whether `text` has at most `most` characters.
fn fits(text: &str, most: u32) -> bool {
    let most = usize::try_from(most).unwrap_or(usize::MAX);
    text.len() <= most || text.chars().count() <= most
}

/// The key of `bytes`: compared as unsigned bytes, as `bytes` compare,
/// and no other's prefix.
fn put_key(bytes: &[u8], out: &mut Vec<u8>) {
    Binary::new(bytes.to_vec()).put_key(out);
}

/// The bytes of the key `rest` begins with, of a type named `name`.
fn take_key(rest: &mut &[u8], name: &str) -> Result<Binary> {
    Binary::take_key(rest).ok_or_else(|| SqlError::new(Code::Corrupt, format!("[DESIGN] an invalid {name} index key")))
}

/// The bytes `put_value` wrote, as `r` reads them, checked by `check`.
fn read_payload<'r>(r: &mut Reader<'r>, check: impl Fn(&[u8]) -> bool, name: &str) -> Result<&'r [u8]> {
    let raw = r.bytes()?;
    if check(raw) { Ok(raw) } else { Err(r.corrupt(&format!("an invalid {name} value"))) }
}

// Unit tests of the types and routines (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;

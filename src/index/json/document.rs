//! \[DESIGN\] A document as a json index reads it (RULES.md D34): its hash,
//! the projection of the key an entry is counted under (D85), and its
//! terms; or, of one it cannot read, the term `UNREAD`, so every probe
//! finds it and the WHERE check raises of it as a scan would (D15).
//!
//! The hash is of the document's characters, read as they are taken,
//! trailing spaces dropped, so documents SQL's comparison finds equal,
//! 'x' and 'x ' with PAD SPACE, project alike, as equal keys must. It is
//! XXH3-128 (`hash/xxhash.rs`), chained over blocks, so a
//! large object is never held whole for it: stable across runs and
//! platforms, as the tree keeps it.
//!
//! A character string is parsed as JSON text, WITHOUT UNIQUE KEYS, as the
//! query operators parse it; a value of the JSON type gives its item
//! already parsed, and is hashed by its comparison key. `Document::read`
//! is the one path from a column's value to its items.

use std::collections::BTreeSet;

use super::keys::{DocHash, HASH_BYTES};
use super::terms::{Shape, Term, UNREAD, terms_of};
use crate::error::{Code, Result, SqlError};
use crate::functions::lobs::like::held_text;
use crate::hash::xxhash::xxh3_128_halves;
use crate::index::fulltext::chars_of;
use crate::index::{Key, KeyProjection};
use crate::json::{Item, Keys, parse};
use crate::value::lob::LobKind;
use crate::value::{Binary, Value};

/// \[DESIGN\] The seed of a document's hash; its bytes spell "JSD1".
const HASH_SEED: u64 = 0x4A53_4431;
/// \[DESIGN\] The bytes hashed into one block of the chain.
const HASH_BLOCK: usize = 4_096;
/// Bytes of a hash's half.
const HALF: usize = 8;
/// What a large object read whole as JSON text is, for its error.
const JSON_TEXT: &str = "JSON text of a json index";

/// The error of a key that is not one character string or projected hash.
fn not_text() -> SqlError {
    SqlError::new(Code::Api, "[DESIGN] a json index's key is one character string")
}

/// The one value of a key.
///
/// # Errors
///
/// SQLCODE -900 of a key not of one value.
pub(super) fn single(key: &Key) -> Result<&Value> {
    match key.0.as_slice() {
        [value] => Ok(value),
        _ => Err(not_text()),
    }
}

/// XXH3-128 of `bytes`, its halves little-endian, the low first.
fn digest(bytes: &[u8]) -> DocHash {
    let (low, high) = xxh3_128_halves(bytes, HASH_SEED);
    let mut out = [0_u8; HASH_BYTES];
    let (first, second) = out.split_at_mut(HALF);
    first.copy_from_slice(&low.to_le_bytes());
    second.copy_from_slice(&high.to_le_bytes());
    out
}

/// The hash of the document `value`, its trailing spaces dropped; none if
/// it is null.
///
/// # Errors
///
/// SQLCODE -900 of no character string; a large object's read errors.
pub(super) fn hash_of(value: &Value) -> Result<Option<DocHash>> {
    if let Value::Json(json) = value {
        return Ok(Some(chained(&json.key()?)));
    }
    let Some(chars) = chars_of(value)? else { return Ok(None) };
    let mut buffer = Vec::with_capacity(HASH_BLOCK.saturating_add(HASH_BYTES));
    buffer.extend_from_slice(&[0; HASH_BYTES]);
    let mut spaces = 0_usize;
    let mut utf8 = [0_u8; 4];
    for c in chars {
        let c = c?;
        if c == ' ' {
            spaces = spaces.saturating_add(1);
            continue;
        }
        buffer.extend(std::iter::repeat_n(b' ', spaces));
        spaces = 0;
        buffer.extend_from_slice(c.encode_utf8(&mut utf8).as_bytes());
        if buffer.len() >= HASH_BLOCK {
            let state = digest(&buffer);
            buffer.clear();
            buffer.extend_from_slice(&state);
        }
    }
    Ok(Some(digest(&buffer)))
}

/// The hash of a JSON value's comparison key, so values equal as SQL
/// compares them project alike; tagged, so no character string's hash is
/// sought from it.
fn chained(key: &[u8]) -> DocHash {
    let mut state = digest(b"JSON");
    for block in key.chunks(HASH_BLOCK) {
        let mut buffer = state.to_vec();
        buffer.extend_from_slice(block);
        state = digest(&buffer);
    }
    state
}

/// The hash a key names: of its document, or the hash itself if the key
/// is one already projected (`hash_key`), a binary string, which no
/// document is; none of a null document.
///
/// # Errors
///
/// As `hash_of`; SQLCODE -900 of a key not of one value.
pub(super) fn hash_in(key: &Key) -> Result<Option<DocHash>> {
    let value = single(key)?;
    if let Value::Binary(hash) = value {
        return DocHash::try_from(hash.bytes()).map(Some).map_err(|_| not_text());
    }
    hash_of(value)
}

/// The key an entry of a document of hash `hash` is counted under.
pub(super) fn hash_key(hash: &DocHash) -> Key {
    Key(vec![Value::Binary(Binary::of(hash))])
}

/// A document read: its hash and its terms, `UNREAD` alone of one that
/// is no JSON text, or is past a limit of turborel's.
pub(super) struct Document {
    /// Its hash.
    pub hash: DocHash,
    /// Its terms, each once.
    pub terms: BTreeSet<Term>,
}

impl Document {
    /// The document of `key`, read as `shape` says; none if it is null.
    ///
    /// # Errors
    ///
    /// As `hash_of`, of reading the key; a document that cannot be
    /// parsed is no error, but `UNREAD`.
    pub(super) fn read(key: &Key, shape: Shape) -> Result<Option<Document>> {
        let value = single(key)?;
        let Some(hash) = hash_of(value)? else { return Ok(None) };
        let terms = match items_of(value) {
            Ok(item) => terms_of(&item, shape),
            Err(_) => BTreeSet::from([UNREAD]),
        };
        Ok(Some(Document { hash, terms }))
    }
}

/// The items of a document that is not null: its JSON text parsed.
///
/// # Errors
///
/// Of JSON text that is none, or past a limit; of a large object longer
/// than one chunk; of a value that is no character string.
fn items_of(value: &Value) -> Result<Item> {
    match value {
        Value::Char(text) => parse(text, Keys::Any),
        Value::Lob(lob) => match lob.kind() {
            LobKind::Character => parse(&held_text(lob, JSON_TEXT)?, Keys::Any),
            LobKind::Binary => Err(not_text()),
        },
        Value::Json(json) => json.item(),
        Value::Null
        | Value::Exact(_)
        | Value::Approx(_)
        | Value::DecFloat(_)
        | Value::Binary(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Row(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Xml(_)
        | Value::Extension(_) => Err(not_text()),
    }
}

/// \[DESIGN\] The projection of an index's keys to its documents' hashes
/// (D85): fixed for every index, so a reader rechecks a fetched row by
/// projecting its document alike. A null document projects to null.
pub(super) struct Projection;

impl KeyProjection for Projection {
    fn project(&self, key: &Key) -> Result<Key> {
        Ok(match hash_in(key)? {
            Some(hash) => hash_key(&hash),
            None => Key(vec![Value::Null]),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(text: &str) -> Key {
        Key(vec![Value::Char(text.into())])
    }

    /// Verifies: [DESIGN] D85: documents equal by SQL's comparison, apart
    /// by trailing spaces, project alike; others apart; null to null.
    #[test]
    fn documents_equal_by_sql_project_alike() {
        let p = Projection;
        assert_eq!(p.project(&key(r#"{"a":1}"#)).unwrap(), p.project(&key(r#"{"a":1}   "#)).unwrap());
        assert_ne!(p.project(&key(r#"{"a":1}"#)).unwrap(), p.project(&key(r#"{"a": 1}"#)).unwrap());
        assert!(p.project(&Key(vec![Value::Null])).unwrap().0.iter().all(Value::is_null));
        let projected = p.project(&key("[1]")).unwrap();
        assert_eq!(p.project(&projected).unwrap(), projected, "a projected key projects to itself");
    }

    /// Verifies: [DESIGN] D15: a document that is no JSON text posts
    /// `UNREAD` alone, so every probe finds it.
    #[test]
    fn a_document_not_json_is_unread() {
        let shape = Shape { paths_only: false, max_depth: 128 };
        let read = Document::read(&key("{"), shape).unwrap().unwrap();
        assert_eq!(read.terms, BTreeSet::from([UNREAD]));
        let deep = format!("{}{}", "[".repeat(200), "]".repeat(200));
        assert_eq!(Document::read(&key(&deep), shape).unwrap().unwrap().terms, BTreeSet::from([UNREAD]), "past the nesting limit");
        assert!(Document::read(&Key(vec![Value::Null]), shape).unwrap().is_none());
    }
}

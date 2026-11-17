//! \[DESIGN\] A document as the full-text index reads it (RULES.md D86):
//! its characters, from a character string or a character large object,
//! read as they are taken; the terms its words are kept under; and its
//! hash, the projection of the key an entry is counted under (D85).
//!
//! The hash is of the document's tokens, each word with its places and
//! whether it is a stop word, not of its characters: two documents whose
//! tokens are alike have the same postings, so versions of a row that
//! differ only in what the tokenizer drops, as trailing spaces, or case
//! once words are normalized, are one document and project alike, as
//! equal keys must [ISO/IEC 13249-2:2002 (CD, SC32 N0640) §4.5]. Hashing
//! is MurmurHash3 x64 128-bit (`hash/murmur.rs`), chained over blocks so a
//! document of any length is never held whole: stable across runs and
//! platforms, as the tree keeps it.

use std::sync::Arc;

use super::keys::{DocHash, HASH_BYTES};
use crate::error::{Code, Result, SqlError};
use crate::fulltext::text::{StopWords, Tokens, tokenize_with};
use crate::fulltext::{Language, Token};
use crate::hash::murmur::murmur3_x64_128;
use crate::index::{Key, KeyProjection};
use crate::value::lob::{LobChars, LobKind};
use crate::value::{Binary, Value};

/// \[DESIGN\] The seed of the document hash: fixed, so the hash is the
/// same in every run. Its bytes spell "FTX1".
const HASH_SEED: u32 = 0x4654_5831;
/// \[DESIGN\] The bytes of tokens hashed into one block of the chain.
const HASH_BLOCK: usize = 4_096;
/// Bytes of a hash's half.
const HALF: usize = 8;

/// The characters of a document, read as they are taken.
pub(crate) enum DocChars<'v> {
    /// Of a character string held.
    Text(std::str::Chars<'v>),
    /// Of a character large object, a chunk at a time.
    Lob(Box<LobChars>),
}

impl Iterator for DocChars<'_> {
    type Item = Result<char>;

    fn next(&mut self) -> Option<Result<char>> {
        match self {
            DocChars::Text(chars) => chars.next().map(Ok),
            DocChars::Lob(chars) => chars.next_char().transpose(),
        }
    }
}

/// The error of a value that is not a character string.
fn not_text() -> SqlError {
    SqlError::new(Code::Api, "[DESIGN] a full-text index or TEXT_MATCHES was given a value that is not a character string")
}

/// The characters of `value`, or none if it is null.
///
/// # Errors
///
/// SQLCODE -900 if it is not a character string; a large object's read
/// errors.
pub(crate) fn chars_of(value: &Value) -> Result<Option<DocChars<'_>>> {
    if value.is_null() {
        return Ok(None);
    }
    if let Value::Char(text) = value {
        return Ok(Some(DocChars::Text(text.chars())));
    }
    let Value::Lob(lob) = value else { return Err(not_text()) };
    match lob.kind() {
        LobKind::Character => Ok(Some(DocChars::Lob(Box::new(LobChars::new(lob, 0)?)))),
        LobKind::Binary => Err(not_text()),
    }
}

/// The one value of a full-text index's key.
///
/// # Errors
///
/// SQLCODE -900 if the key is not of one value.
pub(super) fn single(key: &Key) -> Result<&Value> {
    match key.0.as_slice() {
        [value] => Ok(value),
        _ => Err(SqlError::new(Code::Api, "[DESIGN] a full-text index key is one character string")),
    }
}

/// The bytes of `word` a term keeps: up to the first U+0000, which a key
/// uses to end it (`keys.rs`), and at most `most` bytes, cut where a
/// character begins. A longer word shares its term with every word
/// beginning alike, so a search for it finds them too, as candidates the
/// function's check rejects (RULES.md D34).
pub(super) fn term_of(word: &str, most: usize) -> &[u8] {
    let word = word.split('\0').next().unwrap_or_default();
    let mut cut = 0_usize;
    for (at, c) in word.char_indices() {
        let end = at.saturating_add(c.len_utf8());
        if end > most {
            break;
        }
        cut = end;
    }
    word.as_bytes().get(..cut).unwrap_or_default()
}

/// The hash of a document's tokens, made as they are read.
pub(super) struct Hasher {
    /// The chain's state, then the tokens hashed since.
    buffer: Vec<u8>,
}

impl Hasher {
    /// A hash of no tokens yet.
    pub fn new() -> Hasher {
        let mut buffer = Vec::with_capacity(HASH_BLOCK.saturating_add(HASH_BYTES));
        buffer.extend_from_slice(&[0; HASH_BYTES]);
        Hasher { buffer }
    }

    /// Adds `token`: its word, its places and whether it is a stop word.
    pub fn add(&mut self, token: &Token) {
        let word = token.word.as_bytes();
        self.buffer.extend_from_slice(&u32::try_from(word.len()).unwrap_or(u32::MAX).to_le_bytes());
        self.buffer.extend_from_slice(word);
        for place in [token.ordinal, token.sentence, token.paragraph, token.offset] {
            self.buffer.extend_from_slice(&place.to_le_bytes());
        }
        self.buffer.push(u8::from(token.stop));
        if self.buffer.len() >= HASH_BLOCK {
            let state = digest(&self.buffer);
            self.buffer.clear();
            self.buffer.extend_from_slice(&state);
        }
    }

    /// The hash.
    pub fn finish(self) -> DocHash {
        digest(&self.buffer)
    }
}

/// MurmurHash3 x64 128-bit of `bytes`, its halves little-endian.
fn digest(bytes: &[u8]) -> DocHash {
    let (low, high) = murmur3_x64_128(bytes, HASH_SEED);
    let mut out = [0_u8; HASH_BYTES];
    let (first, second) = out.split_at_mut(HALF);
    first.copy_from_slice(&low.to_le_bytes());
    second.copy_from_slice(&high.to_le_bytes());
    out
}

/// The hash of the document `value` read as `reading` says, or none if
/// it is null.
///
/// # Errors
///
/// As `chars_of`; the tokenizer's.
pub(super) fn hash_of(value: &Value, reading: &Reading) -> Result<Option<DocHash>> {
    let Some(chars) = chars_of(value)? else { return Ok(None) };
    let mut hasher = Hasher::new();
    for token in reading.tokens(chars) {
        hasher.add(&token?);
    }
    Ok(Some(hasher.finish()))
}

/// The key an entry of a document of hash `hash` is counted under.
pub(super) fn hash_key(hash: &DocHash) -> Key {
    Key(vec![Value::Binary(Binary::new(hash.to_vec()))])
}

/// The hash a key names: of its document, or the hash itself if the key
/// is one already projected (`hash_key`), a binary string, which no
/// document is; none for a null document.
///
/// # Errors
///
/// As `hash_of`; SQLCODE -900 if the key is not of one value.
pub(super) fn hash_in(key: &Key, reading: &Reading) -> Result<Option<DocHash>> {
    let value = single(key)?;
    if let Value::Binary(hash) = value {
        return DocHash::try_from(hash.bytes()).map(Some).map_err(|_| not_text());
    }
    hash_of(value, reading)
}

/// How an index reads its documents: in its language, its stop words
/// marked by its set (`FULLTEXT.STOP_WORDS`). Both are fixed for the
/// index's life, as its postings and hashes depend on them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Reading {
    /// The language it reads in.
    pub language: Language,
    /// Its stop words.
    pub stop: StopWords,
}

impl Reading {
    /// The tokens of `chars`, read so.
    pub fn tokens<I: Iterator<Item = Result<char>>>(&self, chars: I) -> Tokens<I> {
        tokenize_with(chars, self.language, &self.stop)
    }

    /// Whether the normalized word `word` is a stop word, as read so.
    pub fn is_stop(&self, word: &str) -> bool {
        self.stop.contains(self.language, word)
    }
}

/// \[DESIGN\] The projection of an index's keys to its documents' hashes
/// (D85): fixed for the index's life by how it reads its documents, so a
/// reader rechecks a fetched row by projecting its document alike. A null
/// document projects to null.
pub(super) struct Projection {
    /// How the index reads its documents.
    pub reading: Reading,
}

impl Projection {
    /// The projection of documents read as `reading` says, shared.
    pub fn shared(reading: Reading) -> Arc<Projection> {
        Arc::new(Projection { reading })
    }
}

impl KeyProjection for Projection {
    fn project(&self, key: &Key) -> Result<Key> {
        let value = single(key)?;
        Ok(match hash_of(value, &self.reading)? {
            Some(hash) => hash_key(&hash),
            None => Key(vec![Value::Null]),
        })
    }
}

//! \[DESIGN\] The layout of a full-text index's tree (RULES.md D86): every
//! key begins with a tag byte naming what it records, so the kinds of
//! entry share one B-link tree and each kind's entries are adjacent.
//!
//! * `C`: the documents indexed, their lengths summed, and the next
//!   document number to give.
//! * `L` language name: nothing; a language of a document indexed other
//!   than the index's, kept once one is (`languages.rs`).
//! * `D` row, document hash: the versions of the row holding the document,
//!   its number, and its length in words that are not stop words.
//! * `H` document hash, row: nothing; it finds the rows of a document.
//! * `N` document number: the row and document hash it numbers, and the
//!   document's length, so ranking reads one entry.
//! * `P` term, 0, document number: a block of the term's postings in up to
//!   `MOST_BLOCK_DOCUMENTS` documents from that number on (`block.rs`).
//! * `Q` term, 0, document number, chunk: places of the term in one
//!   document too many for its block.
//! * `S` term, 0: the documents holding the term, and the most times it
//!   occurs in one.
//! * `W` stem, 0, term, 0: the documents holding the term, found by its
//!   stem.
//!
//! A term never holds the byte 0 (`document.rs` cuts it there), so the 0
//! after it ends it, and no key is a prefix of another of its kind: their
//! bytes sort as the tree sorts their entries, and a bulk load's records
//! do too (`load.rs`). Numbers in keys are big-endian, so rows and
//! document numbers sort in order; in values, little-endian, as
//! `crate::bytes` writes them, or variable-length.

use crate::bytes::{Put, Reader};
use crate::error::{Code, Result, SqlError};
use crate::storage::RowId;

/// The tag of a block of a term's postings.
pub(super) const POSTING: u8 = b'P';
/// The tag of a term's places in one document, past its block.
pub(super) const OVERFLOW: u8 = b'Q';
/// The tag of a document number.
pub(super) const NUMBER: u8 = b'N';
/// The tag of a document of a row.
pub(super) const DOCUMENT: u8 = b'D';
/// The tag of a row found by its document's hash.
pub(super) const HASHED: u8 = b'H';
/// The tag of a term's statistics.
pub(super) const STATISTIC: u8 = b'S';
/// The tag of a term found by its stem.
pub(super) const STEM: u8 = b'W';
/// The tag of a language of the index's documents.
pub(super) const LANGUAGE: u8 = b'L';
/// The key of the index's totals.
pub(super) const COUNTS_KEY: [u8; 1] = *b"C";
/// The key of a language of the index's documents, by its name.
pub(super) fn language_key(name: &str) -> Vec<u8> {
    [&[LANGUAGE][..], name.as_bytes()].concat()
}

/// The byte that ends a term or a stem in a key.
const END: u8 = 0;

/// Bytes of a document's hash (`document.rs`).
pub(super) const HASH_BYTES: usize = 16;
/// Bytes of a row id in a key.
const ROW_BYTES: usize = 8;
/// Bytes of a chunk's number in a key.
const CHUNK_BYTES: usize = 2;
/// Bytes of a document number.
pub(super) const DOCNO_BYTES: usize = 8;
/// Bytes of a block key besides its term: the tag, the 0 ending the term
/// and the first document number.
pub(super) const BLOCK_OVERHEAD: usize = 2 + DOCNO_BYTES;
/// Bytes of an overflow key besides its term: a block key's and a chunk's
/// number.
pub(super) const OVERFLOW_OVERHEAD: usize = BLOCK_OVERHEAD + CHUNK_BYTES;
/// Bytes of a document key, and of a hashed key: the tag, a row and a hash.
pub(super) const DOCUMENT_KEY: usize = 1 + ROW_BYTES + HASH_BYTES;
/// Bytes of a stem key besides its stem and term: the tag and two ends.
pub(super) const STEM_OVERHEAD: usize = 3;

/// A document's hash.
pub(super) type DocHash = [u8; HASH_BYTES];

/// One document of one row: what a posting, a document entry and a search
/// result name. Ordered by row, then hash, as their keys sort.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct Doc {
    /// The row.
    pub row: RowId,
    /// The hash of its document.
    pub hash: DocHash,
}

/// The error of an entry not as this layout writes it.
pub(super) fn corrupt() -> SqlError {
    SqlError::new(Code::Corrupt, "[DESIGN] a full-text index entry is malformed")
}

/// The bytes a term's key begins with: the tag, the term, its end.
fn term_prefix(tag: u8, term: &[u8], more: usize) -> Vec<u8> {
    let mut key = Vec::with_capacity(term.len().saturating_add(more).saturating_add(2));
    key.push(tag);
    key.extend_from_slice(term);
    key.push(END);
    key
}

/// The key of `term`'s block from document number `docno` on.
pub(super) fn block_key(term: &[u8], docno: u64) -> Vec<u8> {
    let mut key = term_prefix(POSTING, term, DOCNO_BYTES);
    key.extend_from_slice(&docno.to_be_bytes());
    key
}

/// The bounds of every block key of `term`: from the term and its end, to
/// below the byte after the end.
pub(super) fn block_range(term: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let lower = term_prefix(POSTING, term, 0);
    let mut upper = lower.clone();
    if let Some(last) = upper.last_mut() {
        *last = END.saturating_add(1);
    }
    (lower, upper)
}

/// The first document number of the block key `key` of `term`.
///
/// # Errors
///
/// SQLCODE -916 if `key` is not one.
pub(super) fn block_docno(key: &[u8], term: &[u8]) -> Result<u64> {
    let number = key.get(term.len().saturating_add(2)..).ok_or_else(corrupt)?;
    Ok(u64::from_be_bytes(number.try_into().map_err(|_| corrupt())?))
}

/// The key of chunk `chunk` of `term`'s places past its block in the
/// document numbered `docno`.
pub(super) fn overflow_key(term: &[u8], docno: u64, chunk: u16) -> Vec<u8> {
    let mut key = term_prefix(OVERFLOW, term, DOCNO_BYTES.saturating_add(CHUNK_BYTES));
    key.extend_from_slice(&docno.to_be_bytes());
    key.extend_from_slice(&chunk.to_be_bytes());
    key
}

/// The key of the document numbered `docno`.
pub(super) fn number_key(docno: u64) -> Vec<u8> {
    let mut key = vec![NUMBER];
    key.extend_from_slice(&docno.to_be_bytes());
    key
}

/// The value of a document number's entry: the document's row and hash.
pub(super) fn number_value(doc: Doc, length: u32) -> Vec<u8> {
    let mut out = doc.row.to_be_bytes().to_vec();
    out.extend_from_slice(&doc.hash);
    put_varint(&mut out, u64::from(length));
    out
}

/// The document a document number's entry names.
///
/// # Errors
///
/// SQLCODE -916 if `value` is not one.
pub(super) fn number_doc(value: &[u8]) -> Result<(Doc, u32)> {
    let (row, rest) = value.split_at_checked(ROW_BYTES).ok_or_else(corrupt)?;
    let (hash, rest) = rest.split_at_checked(HASH_BYTES).ok_or_else(corrupt)?;
    let (length, rest) = take_varint(rest)?;
    if !rest.is_empty() {
        return Err(corrupt());
    }
    let row = RowId::from_be_bytes(row.try_into().map_err(|_| corrupt())?);
    let hash = DocHash::try_from(hash).map_err(|_| corrupt())?;
    Ok((Doc { row, hash }, u32::try_from(length).map_err(|_| corrupt())?))
}

/// The key of `doc`'s document entry.
pub(super) fn document_key(doc: Doc) -> Vec<u8> {
    let mut key = Vec::with_capacity(DOCUMENT_KEY);
    key.push(DOCUMENT);
    key.extend_from_slice(&doc.row.to_be_bytes());
    key.extend_from_slice(&doc.hash);
    key
}

/// The document a document key names.
///
/// # Errors
///
/// SQLCODE -916 if `key` is not one.
pub(super) fn document_of(key: &[u8]) -> Result<Doc> {
    let mut reader = Reader::new(key, "full-text document key");
    if reader.u8()? != DOCUMENT {
        return Err(corrupt());
    }
    let row = RowId::from_be_bytes(reader.take(ROW_BYTES)?.try_into().map_err(|_| corrupt())?);
    let hash = DocHash::try_from(reader.take(HASH_BYTES)?).map_err(|_| corrupt())?;
    reader.finish()?;
    Ok(Doc { row, hash })
}

/// The key finding `doc`'s row by its hash.
pub(super) fn hashed_key(doc: Doc) -> Vec<u8> {
    let mut key = Vec::with_capacity(DOCUMENT_KEY);
    key.push(HASHED);
    key.extend_from_slice(&doc.hash);
    key.extend_from_slice(&doc.row.to_be_bytes());
    key
}

/// The row a hashed key names.
///
/// # Errors
///
/// SQLCODE -916 if `key` is not one.
pub(super) fn hashed_row(key: &[u8]) -> Result<RowId> {
    let row = key.get(1_usize.saturating_add(HASH_BYTES)..).ok_or_else(corrupt)?;
    Ok(RowId::from_be_bytes(row.try_into().map_err(|_| corrupt())?))
}

/// The bounds of the hashed keys of `hash`: the tag and hash, to below the
/// hash one greater, which is past them unless every byte is 0xFF, when
/// the next tag bounds them.
pub(super) fn hashed_range(hash: &DocHash) -> (Vec<u8>, Vec<u8>) {
    let mut lower = vec![HASHED];
    lower.extend_from_slice(hash);
    let mut upper = lower.clone();
    upper.extend_from_slice(&[u8::MAX; ROW_BYTES]);
    upper.push(u8::MAX);
    (lower, upper)
}

/// The key of `term`'s statistics.
pub(super) fn statistic_key(term: &[u8]) -> Vec<u8> {
    term_prefix(STATISTIC, term, 0)
}

/// The key of `term` under its stem `stem`.
pub(super) fn stem_key(stem: &[u8], term: &[u8]) -> Vec<u8> {
    let mut key = Vec::with_capacity(stem.len().saturating_add(term.len()).saturating_add(STEM_OVERHEAD));
    key.push(STEM);
    key.extend_from_slice(stem);
    key.push(END);
    key.extend_from_slice(term);
    key.push(END);
    key
}

/// The bounds of every key of a tag: from it to below the next.
pub(super) fn tag_range(tag: u8) -> (Vec<u8>, Vec<u8>) {
    (vec![tag], vec![tag.saturating_add(1)])
}

/// A document entry's value: the versions holding it, its number, and
/// its length in words that are not stop words, a variable-length
/// integer each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DocumentValue {
    /// The versions of the row holding the document (`Index`'s counts).
    pub count: u32,
    /// Its document number.
    pub docno: u64,
    /// Its words that are not stop words.
    pub length: u32,
}

impl DocumentValue {
    /// Its bytes.
    pub fn encode(self) -> Vec<u8> {
        let mut out = Vec::with_capacity(VARINT_MOST);
        put_varint(&mut out, u64::from(self.count));
        put_varint(&mut out, self.docno);
        put_varint(&mut out, u64::from(self.length));
        out
    }

    /// The value `bytes` hold.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if they are not one.
    pub fn decode(bytes: &[u8]) -> Result<DocumentValue> {
        let (count, rest) = take_varint(bytes)?;
        let (docno, rest) = take_varint(rest)?;
        let (length, rest) = take_varint(rest)?;
        if !rest.is_empty() {
            return Err(corrupt());
        }
        let narrow = |n: u64| u32::try_from(n).map_err(|_| corrupt());
        Ok(DocumentValue { count: narrow(count)?, docno, length: narrow(length)? })
    }
}

/// A term's statistics: the documents holding it, and the most times it
/// occurs in one. The most is an upper bound: a remove does not lower it,
/// as finding the next greatest would read every posting of the term.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TermStatistics {
    /// The documents holding the term: its document frequency.
    pub documents: u64,
    /// At least the most times it occurs in one document.
    pub most: u32,
}

impl TermStatistics {
    /// Its bytes.
    pub(super) fn encode(self) -> Vec<u8> {
        let mut out = Vec::new();
        out.put_u64(self.documents);
        out.put_u32(self.most);
        out
    }

    /// The statistics `bytes` hold.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if they are not some.
    pub(super) fn decode(bytes: &[u8]) -> Result<TermStatistics> {
        let mut reader = Reader::new(bytes, "full-text term statistics");
        let value = TermStatistics { documents: reader.u64()?, most: reader.u32()? };
        reader.finish()?;
        Ok(value)
    }
}

/// The index's totals: its documents and their lengths summed, for the
/// average length ranking weighs a document's against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Totals {
    /// The documents indexed, one for each row and document.
    pub documents: u64,
    /// Their lengths in words that are not stop words, summed.
    pub length: u64,
}

/// The totals entry's value: the totals, and the next document number to
/// give, which is never given again, so the entry stays once made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct Counts {
    /// The totals.
    pub totals: Totals,
    /// The next document number.
    pub next: u64,
}

impl Counts {
    /// Its bytes.
    pub(super) fn encode(self) -> Vec<u8> {
        let mut out = Vec::new();
        out.put_u64(self.totals.documents);
        out.put_u64(self.totals.length);
        out.put_u64(self.next);
        out
    }

    /// The counts `bytes` hold.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if they are not some.
    pub(super) fn decode(bytes: &[u8]) -> Result<Counts> {
        let mut reader = Reader::new(bytes, "full-text totals");
        let totals = Totals { documents: reader.u64()?, length: reader.u64()? };
        let value = Counts { totals, next: reader.u64()? };
        reader.finish()?;
        Ok(value)
    }
}

/// A count's bytes, a stem key's value.
pub(super) fn count_value(count: u64) -> Vec<u8> {
    count.to_le_bytes().to_vec()
}

/// The count `bytes` hold.
///
/// # Errors
///
/// SQLCODE -916 if they are not one.
pub(super) fn count_of(bytes: &[u8]) -> Result<u64> {
    <[u8; ROW_BYTES]>::try_from(bytes).map(u64::from_le_bytes).map_err(|_| corrupt())
}

/// Appends `value` to `out` as a variable-length integer: seven bits a
/// byte, low first, the high bit set on every byte but the last.
pub(super) fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= VARINT_HIGH {
        out.push(u8::try_from(value & VARINT_MASK).unwrap_or_default() | VARINT_MORE);
        value >>= VARINT_SHIFT;
    }
    out.push(u8::try_from(value).unwrap_or_default());
}

/// The bits a varint byte carries.
const VARINT_SHIFT: u32 = 7;
/// The flag of a varint byte with more after it.
const VARINT_MORE: u8 = 0x80;
/// The bits of a varint byte's value.
const VARINT_MASK: u64 = 0x7F;
/// The least value taking more than one varint byte.
const VARINT_HIGH: u64 = 0x80;
/// The most bytes a varint of a u64 takes.
pub(super) const VARINT_MOST: usize = 10;

/// The variable-length integer at the front of `bytes`, and the bytes
/// after it.
///
/// # Errors
///
/// SQLCODE -916 if it is cut short or longer than a u64's.
pub(super) fn take_varint(bytes: &[u8]) -> Result<(u64, &[u8])> {
    let mut value = 0_u64;
    for (at, &byte) in bytes.iter().enumerate().take(VARINT_MOST) {
        let shift = u32::try_from(at).map_err(|_| corrupt())?.saturating_mul(VARINT_SHIFT);
        value |= (u64::from(byte) & VARINT_MASK).checked_shl(shift).unwrap_or_default();
        if byte & VARINT_MORE == 0 {
            return Ok((value, bytes.get(at.saturating_add(1)..).unwrap_or_default()));
        }
    }
    Err(corrupt())
}

// Unit tests: the entries' layout written and read (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;

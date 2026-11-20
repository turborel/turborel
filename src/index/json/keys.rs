//! \[DESIGN\] The keys of a json index's tree (RULES.md D34), tagged by
//! their first byte:
//!
//! * `D`, the row and the document's hash: the versions holding it, a
//!   count of four bytes; one for each row and document, so versions of a
//!   row with one document post once;
//! * `H`, the hash and the row: nothing, so the rows of a document are
//!   found by its hash (`Index::rows`);
//! * `P`, a term, the hash and the row: nothing, a posting; a term's
//!   postings are in order of hash and row, so they merge and are looked
//!   up by one descent;
//! * `P`, a term, then `COUNT_TAIL`: the documents posting the term, eight
//!   bytes, after its postings, so a bulk load writes it as it passes
//!   them, and a scan of the postings stops before it.

use super::terms::{TERM_BYTES, Term};
use crate::error::{Code, Result, SqlError};
use crate::storage::RowId;

/// Bytes of a document's hash.
pub(super) const HASH_BYTES: usize = 16;
/// Bytes of a row id.
const ROW_BYTES: usize = 8;
/// Bytes of a hash and a row, what a posting points at.
pub(super) const DOC_BYTES: usize = HASH_BYTES + ROW_BYTES;
/// The tag of a document's key.
pub(super) const DOCUMENT: u8 = b'D';
/// The tag of a hash's key.
const HASHED: u8 = b'H';
/// The tag of a posting's or a count's key.
const POSTING: u8 = b'P';
/// What follows a term in its count's key: longer than a posting's tail,
/// and above it, so it sorts after every posting of the term.
const COUNT_TAIL: [u8; COUNT_TAIL_BYTES] = [0xFF; COUNT_TAIL_BYTES];
/// Bytes of `COUNT_TAIL`, one past a posting's tail.
const COUNT_TAIL_BYTES: usize = DOC_BYTES + 1;

/// A document's hash.
pub(super) type DocHash = [u8; HASH_BYTES];

/// What a posting points at: the document's hash, then its row, as their
/// bytes sort.
pub(super) type Doc = [u8; DOC_BYTES];

/// The error of a key or value not of this layout.
pub(super) fn corrupt() -> SqlError {
    SqlError::new(Code::Corrupt, "[DESIGN] a json index's entry is not of its layout")
}

/// The doc of `hash` and `row`.
pub(super) fn doc(hash: &DocHash, row: RowId) -> Doc {
    let mut out = [0_u8; DOC_BYTES];
    let (h, r) = out.split_at_mut(HASH_BYTES);
    h.copy_from_slice(hash);
    r.copy_from_slice(&row.to_be_bytes());
    out
}

/// The hash and row of `doc`.
pub(super) fn parts(doc: &Doc) -> (DocHash, RowId) {
    let (h, r) = doc.split_at(HASH_BYTES);
    let mut hash = [0_u8; HASH_BYTES];
    hash.copy_from_slice(h);
    let mut row = [0_u8; ROW_BYTES];
    row.copy_from_slice(r);
    (hash, RowId::from_be_bytes(row))
}

/// The key of the document `hash` of `row`.
pub(super) fn document_key(row: RowId, hash: &DocHash) -> Vec<u8> {
    let mut key = vec![DOCUMENT];
    key.extend_from_slice(&row.to_be_bytes());
    key.extend_from_slice(hash);
    key
}

/// The doc a document's key is of.
///
/// # Errors
///
/// SQLCODE -916 if it is not of the layout.
pub(super) fn document_of(key: &[u8]) -> Result<Doc> {
    let rest = key.get(1..).ok_or_else(corrupt)?;
    let (row, hash) = rest.split_at_checked(ROW_BYTES).ok_or_else(corrupt)?;
    let row = RowId::from_be_bytes(row.try_into().map_err(|_| corrupt())?);
    let hash: DocHash = hash.try_into().map_err(|_| corrupt())?;
    Ok(doc(&hash, row))
}

/// The key of a row of the document `hash`.
pub(super) fn hashed_key(hash: &DocHash, row: RowId) -> Vec<u8> {
    let mut key = vec![HASHED];
    key.extend_from_slice(&doc(hash, row));
    key
}

/// The keys from the first of hash `hash` to past its last.
pub(super) fn hashed_range(hash: &DocHash) -> (Vec<u8>, Vec<u8>) {
    let mut low = vec![HASHED];
    low.extend_from_slice(hash);
    let mut high = low.clone();
    high.extend_from_slice(&[0xFF; ROW_BYTES + 1]);
    (low, high)
}

/// The row a hash's key is of.
///
/// # Errors
///
/// SQLCODE -916 if it is not of the layout.
pub(super) fn hashed_row(key: &[u8]) -> Result<RowId> {
    let tail = key.get(key.len().saturating_sub(ROW_BYTES)..).ok_or_else(corrupt)?;
    Ok(RowId::from_be_bytes(tail.try_into().map_err(|_| corrupt())?))
}

/// The key of `term`'s posting of `doc`.
pub(super) fn posting_key(term: &Term, doc: &Doc) -> Vec<u8> {
    let mut key = Vec::with_capacity(1 + TERM_BYTES + DOC_BYTES);
    key.push(POSTING);
    key.extend_from_slice(term);
    key.extend_from_slice(doc);
    key
}

/// The doc a posting's key points at.
///
/// # Errors
///
/// SQLCODE -916 if it is not of the layout.
pub(super) fn posting_doc(key: &[u8]) -> Result<Doc> {
    key.get(1 + TERM_BYTES..).and_then(|tail| tail.try_into().ok()).ok_or_else(corrupt)
}

/// The keys of `term`'s postings: from its first, at `from` if given, to
/// its last, inclusive, its count past them.
pub(super) fn posting_range(term: &Term, from: Option<&Doc>) -> (Vec<u8>, Vec<u8>) {
    let low = posting_key(term, from.unwrap_or(&[0; DOC_BYTES]));
    let high = posting_key(term, &[0xFF; DOC_BYTES]);
    (low, high)
}

/// The key of `term`'s count.
pub(super) fn count_key(term: &Term) -> Vec<u8> {
    let mut key = Vec::with_capacity(1 + TERM_BYTES + COUNT_TAIL_BYTES);
    key.push(POSTING);
    key.extend_from_slice(term);
    key.extend_from_slice(&COUNT_TAIL);
    key
}

/// Whether `key` is a count's, and of which term.
#[cfg(test)]
pub(super) fn count_term(key: &[u8]) -> Option<Term> {
    let rest = key.strip_prefix(&[POSTING])?;
    let (term, tail) = rest.split_at_checked(TERM_BYTES)?;
    (tail == COUNT_TAIL).then(|| term.try_into().ok()).flatten()
}

/// The keys of every entry with `tag`.
pub(super) fn tag_range(tag: u8) -> (Vec<u8>, Vec<u8>) {
    (vec![tag], vec![tag.saturating_add(1)])
}

/// A count's value.
pub(super) fn count_value(n: u64) -> Vec<u8> {
    n.to_be_bytes().to_vec()
}

/// The count a value holds, of eight bytes, or of four, a document's.
///
/// # Errors
///
/// SQLCODE -916 of another length.
pub(super) fn count_of(value: &[u8]) -> Result<u64> {
    if let Ok(four) = <[u8; 4]>::try_from(value) {
        return Ok(u64::from(u32::from_be_bytes(four)));
    }
    let eight: [u8; 8] = value.try_into().map_err(|_| corrupt())?;
    Ok(u64::from_be_bytes(eight))
}

/// A document's count's value.
pub(super) fn versions_value(n: u32) -> Vec<u8> {
    n.to_be_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] a term's count sorts after each of its
    /// postings, and before the next term's; keys read back their parts.
    #[test]
    fn counts_sort_after_their_postings() {
        let (t, next) = ([1_u8; TERM_BYTES], [2_u8; TERM_BYTES]);
        let top = posting_key(&t, &[0xFF; DOC_BYTES]);
        assert!(count_key(&t) > top);
        assert!(count_key(&t) < posting_key(&next, &[0; DOC_BYTES]));
        assert_eq!(count_term(&count_key(&t)), Some(t));
        assert_eq!(count_term(&top), None);
        let d = doc(&[7; HASH_BYTES], 42);
        assert_eq!(parts(&d), ([7; HASH_BYTES], 42));
        assert_eq!(posting_doc(&posting_key(&t, &d)).unwrap(), d);
        assert_eq!(document_of(&document_key(42, &[7; HASH_BYTES])).unwrap(), d);
        assert_eq!(hashed_row(&hashed_key(&[7; HASH_BYTES], 42)).unwrap(), 42);
        assert_eq!(count_of(&count_value(9)).unwrap(), 9);
        assert_eq!(count_of(&versions_value(3)).unwrap(), 3);
    }

    /// Verifies: [DESIGN] a json index's entries, R-ERR-2: a document key,
    /// a hashed key, a posting key and a count cut short, or a count of
    /// a length that is neither four nor eight, are refused with SQLCODE
    /// -916.
    #[test]
    fn malformed_entries_are_refused() {
        let d = doc(&[7; HASH_BYTES], 42);
        let keys = [document_key(42, &[7; HASH_BYTES]), hashed_key(&[7; HASH_BYTES], 42), posting_key(&[1; TERM_BYTES], &d)];
        for cut in 0..keys[0].len() {
            assert_eq!(document_of(&keys[0][..cut]).unwrap_err().sqlcode(), -916_i32, "cut at {cut}");
        }
        for cut in 0..ROW_BYTES {
            assert_eq!(hashed_row(&keys[1][..cut]).unwrap_err().sqlcode(), -916_i32, "cut at {cut}");
        }
        for cut in 0..keys[2].len() {
            assert_eq!(posting_doc(&keys[2][..cut]).unwrap_err().sqlcode(), -916_i32, "cut at {cut}");
        }
        assert_eq!(document_of(&[keys[0].as_slice(), &[0]].concat()).unwrap_err().sqlcode(), -916_i32);
        for n in [0, 3, 5, 9] {
            assert_eq!(count_of(&vec![0; n]).unwrap_err().sqlcode(), -916_i32, "{n} bytes");
        }
    }
}

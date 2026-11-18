//! \[DESIGN\] A json index built whole (RULES.md D34, D31): each entry's
//! document read once, its keys sorted in a spill area
//! (`spill::sorted_records`), so no more of the index is held than an
//! area's bytes, and the tree loaded in key order (`BLinkTree::load`), as
//! inserting each would leave it. Keys alike, of versions of a row with
//! one document, are kept once, a document's counted; each term's count
//! is written after its postings as they pass, where its key sorts.

use std::iter::Peekable;

use super::JsonIndex;
use super::document::Document;
use super::keys::{DOCUMENT, count_key, count_value, doc, document_key, hashed_key, posting_key, versions_value};
use super::terms::{TERM_BYTES, Term};
use crate::error::Result;
use crate::index::Key;
use crate::spill::{Area, Records, WorkAreas, sorted_records};
use crate::storage::RowId;

/// The keys of one entry: its document's, its hash's and its postings';
/// none of a null document.
fn keys_of(entry: Result<(Key, RowId)>, index: &JsonIndex) -> Vec<Result<Vec<u8>>> {
    let read = entry.and_then(|(key, row)| Ok(Document::read(&key, index.shape)?.map(|d| (row, d))));
    match read {
        Ok(Some((row, document))) => {
            let at = doc(&document.hash, row);
            let mut keys = vec![Ok(document_key(row, &document.hash)), Ok(hashed_key(&document.hash, row))];
            keys.extend(document.terms.iter().map(|term| Ok(posting_key(term, &at))));
            keys
        }
        Ok(None) => Vec::new(),
        Err(e) => vec![Err(e)],
    }
}

/// The term a posting's key is of, if it is one.
fn term_of(key: &[u8]) -> Option<Term> {
    let (tag, rest) = key.split_first()?;
    if *tag == DOCUMENT {
        return None;
    }
    rest.get(..TERM_BYTES).and_then(|t| t.try_into().ok()).filter(|_| *tag == b'P')
}

/// The tree's entries of sorted keys: each key once, a document's with its
/// count, and each term's count after its postings.
struct Entries {
    keys: Peekable<Records>,
    /// The term whose postings are being passed, and how many so far.
    term: Option<(Term, u64)>,
    /// A key read past a term's count, to give next.
    pending: Option<Vec<u8>>,
}

impl Iterator for Entries {
    type Item = Result<(Vec<u8>, Vec<u8>)>;

    fn next(&mut self) -> Option<Self::Item> {
        let key = match self.pending.take().map(Ok).or_else(|| self.keys.next()) {
            Some(Ok(key)) => Some(key),
            Some(Err(e)) => return Some(Err(e)),
            None => None,
        };
        let term = key.as_deref().and_then(term_of);
        if let Some((done, n)) = self.term.filter(|(t, _)| Some(*t) != term) {
            // Its count, before the key read, which comes next.
            self.term = None;
            let count = (count_key(&done), count_value(n));
            self.pending = key;
            return Some(Ok(count));
        }
        let key = key?;
        let mut alike = 1_u32;
        while self.keys.next_if(|next| next.as_ref().is_ok_and(|n| *n == key)).is_some() {
            alike = alike.saturating_add(1);
        }
        if let Some(t) = term {
            let n = self.term.map_or(0, |(_, n)| n);
            self.term = Some((t, n.saturating_add(1)));
        }
        let value = if key.first() == Some(&DOCUMENT) { versions_value(alike) } else { Vec::new() };
        Some(Ok((key, value)))
    }
}

impl JsonIndex {
    /// `load`: every entry's keys sorted, then the tree loaded of them.
    ///
    /// # Errors
    ///
    /// As reading the entries and their documents; the spill area's; the
    /// tree's.
    pub(super) fn load_whole(
        &self,
        entries: &mut dyn Iterator<Item = Result<(Key, RowId)>>,
        (areas, area): (&WorkAreas, Area),
    ) -> Result<()> {
        let mut keys = entries.flat_map(|entry| keys_of(entry, self));
        let sorted = sorted_records(areas, area, &mut keys)?;
        self.tree.load(&mut Entries { keys: sorted.peekable(), term: None, pending: None })
    }
}

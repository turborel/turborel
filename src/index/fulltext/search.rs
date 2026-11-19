//! \[DESIGN\] A full-text index read (RULES.md D86, D34): the documents
//! holding every word of a search, found by intersecting each word's
//! cursor over its blocks (`cursor.rs`), each number found then mapped to
//! its row and document by its `N` entry; every document, by its entries;
//! and the rows of a document, by its hash.
//!
//! A search holds a block or two for each word, and what it gives holds
//! the index, so its pages are not freed while it is read.

use std::ops::{Bound, Deref};
use std::sync::Arc;

use super::FullTextIndex;
use super::cursor::{Cursor, Numbers, intersect};
use super::document::{hash_in, hash_key, term_of};
use super::keys::{
    COUNTS_KEY, Counts, DOCUMENT, Doc, DocumentValue, TermStatistics, Totals, document_key, document_of, hashed_range, hashed_row,
    number_doc, number_key, statistic_key, tag_range,
};
use super::near::stream;
use super::plan::{Plan, or};
use super::postings::Position;
use crate::engine::tree::{BLinkTree, Direction, Scan, scan_of};
use crate::error::{Code, Result, SqlError};
use crate::fulltext::matches::search_words;
use crate::fulltext::pattern::{CONTAINS, Pattern, contains as holds, parse};
use crate::index::{Entries, Key, Probe, RowIds};
use crate::storage::RowId;
use crate::value::Value;

/// An index's tree, reached through the index it holds: what a search
/// reads, so the index outlives the search.
pub(super) struct Held(pub Arc<FullTextIndex>);

impl Deref for Held {
    type Target = BLinkTree;
    fn deref(&self) -> &BLinkTree {
        &self.0.tree
    }
}

/// The scan of `index`'s keys from `lower` to below `upper`, holding it.
fn scan(index: &Arc<FullTextIndex>, (lower, upper): (Vec<u8>, Vec<u8>)) -> Scan<Held> {
    scan_of(Held(Arc::clone(index)), Bound::Included(&lower), Bound::Excluded(&upper), Direction::Forward)
}

/// The documents of `index`, each once, in row order.
fn documents(index: &Arc<FullTextIndex>) -> impl Iterator<Item = Result<Doc>> + Send + use<> {
    scan(index, tag_range(DOCUMENT)).map(|entry| document_of(&entry?.0))
}

/// The entry of `doc`: the key it is counted under, and its row.
fn entry(doc: Result<Doc>) -> Result<(Key, RowId)> {
    doc.map(|doc| (hash_key(&doc.hash), doc.row))
}

/// The error of a probe it does not answer.
fn refused() -> SqlError {
    SqlError::new(Code::Unsupported, "[EXTENSION] the fulltext index method answers TEXT_MATCHES and every entry, no comparison")
}

/// `search`: every document for `Probe::All`; for TEXT_MATCHES, those
/// holding every term of its words, every document if they have none
/// that is not a stop word, none if they are null.
pub(super) fn search(index: &Arc<FullTextIndex>, probe: &Probe) -> Entries {
    let words = match probe {
        Probe::All => return Box::new(documents(index).map(entry)),
        Probe::Predicate { predicate, value } => {
            if predicate.function.0.name() == CONTAINS {
                return contains(index, value);
            }
            value
        }
        Probe::Columns(_) | Probe::Compare { .. } | Probe::Prefix { .. } | Probe::Range { .. } | Probe::Xml(_) => {
            return Box::new(std::iter::once(Err(refused())));
        }
    };
    let terms = match index.terms_of(words) {
        Ok(Some(terms)) => terms,
        Ok(None) => return Box::new(std::iter::empty()),
        Err(e) => return Box::new(std::iter::once(Err(e))),
    };
    if terms.is_empty() {
        return Box::new(documents(index).map(entry));
    }
    let cursors = terms.into_iter().map(|term| Cursor::new(index, term)).collect();
    let held = Arc::clone(index);
    Box::new(intersect(cursors).filter_map(move |docno| held.numbered(docno).transpose()).map(entry))
}

/// The plan of CONTAINS's pattern `value` [ISO/IEC 13249-2:2002 (CD,
/// SC32 N0640) §5.1.2]: none for a null pattern; every document for one
/// that is not a character string or that Contains refuses, as one that
/// does not parse or of stop words only, so the recheck of the first row
/// gives the error, where a scan of the table would. A pattern's words
/// are of its document's language where they name none, so its plan is
/// the union of its plans in each language its documents are in.
fn pattern_plan(index: &FullTextIndex, value: &Value) -> Result<Option<Plan>> {
    if value.is_null() {
        return Ok(None);
    }
    let Some(text) = crate::fulltext::types::pattern_of(value) else { return Ok(Some(Plan::All)) };
    let mut patterns: Vec<Pattern> = Vec::new();
    for language in index.languages().iter() {
        let Ok(pattern) = parse(text, language) else { return Ok(Some(Plan::All)) };
        if holds(&pattern, std::iter::empty(), language).is_err() {
            return Ok(Some(Plan::All));
        }
        if !patterns.contains(&pattern) {
            patterns.push(pattern);
        }
    }
    let plans = patterns.iter().map(|pattern| index.plan(pattern)).collect::<Result<Vec<Plan>>>()?;
    Ok(Some(or(plans)))
}

/// `search` for CONTAINS of the pattern `value`: the documents its plan
/// finds, a superset of those it holds, as they are read.
fn contains(index: &Arc<FullTextIndex>, value: &Value) -> Entries {
    let plan = match pattern_plan(index, value) {
        Ok(Some(plan)) => plan,
        Ok(None) => return Box::new(std::iter::empty()),
        Err(e) => return Box::new(std::iter::once(Err(e))),
    };
    let held = Arc::clone(index);
    Box::new(Numbers::new(stream(index, plan)).filter_map(move |docno| held.numbered(docno).transpose()).map(entry))
}

/// `key_rows`: the rows of `key`'s document, read as they are taken.
pub(super) fn key_rows(index: &Arc<FullTextIndex>, key: &Key) -> RowIds {
    match hash_in(key, &index.reading) {
        Ok(Some(hash)) => Box::new(scan(index, hashed_range(&hash)).map(|entry| hashed_row(&entry?.0))),
        Ok(None) => Box::new(std::iter::empty()),
        Err(e) => Box::new(std::iter::once(Err(e))),
    }
}

impl FullTextIndex {
    /// The terms TEXT_MATCHES's words `words` are looked for under, each
    /// once: its words that are not stop words, of its language, English,
    /// or of the index's set, as the index posts none of the latter;
    /// none if they are null.
    ///
    /// # Errors
    ///
    /// As reading them.
    pub(super) fn terms_of(&self, words: &Value) -> Result<Option<Vec<Vec<u8>>>> {
        let Some(words) = search_words(words)? else { return Ok(None) };
        let mut terms: Vec<Vec<u8>> =
            words.iter().filter(|word| !self.reading.is_stop(word)).map(|word| term_of(word, self.most_term).to_vec()).collect();
        terms.sort_unstable();
        terms.dedup();
        Ok(Some(terms))
    }

    /// `rows`: the rows of `key`'s document, each once.
    ///
    /// # Errors
    ///
    /// As reading the document; the tree's.
    pub(super) fn rows_of(&self, key: &Key) -> Result<Vec<RowId>> {
        let Some(hash) = hash_in(key, &self.reading)? else { return Ok(Vec::new()) };
        let (lower, upper) = hashed_range(&hash);
        self.tree.scan(Bound::Included(&lower), Bound::Excluded(&upper), Direction::Forward).map(|entry| hashed_row(&entry?.0)).collect()
    }

    /// \[EXTENSION\] Its documents, and their lengths summed.
    ///
    /// # Errors
    ///
    /// The tree's; SQLCODE -916 if the entry is malformed.
    pub fn totals(&self) -> Result<Totals> {
        self.tree.get(&COUNTS_KEY)?.map_or(Ok(Totals::default()), |bytes| Counts::decode(&bytes).map(|c| c.totals))
    }

    /// \[EXTENSION\] The statistics of the normalized word `word`: of
    /// every word kept under its term.
    ///
    /// # Errors
    ///
    /// The tree's; SQLCODE -916 if the entry is malformed.
    pub fn statistics(&self, word: &str) -> Result<TermStatistics> {
        let key = statistic_key(term_of(word, self.most_term));
        self.tree.get(&key)?.map_or(Ok(TermStatistics::default()), |bytes| TermStatistics::decode(&bytes))
    }

    /// \[EXTENSION\] The postings of the normalized word `word` in the
    /// document of `row` that `key` names, the document or its projection
    /// (`hash_key`): the term's frequency there and its places in order;
    /// none if the document does not hold it.
    ///
    /// # Errors
    ///
    /// As reading the document; the tree's; SQLCODE -916 if a posting is
    /// malformed.
    pub fn postings(&self, word: &str, key: &Key, row: RowId) -> Result<Option<(u32, Vec<Position>)>> {
        let Some(hash) = hash_in(key, &self.reading)? else { return Ok(None) };
        let Some(value) = self.tree.get(&document_key(Doc { row, hash }))? else { return Ok(None) };
        let docno = DocumentValue::decode(&value)?.docno;
        let term = term_of(word, self.most_term);
        let Some(posting) = self.posting(term, docno)? else { return Ok(None) };
        let places = self.places(term, docno)?;
        Ok(Some((posting.frequency, places)))
    }

    /// The document numbered `docno`, if it is still indexed.
    ///
    /// # Errors
    ///
    /// The tree's; SQLCODE -916 if its entry is malformed.
    pub(super) fn numbered(&self, docno: Result<u64>) -> Result<Option<Doc>> {
        Ok(self.numbered_with_length(docno?)?.map(|(doc, _)| doc))
    }

    /// The document numbered `docno` and its length, if it is still
    /// indexed.
    ///
    /// # Errors
    ///
    /// As `numbered`.
    pub(super) fn numbered_with_length(&self, docno: u64) -> Result<Option<(Doc, u32)>> {
        self.tree.get(&number_key(docno))?.map(|value| number_doc(&value)).transpose()
    }

    /// `estimate`: every document for `Probe::All`, and for TEXT_MATCHES
    /// with no word but stop words; else the fewest documents of a term.
    pub(super) fn estimated(&self, probe: &Probe) -> Option<usize> {
        let documents = |stats: Result<u64>| stats.ok().map(|n| usize::try_from(n).unwrap_or(usize::MAX));
        let words = match probe {
            Probe::All => return documents(self.totals().map(|t| t.documents)),
            Probe::Predicate { predicate, value } => {
                if predicate.function.0.name() == CONTAINS {
                    let every = self.totals().ok()?.documents;
                    let Some(plan) = pattern_plan(self, value).ok()? else { return Some(0) };
                    return documents(self.plan_estimate(&plan, every));
                }
                value
            }
            Probe::Columns(_) | Probe::Compare { .. } | Probe::Prefix { .. } | Probe::Range { .. } | Probe::Xml(_) => return None,
        };
        let terms = self.terms_of(words).ok()?;
        let Some(terms) = terms else { return Some(0) };
        if terms.is_empty() {
            return documents(self.totals().map(|t| t.documents));
        }
        let mut fewest: Option<usize> = None;
        for term in &terms {
            let found = self.tree.get(&statistic_key(term)).ok()?.map_or(Ok(TermStatistics::default()), |b| TermStatistics::decode(&b));
            let count = documents(found.map(|s| s.documents))?;
            fewest = Some(fewest.map_or(count, |f| f.min(count)));
        }
        fewest
    }
}

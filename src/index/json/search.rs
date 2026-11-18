//! \[DESIGN\] A json index read (RULES.md D34): the documents posting what
//! a call asks (`need.rs`), found by its rarest part's postings, each then
//! looked up in the others' by one descent, and ORs merged in order;
//! every document posting `UNREAD` too; every document, by its entries;
//! and the rows of a document, by its hash.
//!
//! A search holds a leaf of each posting list it merges, and what it gives
//! holds the index, so its pages are not freed while it is read.

use std::ops::{Bound, Deref};
use std::sync::Arc;

use super::JsonIndex;
use super::document::{hash_in, hash_key};
use super::keys::{
    DOCUMENT, Doc, count_key, count_of, document_of, hashed_range, hashed_row, parts, posting_doc, posting_key, posting_range, tag_range,
};
use super::need::{Need, need_of};
use super::terms::{Term, UNREAD};
use crate::engine::tree::{BLinkTree, Direction, Scan, scan_of};
use crate::error::{Code, Result, SqlError};
use crate::index::{Entries, Probe, RowIds};
use crate::storage::RowId;

/// Documents found, in order of hash and row, each once.
type Docs = Box<dyn Iterator<Item = Result<Doc>> + Send>;

/// An index's tree, reached through the index it holds: what a search
/// reads, so the index outlives the search.
struct Held(Arc<JsonIndex>);

impl Deref for Held {
    type Target = BLinkTree;
    fn deref(&self) -> &BLinkTree {
        &self.0.tree
    }
}

/// The scan of `index`'s keys from `low` to `high`, both included,
/// holding it.
fn scan(index: &Arc<JsonIndex>, (low, high): (Vec<u8>, Vec<u8>)) -> Scan<Held> {
    scan_of(Held(Arc::clone(index)), Bound::Included(&low), Bound::Included(&high), Direction::Forward)
}

/// The documents posting `term`.
fn postings(index: &Arc<JsonIndex>, term: &Term) -> Docs {
    Box::new(scan(index, posting_range(term, None)).map(|entry| posting_doc(&entry?.0)))
}

/// The error of a probe it does not answer.
fn refused() -> SqlError {
    SqlError::new(Code::Unsupported, "[EXTENSION] the json index method answers SQL/JSON calls and every entry, no comparison")
}

/// `search`: every document for `Probe::All`; for a call, those posting
/// what it asks, and every one posting `UNREAD`; every document if it asks
/// nothing.
pub(super) fn search(index: &Arc<JsonIndex>, probe: &Probe) -> Entries {
    let found: Docs = match probe {
        Probe::All => every(index),
        Probe::Predicate { predicate, value } => {
            let Some(call) = predicate.function.0.json_probe() else { return Box::new(std::iter::once(Err(refused()))) };
            match need_of(&call, value, index.shape) {
                Need::Every => every(index),
                need @ (Need::Term(_) | Need::And(_) | Need::Or(_)) => {
                    Box::new(Union::of(vec![drive(index, &need), postings(index, &UNREAD)]))
                }
            }
        }
        Probe::Columns(_) | Probe::Compare { .. } | Probe::Prefix { .. } | Probe::Range { .. } => {
            return Box::new(std::iter::once(Err(refused())));
        }
    };
    Box::new(found.map(|doc| {
        let (hash, row) = parts(&doc?);
        Ok((hash_key(&hash), row))
    }))
}

/// Every document, by its entries, in row order.
fn every(index: &Arc<JsonIndex>) -> Docs {
    let (low, high) = tag_range(DOCUMENT);
    Box::new(
        scan_of(Held(Arc::clone(index)), Bound::Included(&low), Bound::Excluded(&high), Direction::Forward).map(|e| document_of(&e?.0)),
    )
}

/// The documents `need` finds, in order: of an AND, its rarest part's,
/// each looked up in the rest; of an OR, its parts' merged.
fn drive(index: &Arc<JsonIndex>, need: &Need) -> Docs {
    match need {
        Need::Every => every(index),
        Need::Term(term) => postings(index, term),
        Need::Or(parts) => Box::new(Union::of(parts.iter().map(|p| drive(index, p)).collect())),
        Need::And(parts) => {
            let rarest =
                (0..parts.len()).min_by_key(|&at| parts.get(at).and_then(|p| estimate(index, p).ok().flatten()).unwrap_or(u64::MAX));
            let Some((driver, rest)) = rarest.and_then(|at| Some((parts.get(at)?.clone(), without(parts, at)))) else {
                return every(index);
            };
            let held = Arc::clone(index);
            Box::new(drive(index, &driver).filter_map(move |doc| match doc {
                Ok(doc) => match rest.iter().try_fold(true, |all, p| Ok::<bool, SqlError>(all && holds(&held, p, &doc)?)) {
                    Ok(true) => Some(Ok(doc)),
                    Ok(false) => None,
                    Err(e) => Some(Err(e)),
                },
                Err(e) => Some(Err(e)),
            }))
        }
    }
}

/// `parts` but the one at `at`.
fn without(parts: &[Need], at: usize) -> Vec<Need> {
    parts.iter().enumerate().filter(|(i, _)| *i != at).map(|(_, p)| p.clone()).collect()
}

/// Whether `doc` posts what `need` asks.
///
/// # Errors
///
/// The tree's.
fn holds(index: &JsonIndex, need: &Need, doc: &Doc) -> Result<bool> {
    match need {
        Need::Every => Ok(true),
        Need::Term(term) => Ok(index.tree.get(&posting_key(term, doc))?.is_some()),
        Need::And(parts) => {
            for p in parts {
                if !holds(index, p, doc)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        Need::Or(parts) => {
            for p in parts {
                if holds(index, p, doc)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
    }
}

/// The documents `need` may find, by the counts of its terms: of an AND
/// its rarest part's, of an OR its parts' together; none known of every
/// document.
///
/// # Errors
///
/// The tree's.
pub(super) fn estimate(index: &JsonIndex, need: &Need) -> Result<Option<u64>> {
    Ok(match need {
        Need::Every => None,
        Need::Term(term) => Some(count(index, term)?),
        Need::And(parts) => {
            let mut least: Option<u64> = None;
            for p in parts {
                if let Some(n) = estimate(index, p)? {
                    least = Some(least.map_or(n, |l| l.min(n)));
                }
            }
            least
        }
        Need::Or(parts) => {
            let mut sum = 0_u64;
            for p in parts {
                let Some(n) = estimate(index, p)? else { return Ok(None) };
                sum = sum.saturating_add(n);
            }
            Some(sum)
        }
    })
}

/// The documents posting `term`.
///
/// # Errors
///
/// The tree's; SQLCODE -916 of a count not of the layout.
pub(super) fn count(index: &JsonIndex, term: &Term) -> Result<u64> {
    index.tree.get(&count_key(term))?.map_or(Ok(0), |v| count_of(&v))
}

/// The estimate of `probe`: of a call, its need's, and the documents it
/// cannot read, all of which every probe finds.
pub(super) fn estimated(index: &JsonIndex, probe: &Probe) -> Option<usize> {
    let Probe::Predicate { predicate, value } = probe else { return None };
    let call = predicate.function.0.json_probe()?;
    let found = estimate(index, &need_of(&call, value, index.shape)).ok()??;
    let unread = count(index, &UNREAD).ok()?;
    usize::try_from(found.saturating_add(unread)).ok()
}

/// The rows with an entry of the document `key` projects to, each once.
pub(super) fn key_rows(index: &Arc<JsonIndex>, key: &crate::index::Key) -> RowIds {
    match hash_in(key) {
        Ok(Some(hash)) => Box::new(scan(index, hashed_range(&hash)).map(|entry| hashed_row(&entry?.0))),
        Ok(None) => Box::new(std::iter::empty()),
        Err(e) => Box::new(std::iter::once(Err(e))),
    }
}

/// `key_rows`, gathered.
///
/// # Errors
///
/// As reading them.
pub(super) fn rows_of(index: &JsonIndex, key: &crate::index::Key) -> Result<Vec<RowId>> {
    let Some(hash) = hash_in(key)? else { return Ok(Vec::new()) };
    let (low, high) = hashed_range(&hash);
    index.tree.scan(Bound::Included(&low), Bound::Included(&high), Direction::Forward).map(|entry| hashed_row(&entry?.0)).collect()
}

/// The documents of several streams, each in order, merged in order,
/// each once.
struct Union {
    heads: Vec<(Option<Doc>, Docs)>,
    failed: Option<SqlError>,
}

impl Union {
    /// The merge of `streams`.
    fn of(streams: Vec<Docs>) -> Union {
        let mut failed = None;
        let heads = streams
            .into_iter()
            .map(|mut s| {
                let head = match s.next().transpose() {
                    Ok(head) => head,
                    Err(e) => {
                        failed.get_or_insert(e);
                        None
                    }
                };
                (head, s)
            })
            .collect();
        Union { heads, failed }
    }
}

impl Iterator for Union {
    type Item = Result<Doc>;

    fn next(&mut self) -> Option<Result<Doc>> {
        if let Some(e) = self.failed.take() {
            self.heads.clear();
            return Some(Err(e));
        }
        let least = self.heads.iter().filter_map(|(head, _)| *head).min()?;
        for (head, stream) in &mut self.heads {
            if *head == Some(least) {
                match stream.next().transpose() {
                    Ok(next) => *head = next,
                    Err(e) => {
                        *head = None;
                        self.failed.get_or_insert(e);
                    }
                }
            }
        }
        Some(Ok(least))
    }
}

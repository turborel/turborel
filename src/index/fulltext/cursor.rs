//! \[DESIGN\] Cursors over a term's blocks (RULES.md D86): the document
//! numbers holding a term, read a block at a time, rising, with
//! `advance_to(d)` moving to the first at or past `d`. A cursor reads on
//! along the term's block keys, skipping a block whose next block's key is
//! still at most `d` without decoding it; past `SKIPS_BEFORE_SEEK` of
//! those it descends again, to the last block keyed at most `d`, so a far
//! jump costs a descent, not a walk: the block keys are its skip list.
//!
//! Streams of document numbers (`Docs`) combine: several intersect by
//! leapfrogging, each advanced to the greatest number another found until
//! all agree (`Meet`); several unite by giving the least of their heads
//! (`Union`, `Terms`); every document is read from the number entries
//! (`Every`). Each holds a block and the entry after it for each term. A
//! reader may see a document twice while its block is split (`block.rs`);
//! numbers kept rising give it once.

use std::ops::Bound;
use std::sync::Arc;

use super::FullTextIndex;
use super::block::frequencies;
use super::keys::{DOCNO_BYTES, NUMBER, block_docno, block_key, block_range, corrupt, number_key};
use super::search::Held;
use crate::engine::tree::{Direction, Scan, scan_of};
use crate::error::Result;

/// \[DESIGN\] The blocks a cursor skips by their keys before it descends
/// again to the one it wants.
const SKIPS_BEFORE_SEEK: usize = 4;
/// \[DESIGN\] The number entries `Every` reads past before it descends
/// again to the one it wants.
const NUMBERS_BEFORE_SEEK: u64 = 64;

/// An entry of the tree: its key and value.
type Entry = (Vec<u8>, Vec<u8>);

/// A stream of document numbers, rising.
pub(super) trait Docs: Send {
    /// The first number at or past `docno`, none past the last. Targets
    /// asked never fall.
    ///
    /// # Errors
    ///
    /// The tree's; SQLCODE -916 if an entry is malformed.
    fn advance_to(&mut self, docno: u64) -> Result<Option<u64>>;
}

/// The document numbers of one term, rising, with its frequency in each.
pub(super) struct Cursor {
    index: Arc<FullTextIndex>,
    term: Vec<u8>,
    scan: Scan<Held>,
    /// An entry read past the block decoded, not yet taken.
    pending: Option<Entry>,
    /// The decoded block's numbers and frequencies, and where in them it
    /// is.
    docs: Vec<u64>,
    frequencies: Vec<u32>,
    at: usize,
}

impl Cursor {
    /// The cursor of `term` in `index`, before its first document.
    pub fn new(index: &Arc<FullTextIndex>, term: Vec<u8>) -> Cursor {
        let (lower, upper) = block_range(&term);
        let scan = scan_of(Held(Arc::clone(index)), Bound::Included(&lower), Bound::Excluded(&upper), Direction::Forward);
        Cursor { index: Arc::clone(index), term, scan, pending: None, docs: Vec::new(), frequencies: Vec::new(), at: 0 }
    }

    /// Its term.
    pub fn term(&self) -> &[u8] {
        &self.term
    }

    /// The number it is at, if it has found one.
    pub fn head(&self) -> Option<u64> {
        self.docs.get(self.at).copied()
    }

    /// The term's frequency in the document it is at, 0 if none.
    pub fn frequency(&self) -> u32 {
        self.frequencies.get(self.at).copied().unwrap_or_default()
    }

    /// The next entry: the one read ahead, or the scan's.
    fn entry(&mut self) -> Result<Option<Entry>> {
        match self.pending.take() {
            Some(entry) => Ok(Some(entry)),
            None => self.scan.next().transpose(),
        }
    }

    /// Reads on from the last block keyed at most `docno`, by a descent.
    fn seek(&mut self, docno: u64) -> Result<()> {
        let (lower, upper) = block_range(&self.term);
        let bound = block_key(&self.term, docno);
        let tree = &self.index.tree;
        let found = tree.scan(Bound::Included(&lower), Bound::Included(&bound), Direction::Backward).next().transpose()?;
        let from = found.map_or(lower, |(key, _)| key);
        self.scan = scan_of(Held(Arc::clone(&self.index)), Bound::Included(&from), Bound::Excluded(&upper), Direction::Forward);
        self.pending = None;
        Ok(())
    }

    /// Decodes the next block that may hold `docno` or a later number:
    /// those before it skipped by their keys; false past the last.
    fn next_block(&mut self, docno: u64) -> Result<bool> {
        let mut skipped = 0_usize;
        loop {
            let Some((key, value)) = self.entry()? else { return Ok(false) };
            let next = self.entry()?;
            let later = match next.as_ref() {
                Some((next_key, _)) => block_docno(next_key, &self.term)? <= docno,
                None => false,
            };
            self.pending = next;
            if !later {
                (self.docs, self.frequencies) = frequencies(block_docno(&key, &self.term)?, &value)?;
                self.at = 0;
                #[cfg(test)]
                self.index.blocks_read.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                return Ok(true);
            }
            skipped = skipped.saturating_add(1);
            if skipped >= SKIPS_BEFORE_SEEK {
                self.seek(docno)?;
                skipped = 0;
            }
        }
    }
}

/// A stream boxed is read as itself.
impl Docs for Box<dyn Docs> {
    fn advance_to(&mut self, docno: u64) -> Result<Option<u64>> {
        (**self).advance_to(docno)
    }
}

impl Docs for Cursor {
    fn advance_to(&mut self, docno: u64) -> Result<Option<u64>> {
        loop {
            let rest = self.docs.get(self.at..).unwrap_or_default();
            self.at = self.at.saturating_add(rest.partition_point(|&d| d < docno));
            if let Some(&found) = self.docs.get(self.at) {
                return Ok(Some(found));
            }
            if !self.next_block(docno)? {
                return Ok(None);
            }
        }
    }
}

/// The numbers every stream holds.
pub(super) struct Meet {
    streams: Vec<Box<dyn Docs>>,
}

impl Meet {
    /// The intersection of `streams`, which are some.
    pub fn new(streams: Vec<Box<dyn Docs>>) -> Meet {
        Meet { streams }
    }
}

impl Docs for Meet {
    fn advance_to(&mut self, docno: u64) -> Result<Option<u64>> {
        let mut target = docno;
        'round: loop {
            for stream in &mut self.streams {
                let Some(found) = stream.advance_to(target)? else { return Ok(None) };
                if found > target {
                    target = found;
                    continue 'round;
                }
            }
            return Ok(Some(target));
        }
    }
}

/// The least head of streams, which are advanced to a target lazily: a
/// stream whose head is at or past it is not asked again.
fn least<S: Docs>(heads: &mut [Option<u64>], streams: &mut [S], docno: u64) -> Result<Option<u64>> {
    let mut least: Option<u64> = None;
    for (head, stream) in heads.iter_mut().zip(streams.iter_mut()) {
        if head.is_none_or(|at| at < docno) {
            *head = stream.advance_to(docno)?.map(|at| at.max(docno));
            if head.is_none() {
                *head = Some(u64::MAX);
                continue;
            }
        }
        let found = head.filter(|at| *at != u64::MAX);
        least = match (least, found) {
            (Some(l), Some(f)) => Some(l.min(f)),
            (l, f) => l.or(f),
        };
    }
    Ok(least)
}

/// The numbers any stream holds.
pub(super) struct Union {
    streams: Vec<Box<dyn Docs>>,
    /// Each stream's head, `u64::MAX` once it ends.
    heads: Vec<Option<u64>>,
}

impl Union {
    /// The union of `streams`.
    pub fn new(streams: Vec<Box<dyn Docs>>) -> Union {
        Union { heads: vec![None; streams.len()], streams }
    }
}

impl Docs for Union {
    fn advance_to(&mut self, docno: u64) -> Result<Option<u64>> {
        least(&mut self.heads, &mut self.streams, docno)
    }
}

/// The numbers any of several terms' cursors holds, which tells which
/// terms the number it is at holds.
pub(super) struct Terms {
    cursors: Vec<Cursor>,
    heads: Vec<Option<u64>>,
}

impl Terms {
    /// The union of the cursors of `terms` in `index`.
    pub fn new(index: &Arc<FullTextIndex>, terms: Vec<Vec<u8>>) -> Terms {
        let cursors: Vec<Cursor> = terms.into_iter().map(|term| Cursor::new(index, term)).collect();
        Terms { heads: vec![None; cursors.len()], cursors }
    }

    /// The cursors at `docno`, once it is the number found.
    pub fn at(&self, docno: u64) -> impl Iterator<Item = &Cursor> {
        self.cursors.iter().filter(move |cursor| cursor.head() == Some(docno))
    }
}

impl Docs for Terms {
    fn advance_to(&mut self, docno: u64) -> Result<Option<u64>> {
        least(&mut self.heads, &mut self.cursors, docno)
    }
}

/// Every document's number, from the number entries.
pub(super) struct Every {
    index: Arc<FullTextIndex>,
    scan: Scan<Held>,
    /// The number read last, not yet passed.
    head: Option<u64>,
    done: bool,
}

impl Every {
    /// Every document of `index`.
    pub fn new(index: &Arc<FullTextIndex>) -> Every {
        Every { scan: Every::scan(index, 0), index: Arc::clone(index), head: None, done: false }
    }

    /// The number entries of `index` from `docno` on.
    fn scan(index: &Arc<FullTextIndex>, docno: u64) -> Scan<Held> {
        let lower = number_key(docno);
        let upper = [NUMBER.saturating_add(1)];
        scan_of(Held(Arc::clone(index)), Bound::Included(&lower), Bound::Excluded(&upper[..]), Direction::Forward)
    }
}

impl Docs for Every {
    fn advance_to(&mut self, docno: u64) -> Result<Option<u64>> {
        if self.head.is_some_and(|head| head.saturating_add(NUMBERS_BEFORE_SEEK) < docno) {
            self.scan = Every::scan(&self.index, docno);
        }
        loop {
            if let Some(head) = self.head.filter(|head| *head >= docno) {
                return Ok(Some(head));
            }
            if self.done {
                return Ok(None);
            }
            let Some((key, _)) = self.scan.next().transpose()? else {
                self.done = true;
                return Ok(None);
            };
            let bytes = key.get(1..).ok_or_else(corrupt)?;
            let number: [u8; DOCNO_BYTES] = bytes.try_into().map_err(|_| corrupt())?;
            self.head = Some(u64::from_be_bytes(number));
        }
    }
}

/// No number.
pub(super) struct Nothing;

impl Docs for Nothing {
    fn advance_to(&mut self, _docno: u64) -> Result<Option<u64>> {
        Ok(None)
    }
}

/// The numbers of a stream, each once, rising.
pub(super) struct Numbers {
    docs: Box<dyn Docs>,
    /// The least number not yet given.
    target: u64,
    done: bool,
}

impl Numbers {
    /// The numbers of `docs`.
    pub fn new(docs: Box<dyn Docs>) -> Numbers {
        Numbers { docs, target: 0, done: false }
    }
}

impl Iterator for Numbers {
    type Item = Result<u64>;

    fn next(&mut self) -> Option<Result<u64>> {
        if self.done {
            return None;
        }
        match self.docs.advance_to(self.target) {
            Ok(Some(docno)) => {
                match docno.checked_add(1) {
                    Some(next) => self.target = next,
                    None => self.done = true,
                }
                Some(Ok(docno))
            }
            Ok(None) => {
                self.done = true;
                None
            }
            Err(e) => {
                self.done = true;
                Some(Err(e))
            }
        }
    }
}

/// The numbers every cursor of `cursors` holds, each once, rising.
pub(super) fn intersect(cursors: Vec<Cursor>) -> Numbers {
    let streams: Vec<Box<dyn Docs>> = cursors.into_iter().map(|cursor| -> Box<dyn Docs> { Box::new(cursor) }).collect();
    Numbers::new(Box::new(Meet::new(streams)))
}

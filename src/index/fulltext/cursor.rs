//! \[DESIGN\] Cursors over a term's blocks (RULES.md D86): the document
//! numbers holding a term, read a block at a time, rising, with
//! `advance_to(d)` moving to the first at or past `d`. A cursor reads on
//! along the term's block keys, skipping a block whose next block's key is
//! still at most `d` without decoding it; past `SKIPS_BEFORE_SEEK` of
//! those it descends again, to the last block keyed at most `d`, so a far
//! jump costs a descent, not a walk: the block keys are its skip list.
//!
//! Several cursors intersect by leapfrogging: each is advanced to the
//! greatest number another found, until all agree, holding one block and
//! the entry after it for each term. A reader may see a document twice
//! while its block is split (`block.rs`); numbers kept rising give it once.

use std::ops::Bound;
use std::sync::Arc;

use super::FullTextIndex;
use super::block::docnos;
use super::keys::{block_docno, block_key, block_range};
use super::search::Held;
use crate::engine::tree::{Direction, Scan, scan_of};
use crate::error::Result;

/// \[DESIGN\] The blocks a cursor skips by their keys before it descends
/// again to the one it wants.
const SKIPS_BEFORE_SEEK: usize = 4;

/// An entry of the tree: a block's key and value.
type Entry = (Vec<u8>, Vec<u8>);

/// The document numbers of one term, rising.
pub(super) struct Cursor {
    index: Arc<FullTextIndex>,
    term: Vec<u8>,
    scan: Scan<Held>,
    /// An entry read past the block decoded, not yet taken.
    pending: Option<Entry>,
    /// The decoded block's numbers, and where in them it is.
    docs: Vec<u64>,
    at: usize,
}

impl Cursor {
    /// The cursor of `term` in `index`, before its first document.
    pub fn new(index: &Arc<FullTextIndex>, term: Vec<u8>) -> Cursor {
        let (lower, upper) = block_range(&term);
        let scan = scan_of(Held(Arc::clone(index)), Bound::Included(&lower), Bound::Excluded(&upper), Direction::Forward);
        Cursor { index: Arc::clone(index), term, scan, pending: None, docs: Vec::new(), at: 0 }
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

    /// The first document number at or past `docno`, none past the last.
    ///
    /// # Errors
    ///
    /// The tree's; SQLCODE -916 if a block is malformed.
    pub fn advance_to(&mut self, docno: u64) -> Result<Option<u64>> {
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
                self.docs = docnos(block_docno(&key, &self.term)?, &value)?;
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

/// The document numbers every cursor holds, rising, each once.
pub(super) struct Intersect {
    cursors: Vec<Cursor>,
    /// The least number not yet given.
    target: u64,
    done: bool,
}

impl Intersect {
    /// The intersection of `cursors`.
    pub fn new(cursors: Vec<Cursor>) -> Intersect {
        Intersect { cursors, target: 0, done: false }
    }

    /// Advances every cursor to the target until all agree: the number
    /// they agree on, none if one is done.
    fn agreed(&mut self) -> Result<Option<u64>> {
        'round: loop {
            for cursor in &mut self.cursors {
                let Some(found) = cursor.advance_to(self.target)? else { return Ok(None) };
                if found > self.target {
                    self.target = found;
                    continue 'round;
                }
            }
            return Ok(Some(self.target));
        }
    }
}

impl Iterator for Intersect {
    type Item = Result<u64>;

    fn next(&mut self) -> Option<Result<u64>> {
        if self.done {
            return None;
        }
        let found = self.agreed();
        match found {
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

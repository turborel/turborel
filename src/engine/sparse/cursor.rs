//! \[DESIGN\] A cursor over one dimension's posting blocks, for Block-Max
//! WAND (`wand.rs`). It has two positions, as Ding and Suel's has (SIGIR
//! 2011, §4): a shallow one, the block whose header it has read, moved
//! without decoding a posting; and a deep one, the posting it is at,
//! which decodes its block.
//!
//! It reads the blocks by a forward scan of the tree, a leaf at a time,
//! holding no latch between them. A block split or merged while it reads
//! may come again in part, so it skips every posting of a row at most the
//! last row of the blocks it has passed: a writer never parts two postings
//! of one row (`block.rs`), and a block's rows lie below the next block's
//! key (`keys.rs`).

use std::sync::Arc;

use super::block::{Header, Postings};
use super::keys;
use crate::engine::tree::{BLinkTree, Direction, Scan, scan_of};
use crate::error::Result;
use std::ops::Bound;

/// The most a posting of weight from `min` to `max` adds to a score, by
/// a query weight of `query`.
pub(super) fn reach(query: f64, max: f32, min: f32) -> f64 {
    if query >= 0.0_f64 { query * f64::from(max) } else { query * f64::from(min) }
}

/// The block a cursor is in.
struct Current {
    header: Header,
    bytes: Vec<u8>,
    /// Its postings, once decoded.
    postings: Option<Postings>,
    /// The deep position in them.
    at: usize,
}

/// A cursor over a dimension of the query.
pub(super) struct Cursor {
    /// The query's weight of the dimension.
    pub weight: f64,
    /// The most a posting of the dimension adds to a score, never below 0:
    /// a row without the dimension gets 0 of it.
    pub bound: f64,
    scan: Scan<Arc<BLinkTree>>,
    block: Option<Current>,
    /// The last row of the blocks passed.
    floor: Option<u64>,
    /// The row of the deep position; none past the last.
    pub doc: Option<u64>,
}

impl Cursor {
    /// A cursor at the first posting of `dimension`, whose weights lie in
    /// `bounds`, greatest then least.
    pub(super) fn new(tree: &Arc<BLinkTree>, dimension: u32, weight: f64, bounds: (f32, f32)) -> Result<Cursor> {
        let (lower, upper) = keys::dimension_range(dimension);
        let scan = scan_of(Arc::clone(tree), Bound::Included(lower.as_slice()), Bound::Excluded(upper.as_slice()), Direction::Forward);
        let bound = reach(weight, bounds.0, bounds.1).max(0.0_f64);
        let mut cursor = Cursor { weight, bound, scan, block: None, floor: None, doc: None };
        cursor.advance(0)?;
        Ok(cursor)
    }

    /// Moves the shallow position to the block holding rows from `target`,
    /// reading only headers; none past the last.
    pub(super) fn shallow(&mut self, target: u64) -> Result<()> {
        while self.block.as_ref().is_none_or(|b| b.header.last < target) {
            if let Some(passed) = self.block.take() {
                self.floor = Some(self.floor.map_or(passed.header.last, |f| f.max(passed.header.last)));
            }
            let Some(entry) = self.scan.next() else { return Ok(()) };
            let (_, bytes) = entry?;
            self.block = Some(Current { header: Header::read(&bytes)?, bytes, postings: None, at: 0 });
        }
        Ok(())
    }

    /// The most a posting of the shallow block adds to a score, never
    /// below 0; 0 past the last block.
    pub(super) fn block_bound(&self) -> f64 {
        self.block.as_ref().map_or(0.0_f64, |b| reach(self.weight, b.header.max, b.header.min).max(0.0_f64))
    }

    /// The last row of the shallow block; none past the last block.
    pub(super) fn block_last(&self) -> Option<u64> {
        self.block.as_ref().map(|b| b.header.last)
    }

    /// Moves the deep position to the first posting of a row at least
    /// `target`, decoding its block; `doc` none if there is none.
    pub(super) fn advance(&mut self, target: u64) -> Result<()> {
        let mut want = match self.floor {
            Some(floor) => target.max(floor.saturating_add(1)),
            None => target,
        };
        loop {
            self.shallow(want)?;
            let Some(current) = self.block.as_mut() else {
                self.doc = None;
                return Ok(());
            };
            if current.postings.is_none() {
                current.postings = Some(Postings::decode(&current.bytes)?);
            }
            let rows = current.postings.as_ref().map(|p| p.rows.as_slice()).unwrap_or_default();
            let from = current.at.min(rows.len());
            let at = from.saturating_add(rows.get(from..).unwrap_or_default().partition_point(|&r| r < want));
            if let Some(&row) = rows.get(at) {
                current.at = at;
                self.doc = Some(row);
                return Ok(());
            }
            // A header whose last row is past the postings: read on.
            let Some(next) = current.header.last.checked_add(1) else {
                self.doc = None;
                return Ok(());
            };
            want = want.max(next);
        }
    }

    /// The score the deep position's row gets of this dimension: the most
    /// of its versions' postings, how far that may be from the score of
    /// the weights inserted, and whether the row has several postings, of
    /// several versions, so that the score may mix theirs.
    pub(super) fn score(&self) -> Scored {
        let none = Scored { score: 0.0_f64, error: 0.0_f64, several: false };
        let (Some(current), Some(row)) = (self.block.as_ref(), self.doc) else { return none };
        let Some(postings) = current.postings.as_ref() else { return none };
        let rows = postings.rows.get(current.at..).unwrap_or_default();
        let weights = postings.weights.get(current.at..).unwrap_or_default();
        let mut best = f64::NEG_INFINITY;
        let mut count = 0_usize;
        for (_, &w) in rows.iter().zip(weights).take_while(|(r, _)| **r == row) {
            best = best.max(self.weight * w);
            count = count.saturating_add(1);
        }
        let score = if best.is_finite() { best } else { 0.0_f64 };
        Scored { score, error: self.weight.abs() * postings.error, several: count > 1 }
    }
}

/// A row's score of one dimension (`Cursor::score`).
pub(super) struct Scored {
    /// The most its postings add.
    pub score: f64,
    /// How far that may be from the weights inserted.
    pub error: f64,
    /// Whether it has postings of several versions.
    pub several: bool,
}

//! \[DESIGN\] Counted writes of a sparse inverted index. A row's vector
//! is counted under its hash (`keys.rs`): the insert that finds no count
//! writes its postings, one to each dimension it holds, before its count,
//! and the remove that takes its last count takes them back before the
//! count goes. A failure part way takes back what the write did, so a
//! failed write makes no logical change (RULES.md D19).
//!
//! A posting goes in the block of its dimension of the greatest key at
//! most its row, under the dimension's stripe lock. A block grown past
//! the block size, or past what one entry of the tree holds, is cut in
//! pieces at row boundaries. Pieces under new keys are written before
//! the old key is cut down or taken away, so a reader finds every posting
//! held throughout in one block or another, and skips those it has seen
//! by their rows (`cursor.rs`).

use std::ops::Bound;

use super::block::{Header, Postings, Quantization};
use super::keys::{self, BLOCK_KEY};
use super::{SparseTree, SparseVector};
use crate::engine::tree::Direction;
use crate::error::{Code, Result, SqlError};
use crate::storage::RowId;

/// A tree entry: its key and value.
type Entry = (Vec<u8>, Vec<u8>);

impl SparseTree {
    /// Adds an entry of `row` with `vector`: a count, and the first
    /// count's postings.
    ///
    /// # Errors
    ///
    /// The tree's; SQLCODE -910 if the count would overflow, or -900 if
    /// one row has more versions of a dimension than a block holds. A
    /// failed insert has taken back what it wrote.
    pub fn insert(&self, vector: &SparseVector, row: RowId) -> Result<()> {
        let key = keys::document_key(&vector.hash(), row);
        let _row = self.row_lock(row)?;
        if let Some(value) = self.inner.tree.get(&key)? {
            let count = keys::count_of(&value)?
                .checked_add(1)
                .ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] a sparse index entry is counted at most 2^32 - 1 times"))?;
            self.inner.tree.replace(&key, &keys::count_value(count))?;
            return Ok(());
        }
        for (done, (dimension, weight)) in vector.entries().enumerate() {
            if let Err(error) = self.add(dimension, row, weight) {
                self.take_back(vector.entries().take(done), row, false);
                return Err(error);
            }
        }
        let written = self.inner.tree.insert(&key, &keys::count_value(1)).and_then(|fresh| {
            if fresh { Ok(()) } else { Err(SqlError::new(Code::Corrupt, "[DESIGN] a sparse index count appeared under its row's lock")) }
        });
        if let Err(error) = written {
            self.take_back(vector.entries(), row, false);
            return Err(error);
        }
        self.counted(true);
        Ok(())
    }

    /// Takes back one entry of `row` with `vector`, if there is one: its
    /// count, and with the last its postings.
    ///
    /// # Errors
    ///
    /// The tree's. A failed remove has put back what it took.
    pub fn remove(&self, vector: &SparseVector, row: RowId) -> Result<()> {
        let key = keys::document_key(&vector.hash(), row);
        let _row = self.row_lock(row)?;
        let Some(value) = self.inner.tree.get(&key)? else { return Ok(()) };
        let count = keys::count_of(&value)?;
        if count > 1 {
            self.inner.tree.replace(&key, &keys::count_value(count.saturating_sub(1)))?;
            return Ok(());
        }
        for (done, (dimension, weight)) in vector.entries().enumerate() {
            if let Err(error) = self.cut(dimension, row, weight) {
                self.take_back(vector.entries().take(done), row, true);
                return Err(error);
            }
        }
        if let Err(error) = self.inner.tree.delete(&key) {
            self.take_back(vector.entries(), row, true);
            return Err(error);
        }
        self.counted(false);
        Ok(())
    }

    /// Undoes a failed write's postings: puts them back if `put`, else
    /// cuts them. Its own failure leaves the index as the failure did; the
    /// caller reports the first error, which the statement's undo meets.
    fn take_back(&self, entries: impl Iterator<Item = (u32, f32)>, row: RowId, put: bool) {
        for (dimension, weight) in entries {
            let undone = if put { self.add(dimension, row, weight) } else { self.cut(dimension, row, weight) };
            // The first error is the one reported; a second changes nothing
            // the caller can act on.
            let _ = undone;
        }
    }

    /// Adds `row`'s posting of `weight` to `dimension`.
    fn add(&self, dimension: u32, row: RowId, weight: f32) -> Result<()> {
        let _dimension = self.dimension_lock(dimension)?;
        self.widen(dimension, weight)?;
        let Some((key, value)) = self.block_at(dimension, row)? else {
            let layout = self.layout();
            let bytes = Postings::single(row, weight).encode(layout.quantization)?;
            self.inner.tree.insert(&keys::block_key(dimension, row), &bytes)?;
            return Ok(());
        };
        let quantization = Header::read(&value)?.quantization;
        let mut postings = Postings::decode(&value)?;
        postings.insert(row, weight);
        self.rewrite(dimension, &key, postings, quantization)
    }

    /// Takes back `row`'s posting of `weight` from `dimension`, if it has one.
    fn cut(&self, dimension: u32, row: RowId, weight: f32) -> Result<()> {
        let _dimension = self.dimension_lock(dimension)?;
        let Some((key, value)) = self.block_at(dimension, row)? else { return Ok(()) };
        let quantization = Header::read(&value)?.quantization;
        let mut postings = Postings::decode(&value)?;
        if !postings.remove(row, weight) {
            return Ok(());
        }
        if postings.is_empty() {
            self.inner.tree.delete(&key)?;
        } else {
            self.rewrite(dimension, &key, postings, quantization)?;
        }
        self.dirtied(dimension);
        Ok(())
    }

    /// Widens `dimension`'s bounds to hold `weight`, before any block does.
    fn widen(&self, dimension: u32, weight: f32) -> Result<()> {
        let key = keys::summary_key(dimension);
        match self.inner.tree.get(&key)? {
            None => {
                self.inner.tree.insert(&key, &keys::summary_value(weight, weight))?;
            }
            Some(value) => {
                let (max, min) = keys::bounds_of(&value)?;
                if weight > max || weight < min {
                    self.inner.tree.replace(&key, &keys::summary_value(max.max(weight), min.min(weight)))?;
                }
            }
        }
        Ok(())
    }

    /// The block of `dimension` a posting of `row` goes in: of the
    /// greatest key at most `row`, else the dimension's first; none if the
    /// dimension has no block.
    pub(super) fn block_at(&self, dimension: u32, row: RowId) -> Result<Option<Entry>> {
        let (low, high) = (keys::block_key(dimension, 0), keys::block_key(dimension, row));
        let tree = &self.inner.tree;
        if let Some(found) = tree.scan(Bound::Included(&low), Bound::Included(&high), Direction::Backward).next() {
            return found.map(Some);
        }
        let (_, past) = keys::dimension_range(dimension);
        tree.scan(Bound::Excluded(&high), Bound::Excluded(&past), Direction::Forward).next().transpose()
    }

    /// Writes `postings` in place of the block of `dimension` under `old`.
    pub(super) fn rewrite(&self, dimension: u32, old: &[u8], postings: Postings, quantization: Quantization) -> Result<()> {
        self.regroup(dimension, &[old.to_vec()], postings, quantization)
    }

    /// Writes `postings` in place of the blocks of `dimension` under
    /// `olds`, adjacent, in key order: in pieces that fit, each under its
    /// first row's key, the first under the first old key unless it starts
    /// below it. Pieces under new keys are written first; then old keys
    /// given pieces, left to right; then the old keys left are taken away.
    /// A posting only ever moves to a key at most its old one, or to a new
    /// key, so a reader finds each in one block or another throughout.
    pub(super) fn regroup(&self, dimension: u32, olds: &[Vec<u8>], postings: Postings, quantization: Quantization) -> Result<()> {
        let first_old = olds.first().map(|k| keys::block_of(k)).transpose()?.map_or(0, |(_, first)| first);
        let mut kept: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        for (at, (first, bytes)) in self.pieces(postings, quantization)?.into_iter().enumerate() {
            let stays = at == 0 && first >= first_old;
            let key = olds.first().filter(|_| stays).cloned().unwrap_or_else(|| keys::block_key(dimension, first));
            if olds.contains(&key) {
                kept.push((key, bytes));
            } else if !self.inner.tree.insert(&key, &bytes)? {
                return Err(SqlError::new(Code::Corrupt, "[DESIGN] a sparse index block's key is taken"));
            }
        }
        for (key, bytes) in &kept {
            self.inner.tree.replace(key, bytes)?;
        }
        for old in olds.iter().filter(|old| kept.iter().all(|(k, _)| k != *old)) {
            self.inner.tree.delete(old)?;
        }
        Ok(())
    }

    /// `postings` in blocks of at most the block size that each fit one
    /// entry of the tree, cut at row boundaries: each its first row and
    /// its bytes, in row order.
    pub(super) fn pieces(&self, postings: Postings, quantization: Quantization) -> Result<Vec<(RowId, Vec<u8>)>> {
        let room = self.inner.tree.max_entry().saturating_sub(BLOCK_KEY);
        let most = self.layout().block;
        let mut out = Vec::new();
        let mut pending = vec![postings];
        while let Some(next) = pending.pop() {
            let first = next.rows.first().copied().unwrap_or_default();
            if next.len() <= most {
                let bytes = next.encode(quantization)?;
                if bytes.len() <= room {
                    out.push((first, bytes));
                    continue;
                }
            }
            let Some(at) = next.halfway() else {
                return Err(SqlError::new(Code::Unsupported, "[DESIGN] a sparse index holds fewer versions of one row than a block holds"));
            };
            let (lower, upper) = next.split(at);
            pending.push(upper);
            pending.push(lower);
        }
        Ok(out)
    }
}

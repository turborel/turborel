//! \[DESIGN\] A sparse inverted index's maintenance, on the database's
//! maintenance thread (`Index::maintain`): its vectors counted, once, for
//! an index opened; and the dimensions removes left blocks of merged.
//!
//! A dimension is compacted under its stripe lock: its blocks' headers
//! read first, then runs of adjacent blocks that together fit the block
//! size made one block, and blocks of another quantization than the one
//! set stored again (`SparseTree::regroup`); last its bounds narrowed to
//! its blocks', or taken away with its last block. Inserts only widen a
//! dimension's bounds, so this is what tightens them again.

use std::sync::atomic::Ordering;

use super::block::{Header, Postings, Quantization};
use super::keys::{self, DOCUMENT, SUMMARY};
use super::{Dirty, SparseTree};
use crate::error::Result;
use crate::sync::lock_builtin;

/// A block's key and header.
type Placed = (Vec<u8>, Header);

impl SparseTree {
    /// Does the work due (`maintenance_due`): whether it did any.
    ///
    /// # Errors
    ///
    /// The tree's. Dimensions not compacted stay due.
    pub fn maintain(&self) -> Result<bool> {
        if self.documents().is_none() {
            let counted = self.scan(keys::tag_range(DOCUMENT)).try_fold(0_u64, |n, entry| entry.map(|_| n.saturating_add(1)))?;
            *lock_builtin(&self.inner.documents) = Some(counted);
            return Ok(true);
        }
        if !self.maintenance_due() {
            return Ok(false);
        }
        self.inner.removed.store(0, Ordering::Relaxed);
        let dirty = std::mem::take(&mut *lock_builtin(&self.inner.dirty));
        let dimensions = if dirty.all { self.dimensions()? } else { dirty.dimensions.into_iter().collect() };
        for (at, &dimension) in dimensions.iter().enumerate() {
            if let Err(error) = self.compact(dimension) {
                let mut left = lock_builtin(&self.inner.dirty);
                let rest = dimensions.get(at..).unwrap_or_default();
                *left = Dirty { dimensions: left.dimensions.iter().chain(rest).copied().collect(), all: left.all };
                return Err(error);
            }
        }
        Ok(true)
    }

    /// Every dimension with bounds.
    pub(super) fn dimensions(&self) -> Result<Vec<u32>> {
        self.scan(keys::tag_range(SUMMARY)).map(|entry| keys::summary_of(&entry?.0)).collect()
    }

    /// Merges `dimension`'s small adjacent blocks, stores again those of
    /// another quantization, and narrows its bounds.
    ///
    /// # Errors
    ///
    /// The tree's; SQLCODE -916 if a block is malformed.
    pub fn compact(&self, dimension: u32) -> Result<()> {
        let _dimension = self.dimension_lock(dimension)?;
        let placed: Vec<Placed> = self
            .scan(keys::dimension_range(dimension))
            .map(|entry| entry.and_then(|(k, v)| Ok((k, Header::read(&v)?))))
            .collect::<Result<_>>()?;
        let layout = self.layout();
        for run in runs(&placed, layout.block) {
            let rewritten = run.len() > 1 || run.iter().any(|(_, h)| h.quantization != layout.quantization);
            if rewritten {
                self.merge(dimension, run, layout.quantization)?;
            }
        }
        self.narrow(dimension, &placed)
    }

    /// Writes the blocks of `run` as one, or as few as fit.
    fn merge(&self, dimension: u32, run: &[Placed], quantization: Quantization) -> Result<()> {
        let mut merged: Option<Postings> = None;
        for (key, _) in run {
            let Some(value) = self.inner.tree.get(key)? else { continue };
            let postings = Postings::decode(&value)?;
            merged = Some(match merged {
                Some(before) => before.merge(postings),
                None => postings,
            });
        }
        let Some(merged) = merged else { return Ok(()) };
        let olds: Vec<Vec<u8>> = run.iter().map(|(key, _)| key.clone()).collect();
        self.regroup(dimension, &olds, merged, quantization)
    }

    /// Sets `dimension`'s bounds to those of its blocks `placed` read, or
    /// takes them away if it has none.
    fn narrow(&self, dimension: u32, placed: &[Placed]) -> Result<()> {
        let key = keys::summary_key(dimension);
        let bounds = placed.iter().map(|(_, h)| (h.max, h.min)).reduce(|(a, b), (c, d)| (a.max(c), b.min(d)));
        match bounds {
            Some((max, min)) => self.inner.tree.replace(&key, &keys::summary_value(max, min)).map(drop),
            None => self.inner.tree.delete(&key).map(drop),
        }
    }
}

/// `placed` in runs of adjacent blocks that together hold at most `most`
/// postings.
fn runs(placed: &[Placed], most: usize) -> Vec<&[Placed]> {
    let mut out = Vec::new();
    let (mut start, mut held) = (0_usize, 0_usize);
    for (at, (_, header)) in placed.iter().enumerate() {
        if at > start && held.saturating_add(header.count) > most {
            out.push(placed.get(start..at).unwrap_or_default());
            (start, held) = (at, 0);
        }
        held = held.saturating_add(header.count);
    }
    if start < placed.len() {
        out.push(placed.get(start..).unwrap_or_default());
    }
    out
}

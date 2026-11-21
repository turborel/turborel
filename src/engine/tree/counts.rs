//! The counters of a B-link tree, and what it tells of its reads' misses,
//! for the heuristics above it (RULES.md D30).

use std::sync::atomic::{AtomicU64, Ordering};

use super::BLinkTree;

/// The counters of a tree, for the heuristics above it (RULES.md D30).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TreeCounts {
    /// Nodes split.
    pub splits: u64,
    /// Of those, roots: the tree grew a level.
    pub root_splits: u64,
    /// Links followed to a right neighbour, by readers and writers: a
    /// node had split since its parent was read.
    pub right_moves: u64,
    /// Leaves emptied and taken out of the tree (`reclaim.rs`).
    pub unlinked: u64,
    /// Reads of a table's row the leaf noted for it answered, with no
    /// descent (`hints.rs`).
    pub hinted: u64,
}

#[derive(Default)]
pub(super) struct Counters {
    pub(super) splits: AtomicU64,
    pub(super) root_splits: AtomicU64,
    pub(super) right_moves: AtomicU64,
    pub(super) unlinked: AtomicU64,
    pub(super) hinted: AtomicU64,
}

/// Adds 1 to `counter`; it would wrap only after 2^64.
pub(super) fn count(counter: &AtomicU64) {
    counter.fetch_add(1, Ordering::Relaxed);
}

impl BLinkTree {
    /// The pages its lookups by key read that the pool missed, per lookup,
    /// at most 1, once at least `least` are counted.
    pub fn missed_share(&self, least: u64) -> Option<f64> {
        self.tally.missed_share(least)
    }

    /// The pages its lookups by key read that the pool missed, per lookup,
    /// once at least `least` are counted.
    pub fn missed_pages(&self, least: u64) -> Option<f64> {
        self.tally.missed_per_lookup(least)
    }

    /// The share of its file's pages in use the pool has no frames for,
    /// none if it has for all.
    pub fn unheld_share(&self) -> f64 {
        let pages = |n: u64| crate::stats::parts::count(n);
        let used = pages(self.pool.pages_in_use(self.file).unwrap_or(0));
        let frames = pages(u64::try_from(self.pool.frames()).unwrap_or(u64::MAX));
        if used > frames { 1.0_f64 - frames / used } else { 0.0_f64 }
    }

    /// The counters now.
    pub fn counts(&self) -> TreeCounts {
        let get = |c: &AtomicU64| c.load(Ordering::Relaxed);
        let c = &self.counters;
        TreeCounts {
            splits: get(&c.splits),
            root_splits: get(&c.root_splits),
            right_moves: get(&c.right_moves),
            unlinked: get(&c.unlinked),
            hinted: get(&c.hinted),
        }
    }
}

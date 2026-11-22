//! The counters of a B-link tree, and what it tells of its reads' misses,
//! for the heuristics above it (RULES.md D30).

use std::sync::atomic::{AtomicU64, Ordering};

use super::BLinkTree;
use crate::sync::thread_stripe;

/// \[DESIGN\] Stripes of a counter every read or write may move.
const STRIPES: usize = 16;

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
    /// Leaves emptied, or merged into their right neighbour, and taken out
    /// of the tree (`reclaim.rs`).
    pub unlinked: u64,
    /// Of those, leaves merged: their entries moved into their right
    /// neighbour first (`merge.rs`).
    pub merged: u64,
    /// Reads of a table's row the leaf noted for it answered, with no
    /// descent (`hints.rs`).
    pub hinted: u64,
    /// Entries written into the rightmost leaf with no descent.
    pub edge_writes: u64,
    /// Leaves split early as writers waited for them.
    pub contention_splits: u64,
}

#[derive(Default)]
pub(super) struct Counters {
    pub(super) splits: AtomicU64,
    pub(super) root_splits: AtomicU64,
    pub(super) right_moves: AtomicU64,
    pub(super) unlinked: AtomicU64,
    pub(super) merged: AtomicU64,
    pub(super) hinted: Striped,
    pub(super) edge_writes: Striped,
    pub(super) contention_splits: AtomicU64,
}

/// A stripe of a `Striped` counter, alone on its cache lines.
#[repr(align(128))]
#[derive(Default)]
struct Lane(AtomicU64);

/// A counter that reads and writes of one tree move on every thread, kept
/// by stripe, as the pool's tallies are (`disk/buffer/tally.rs`), so the
/// threads moving it seldom share a cache line.
pub(super) struct Striped(Box<[Lane]>);

impl Default for Striped {
    fn default() -> Striped {
        Striped((0..STRIPES).map(|_| Lane::default()).collect())
    }
}

impl Striped {
    /// Adds 1 on the thread's stripe.
    pub(super) fn add(&self) {
        if let Some(lane) = self.0.get(thread_stripe(STRIPES)) {
            count(&lane.0);
        }
    }

    /// The sum of its stripes.
    fn sum(&self) -> u64 {
        self.0.iter().map(|lane| lane.0.load(Ordering::Relaxed)).fold(0_u64, u64::wrapping_add)
    }
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
            merged: get(&c.merged),
            hinted: c.hinted.sum(),
            edge_writes: c.edge_writes.sum(),
            contention_splits: get(&c.contention_splits),
        }
    }
}

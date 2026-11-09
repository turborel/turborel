//! \[DESIGN\] The counters of a block file (RULES.md D30), counted
//! lock-free as it appends, reads and syncs, for the heuristics above it.

use std::sync::atomic::{AtomicU64, Ordering};

use super::BlockFile;

/// The counters of a block file, for the heuristics above it (RULES.md D30).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BlockCounts {
    /// Blocks appended.
    pub appends: u64,
    /// Payload bytes appended.
    pub appended_bytes: u64,
    /// Bytes they took once encoded: with compression, fewer.
    pub stored_bytes: u64,
    /// Blocks read.
    pub reads: u64,
    /// Payload bytes read.
    pub read_bytes: u64,
    /// Syncs.
    pub syncs: u64,
    /// Blocks read that failed their checksum or were incomplete.
    pub corrupt_reads: u64,
}

/// `BlockCounts`, counted lock-free.
#[derive(Debug, Default)]
pub(super) struct Counters {
    pub(super) appends: AtomicU64,
    pub(super) appended_bytes: AtomicU64,
    pub(super) stored_bytes: AtomicU64,
    pub(super) reads: AtomicU64,
    pub(super) read_bytes: AtomicU64,
    pub(super) syncs: AtomicU64,
    pub(super) corrupt_reads: AtomicU64,
}

/// Adds `n` to `counter`; it would wrap only after 2^64, which no file reaches.
pub(super) fn add(counter: &AtomicU64, n: usize) {
    counter.fetch_add(u64::try_from(n).unwrap_or(u64::MAX), Ordering::Relaxed);
}

impl BlockFile {
    /// The counters now.
    pub fn counts(&self) -> BlockCounts {
        let get = |c: &AtomicU64| c.load(Ordering::Relaxed);
        let c = &self.counters;
        BlockCounts {
            appends: get(&c.appends),
            appended_bytes: get(&c.appended_bytes),
            stored_bytes: get(&c.stored_bytes),
            reads: get(&c.reads),
            read_bytes: get(&c.read_bytes),
            syncs: get(&c.syncs),
            corrupt_reads: get(&c.corrupt_reads),
        }
    }
}

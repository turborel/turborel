//! Geometry candidates grouped by row id for nearby storage fetches.
//! R-tree search promises no SQL key order. Sorting bounded blocks keeps
//! every candidate and leaves MVCC projection and full predicate checks
//! to the executor. Distance-ordered nearest searches never use this path.

use std::sync::Arc;

use crate::error::{Result, SqlError};
use crate::memory::{MemoryBudget, Reservation};
use crate::spatial::Bounds;
use crate::storage::RowId;

type Entry = (Bounds, RowId);

/// Start small: most selective searches finish before needing a larger
/// allocation. The row-fetch locality experiment compared 64–32768 rows;
/// 1024 was the first useful block, and 4096 retained most of the gain
/// while bounding each cursor to 160 KiB of candidate storage.
const INITIAL: usize = 64;
const SORT_FROM: usize = 1024;
const BLOCK: usize = 4096;

pub(super) struct Clustered<I> {
    scan: I,
    buffer: Option<Buffer>,
}

impl<I> Clustered<I> {
    pub(super) fn new(scan: I, budget: Option<&Arc<MemoryBudget>>) -> Self {
        Self { scan, buffer: budget.and_then(Buffer::new) }
    }
}

impl<I: Iterator<Item = Result<Entry>>> Iterator for Clustered<I> {
    type Item = Result<Entry>;

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.buffer {
            Some(buffer) => buffer.next(&mut self.scan),
            None => self.scan.next(),
        }
    }
}

struct Buffer {
    ready: Vec<Entry>,
    at: usize,
    target: usize,
    tried: bool,
    ended: bool,
    error: Option<SqlError>,
    budget: Arc<MemoryBudget>,
    held: Reservation,
}

impl Buffer {
    fn new(budget: &Arc<MemoryBudget>) -> Option<Self> {
        let (ready, held) = allocated(INITIAL, budget)?;
        Some(Self { ready, at: 0, target: INITIAL, tried: false, ended: false, error: None, budget: Arc::clone(budget), held })
    }

    fn next(&mut self, scan: &mut impl Iterator<Item = Result<Entry>>) -> Option<Result<Entry>> {
        loop {
            if let Some(entry) = self.ready.get(self.at).copied() {
                self.at = self.at.saturating_add(1);
                return Some(Ok(entry));
            }
            if let Some(error) = self.error.take() {
                return Some(Err(error));
            }
            if self.ended {
                return None;
            }
            self.ready.clear();
            self.at = 0;
            self.fill(scan);
            if !self.tried && !self.ended && self.ready.len() == INITIAL {
                self.grow();
                self.fill(scan);
            }
            if self.ready.len() >= SORT_FROM {
                self.ready.sort_unstable_by_key(|entry| entry.1);
            }
        }
    }

    /// A prefetched error follows all earlier candidates. In particular,
    /// stopping after one matching row need not observe a later failure.
    fn fill(&mut self, scan: &mut impl Iterator<Item = Result<Entry>>) {
        while !self.ended && self.ready.len() < self.target {
            match scan.next() {
                Some(Ok(entry)) => self.ready.push(entry),
                Some(Err(error)) => {
                    self.error = Some(error);
                    self.ended = true;
                }
                None => self.ended = true,
            }
        }
    }

    /// Keep the small allocation and its candidates if the larger one is
    /// unavailable. Both allocations are charged during the handover.
    fn grow(&mut self) {
        self.tried = true;
        let Some((mut ready, held)) = allocated(BLOCK, &self.budget) else { return };
        ready.append(&mut self.ready);
        self.ready = ready;
        self.held = held;
        self.target = BLOCK;
    }
}

fn allocated(count: usize, budget: &Arc<MemoryBudget>) -> Option<(Vec<Entry>, Reservation)> {
    let bytes = count.checked_mul(size_of::<Entry>())?;
    let mut held = budget.reserve(bytes, "a spatial candidate read buffer").ok()?;
    let mut ready = Vec::new();
    ready.try_reserve_exact(count).ok()?;
    held.resize(ready.capacity().checked_mul(size_of::<Entry>())?).ok()?;
    Some((ready, held))
}

#[cfg(test)]
mod tests;

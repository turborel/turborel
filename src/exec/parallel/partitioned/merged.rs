//! \[DESIGN\] (RULES.md D1227) The rows of a window's or a set
//! operation's partitions, each partition's held in a list that spills
//! past its share of the grant, merged back as they are read: of the
//! lists' rows not yet given, the least by the keys, a tie taken from the
//! earlier partition, so no more of the rows is held at once than the
//! lists hold in memory and one row of each.

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::Arc;

use crate::error::Result;
use crate::exec::{HeldIter, HeldRows, compare_rows};
use crate::logical::SortKey;
use crate::storage::Row;

/// The partitions' rows, each partition's sorted by `keys`, given merged.
pub(super) struct Merged {
    runs: Vec<HeldIter<Row>>,
    /// Each partition's next row, the least first.
    heads: BinaryHeap<Head>,
    keys: Arc<[SortKey]>,
}

/// A partition's next row in a merge, and the partition's number.
struct Head {
    row: Row,
    run: usize,
    keys: Arc<[SortKey]>,
}

impl Ord for Head {
    /// Reversed, so the heap's greatest is the least row, of the earliest
    /// partition of a tie.
    fn cmp(&self, other: &Self) -> Ordering {
        compare_rows(&self.keys, &other.row, &self.row).then_with(|| other.run.cmp(&self.run))
    }
}

impl PartialOrd for Head {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Head {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Head {}

impl Merged {
    /// The rows of `outs`, each sorted by `keys`.
    ///
    /// # Errors
    ///
    /// As reading a list back from the spill fails.
    pub(super) fn of(outs: Vec<HeldRows<Row>>, keys: &[SortKey]) -> Result<Merged> {
        let runs: Vec<HeldIter<Row>> = outs.into_iter().map(IntoIterator::into_iter).collect();
        let mut merged = Merged { heads: BinaryHeap::with_capacity(runs.len()), runs, keys: Arc::from(keys) };
        for run in 0..merged.runs.len() {
            merged.advance(run)?;
        }
        Ok(merged)
    }

    /// Takes partition `run`'s next row as its head, if it has one.
    ///
    /// # Errors
    ///
    /// As reading the list back fails.
    fn advance(&mut self, run: usize) -> Result<()> {
        if let Some(next) = self.runs.get_mut(run).and_then(Iterator::next) {
            self.heads.push(Head { row: next?, run, keys: Arc::clone(&self.keys) });
        }
        Ok(())
    }
}

impl Iterator for Merged {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Result<Row>> {
        let Head { row, run, .. } = self.heads.pop()?;
        if let Err(e) = self.advance(run) {
            return Some(Err(e));
        }
        Some(Ok(row))
    }
}

// Unit tests: rows merged as read back from lists spilled past their
// share are those of the lists merged in memory (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;

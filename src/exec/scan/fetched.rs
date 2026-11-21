//! \[DESIGN\] (RULES.md D513) The rows an index scan finds, read in
//! batches of its entries (`entries::entry_rows`), so an engine that keeps
//! rows in groups of columns reads the rows of a group together. The
//! batches grow from one entry, doubling to `MOST_ENTRIES`, so a scan of
//! which few rows are asked reads few ahead.

use std::collections::VecDeque;
use std::sync::Arc;

use super::super::entries::{Reading, entry_rows};
use super::read_row;
use crate::catalog::KeyShape;
use crate::error::Result;
use crate::index::Key;
use crate::storage::{Row, RowId, TableStorage};
use crate::txn::TxId;

/// The most entries a batch reads.
const MOST_ENTRIES: usize = 1024;

/// The rows of an index scan's entries, read in batches.
pub(super) struct Fetched<I> {
    entries: I,
    storage: Arc<dyn TableStorage>,
    ix: KeyShape,
    reading: Reading,
    tx: TxId,
    /// How wide a covering scan makes a row of its entry, if it does.
    width: Option<usize>,
    located: bool,
    ready: VecDeque<Result<Row>>,
    /// The entries the next batch reads.
    next: usize,
    done: bool,
}

impl<I: Iterator<Item = Result<(Key, RowId)>>> Fetched<I> {
    /// The rows of `entries` of `storage`'s index `ix` as `tx` sees them,
    /// read by `reading`, made of their entries `width` wide if given, each
    /// followed by its id if `located`.
    pub(super) fn new(
        entries: I,
        (storage, ix, reading): (Arc<dyn TableStorage>, KeyShape, Reading),
        (tx, width, located): (TxId, Option<usize>, bool),
    ) -> Fetched<I> {
        Fetched { entries, storage, ix, reading, tx, width, located, ready: VecDeque::new(), next: 1, done: false }
    }

    /// Reads the next batch of entries' rows into `ready`, an entry's
    /// error after the rows of those before it, the scan then done.
    fn fill(&mut self) {
        let mut wanted = Vec::with_capacity(self.next);
        let mut failed = None;
        while wanted.len() < self.next {
            match self.entries.next() {
                Some(Ok(entry)) => wanted.push(entry),
                Some(Err(e)) => {
                    failed = Some(e);
                    break;
                }
                None => break,
            }
        }
        self.done = wanted.len() < self.next;
        self.next = self.next.saturating_mul(2).min(MOST_ENTRIES);
        let ids: Vec<RowId> = wanted.iter().map(|&(_, id)| id).collect();
        match entry_rows(&*self.storage, (&self.ix, self.width), &mut self.reading, self.tx, &wanted) {
            Ok(rows) => {
                let located = self.located;
                let found = ids.into_iter().zip(rows).filter_map(|(id, row)| row.map(|row| Ok(read_row(located, (id, row)))));
                self.ready.extend(found);
            }
            Err(e) => {
                self.ready.push_back(Err(e));
                self.done = true;
            }
        }
        if let Some(e) = failed {
            self.ready.push_back(Err(e));
        }
    }
}

impl<I: Iterator<Item = Result<(Key, RowId)>>> Iterator for Fetched<I> {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Result<Row>> {
        loop {
            if let Some(row) = self.ready.pop_front() {
                return Some(row);
            }
            if self.done {
                return None;
            }
            self.fill();
        }
    }
}

//! [X3.135-1986 §8.3 GR3b] ORDER BY: rows sorted by each key in turn, most
//! significant first. \[DESIGN\] Rows that fit the work area are sorted in
//! memory; more are sorted externally (RULES.md D38): each work area's worth
//! sorted and written as a run of chunks of vectors, each row beside its first
//! key's prefix (`sort/chunk.rs`, `sort/vectors.rs`), the runs merged by their
//! prefixes, as many at once as the work area reads, in passes until one is
//! left (`sort/merge.rs`). Sorting is stable, a tie going to the earlier run,
//! so rows of equal keys keep their order. Rows are sorted by a prefix of
//! their first key, where it has one (`sort/prefix.rs`). The runs read back
//! look for a cancel of the statement (RULES.md D44). A stream's batches are
//! sorted as they are, no row made but to be given or written
//! (`sort/vectors.rs`, RULES.md D31).

mod chunk;
mod merge;
mod prefix;
mod vectors;

pub(super) use vectors::sort_batches;

use std::cmp::Ordering;
use std::sync::Arc;

use super::work::Work;
use super::{RowIter, broken, failed};
use crate::db::State;
use crate::error::Result;
use crate::logical::SortKey;
use crate::spill::{SpillRun, row_bytes};
use crate::storage::Row;
use crate::txn::TxId;
use merge::{Order, merge};
use vectors::written_rows;

/// [X3.135-1986 §8.3 GR3b] `input` sorted by `keys`, in a work area of
/// `db`, for the statement of `tx`. Never inlined into the caller, whose
/// frame is on the stack while its input is made (RULES.md R-CF-6).
#[inline(never)]
pub(super) fn sort(keys: &[SortKey], input: RowIter, db: &State, tx: TxId) -> RowIter {
    match sorted(keys, input, &mut Work::of(db, tx)) {
        Ok(rows) => rows,
        Err(e) => failed(e),
    }
}

/// `sort`'s work.
fn sorted(keys: &[SortKey], input: RowIter, work: &mut Work) -> Result<RowIter> {
    let mut sorter = Sorter::new(keys.to_vec());
    for r in input {
        let row = r?;
        if let Some(k) = keys.iter().find(|k| row.get(k.column).is_none()) {
            return Err(broken(&format!("sort column {} is not in the rows sorted", k.column)));
        }
        sorter.push(row, work)?;
    }
    sorter.sorted(work)
}

/// Rows being sorted by their keys, given one at a time, held while they
/// fit the work area, each work area's worth then written as a sorted run.
pub(super) struct Sorter {
    keys: Vec<SortKey>,
    rows: Vec<Row>,
    bytes: usize,
    runs: Vec<Box<dyn SpillRun>>,
}

impl Sorter {
    /// Rows to be sorted by `keys`, each of which every row has.
    pub(super) fn new(keys: Vec<SortKey>) -> Sorter {
        Sorter { keys, rows: Vec::new(), bytes: 0, runs: Vec::new() }
    }

    /// Adds `row`, the rows held written to a run of `work` once they
    /// outgrow it.
    pub(super) fn push(&mut self, row: Row, work: &mut Work) -> Result<()> {
        self.bytes = self.bytes.saturating_add(row_bytes(&row));
        self.rows.push(row);
        if self.bytes > work.limit() {
            self.runs.push(written_rows(&self.keys, std::mem::take(&mut self.rows), work)?);
            self.bytes = 0;
        }
        Ok(())
    }

    /// The rows added, sorted: in memory if they fit `work`, else merged
    /// from its runs.
    pub(super) fn sorted(self, work: &mut Work) -> Result<RowIter> {
        let Sorter { keys, rows, mut runs, .. } = self;
        if runs.is_empty() {
            return Ok(Box::new(prefix::sorted(&keys, rows).into_iter().map(Ok)));
        }
        if !rows.is_empty() {
            runs.push(written_rows(&keys, rows, work)?);
        }
        merged(&keys, runs, work)
    }
}

/// The rows of sorted `runs`, merged by `keys`, in passes of as many as
/// `work` reads at once, until one is left.
fn merged(keys: &[SortKey], mut runs: Vec<Box<dyn SpillRun>>, work: &mut Work) -> Result<RowIter> {
    let (order, width) = (Arc::new(Order::new(keys, work.types().clone())), work.width());
    while runs.len() > width {
        let (mut merged, mut rest) = (Vec::new(), runs.into_iter());
        loop {
            let group: Vec<Box<dyn SpillRun>> = rest.by_ref().take(width).collect();
            if group.is_empty() {
                break;
            }
            let mut run = work.run()?;
            merge(&order, group)?.write_to(&mut *run)?;
            merged.push(run);
        }
        runs = merged;
    }
    Ok(Box::new(merge(&order, runs)?))
}

/// [X3.135-1986 §8.3 GR3b; ISO/IEC 9075-2:2023 §10.10 GR1] The order of
/// rows `a` and `b` under `keys`, each key's nulls placed by its null
/// ordering. `sort` has checked that both have every key column; a missing
/// one would compare equal.
pub fn compare_rows(keys: &[SortKey], a: &Row, b: &Row) -> Ordering {
    keys.iter()
        .map(|k| a.get(k.column).zip(b.get(k.column)).map_or(Ordering::Equal, |(x, y)| k.order().cmp(x, y)))
        .find(|o| o.is_ne())
        .unwrap_or(Ordering::Equal)
}

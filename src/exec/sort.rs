//! [X3.135-1986 §8.3 GR3b] ORDER BY: rows sorted by each key in turn, most
//! significant first. \[DESIGN\] Rows that fit the work area are sorted in
//! memory; more are sorted externally (RULES.md D38): each work area's
//! worth sorted and written as a run, the runs merged, as many at once as
//! the work area reads, in passes until one is left. Sorting is stable, a
//! tie going to the earlier run, so rows of equal keys keep their order.
//! Split from exec.rs (RULES.md R-SIZE-2).

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::Arc;

use super::work::Work;
use super::{RowIter, broken, failed};
use crate::db::State;
use crate::error::Result;
use crate::logical::SortKey;
use crate::spill::{Records, SpillRun, record_row, row_bytes, row_record};
use crate::storage::Row;
use crate::value::sort_cmp;
use crate::value::types::TypeRegistry;

/// [X3.135-1986 §8.3 GR3b] `input` sorted by `keys`, in a work area of
/// `db`. Never inlined into the caller, whose frame is on the stack while
/// its input is made (RULES.md R-CF-6).
#[inline(never)]
pub(super) fn sort(keys: &[SortKey], input: RowIter, db: &State) -> RowIter {
    match sorted(keys, input, &mut Work::new(db)) {
        Ok(rows) => rows,
        Err(e) => failed(e),
    }
}

/// `sort`'s work.
fn sorted(keys: &[SortKey], input: RowIter, work: &mut Work) -> Result<RowIter> {
    let keys: Arc<[SortKey]> = Arc::from(keys);
    let (mut rows, mut bytes, mut runs) = (Vec::new(), 0_usize, Vec::new());
    for r in input {
        let row = r?;
        if let Some(k) = keys.iter().find(|k| row.get(k.column).is_none()) {
            return Err(broken(&format!("sort column {} is not in the rows sorted", k.column)));
        }
        bytes = bytes.saturating_add(row_bytes(&row));
        rows.push(row);
        if bytes > work.limit() {
            runs.push(written(&keys, std::mem::take(&mut rows), work)?);
            bytes = 0;
        }
    }
    if runs.is_empty() {
        rows.sort_by(|a, b| compare_rows(&keys, a, b));
        return Ok(Box::new(rows.into_iter().map(Ok)));
    }
    if !rows.is_empty() {
        runs.push(written(&keys, rows, work)?);
    }
    let width = work.width();
    while runs.len() > width {
        let (mut merged, mut rest) = (Vec::new(), runs.into_iter());
        loop {
            let group: Vec<Box<dyn SpillRun>> = rest.by_ref().take(width).collect();
            if group.is_empty() {
                break;
            }
            let mut run = work.run()?;
            for row in merge(&keys, group, work.types())? {
                run.push(&row_record(&row?))?;
            }
            merged.push(run);
        }
        runs = merged;
    }
    Ok(Box::new(merge(&keys, runs, work.types())?))
}

/// `rows`, sorted by `keys`, written as a run of `work`'s spill area.
fn written(keys: &[SortKey], mut rows: Vec<Row>, work: &mut Work) -> Result<Box<dyn SpillRun>> {
    rows.sort_by(|a, b| compare_rows(keys, a, b));
    let mut run = work.run()?;
    for row in &rows {
        run.push(&row_record(row))?;
    }
    Ok(run)
}

/// [X3.135-1986 §8.3 GR3b] The order of rows `a` and `b` under `keys`.
/// `sort` has checked that both have every key column; a missing one would
/// compare equal.
pub fn compare_rows(keys: &[SortKey], a: &Row, b: &Row) -> Ordering {
    keys.iter()
        .map(|k| {
            let o = a.get(k.column).zip(b.get(k.column)).map_or(Ordering::Equal, |(x, y)| sort_cmp(x, y));
            if k.descending { o.reverse() } else { o }
        })
        .find(|o| o.is_ne())
        .unwrap_or(Ordering::Equal)
}

/// The rows of sorted `runs`, merged in order, each run's pages freed as
/// they are read.
fn merge(keys: &Arc<[SortKey]>, runs: Vec<Box<dyn SpillRun>>, types: &TypeRegistry) -> Result<Merge> {
    let mut sources = Vec::new();
    for run in runs {
        sources.push(run.into_records()?);
    }
    let mut merge = Merge { keys: Arc::clone(keys), sources, heads: BinaryHeap::new(), types: types.clone() };
    for source in 0..merge.sources.len() {
        merge.refill(source)?;
    }
    Ok(merge)
}

/// Runs being merged: each one's next row in the heap, first the least.
struct Merge {
    keys: Arc<[SortKey]>,
    sources: Vec<Records>,
    heads: BinaryHeap<Head>,
    types: TypeRegistry,
}

impl Merge {
    /// Puts the next row of `source`, if any, in the heap.
    fn refill(&mut self, source: usize) -> Result<()> {
        let Some(next) = self.sources.get_mut(source).and_then(Iterator::next) else { return Ok(()) };
        let row = record_row(&next?, &self.types)?;
        self.heads.push(Head { row, source, keys: Arc::clone(&self.keys) });
        Ok(())
    }
}

impl Iterator for Merge {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Result<Row>> {
        let Head { row, source, .. } = self.heads.pop()?;
        Some(self.refill(source).map(|()| row))
    }
}

/// A run's next row. The heap's greatest is the least row, of the earliest
/// run among equal ones.
struct Head {
    row: Row,
    source: usize,
    keys: Arc<[SortKey]>,
}

impl Ord for Head {
    fn cmp(&self, other: &Head) -> Ordering {
        compare_rows(&self.keys, &other.row, &self.row).then(other.source.cmp(&self.source))
    }
}

impl PartialOrd for Head {
    fn partial_cmp(&self, other: &Head) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Head {
    fn eq(&self, other: &Head) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Head {}

//! [X3.135-1986 §8.3 GR3b] ORDER BY: rows sorted by each key in turn, most
//! significant first. \[DESIGN\] Rows that fit the work area are sorted in
//! memory; more are sorted externally (RULES.md D38): each work area's worth
//! sorted and written as a run of chunks of vectors, each row beside its
//! normal key (`sort/chunk.rs`, `sort/vectors.rs`), the runs merged by their
//! keys by a tree of losers, as many at once as the work area reads, in
//! passes until one is left (`sort/merge.rs`). Sorting is stable, a tie
//! going to the earlier run, so rows of equal keys keep their order. Rows
//! are sorted by their keys made one string of bytes each, by radix sort if
//! short (`sort/normal.rs`, `sort/keyed.rs`, RULES.md D364); keys of no such
//! form by a prefix of their first key, where it has one (`sort/prefix.rs`). The runs read back
//! look for a cancel of the statement (RULES.md D44). A stream's batches are
//! sorted as they are, no row made but to be given or written
//! (`sort/vectors.rs`, RULES.md D31). A sort of which only the first rows
//! are asked for holds about those alone (`sort/top.rs`).

mod chunk;
mod keyed;
mod merge;
mod normal;
mod out;
mod prefix;
mod top;
mod vectors;

pub(super) use keyed::sorted_rows;
pub(super) use vectors::{rows_of_batches, sort_batches, sorted_morsel, sorted_morsel_batches, sorted_out};

use std::cmp::Ordering;
use std::sync::Arc;

use super::work::Work;
use super::{RowIter, broken, failed};
use crate::db::State;
use crate::error::Result;
use crate::logical::{Most, SortKey};
use crate::spill::{SpillRun, row_bytes};
use crate::storage::{Row, Threshold};
use crate::txn::TxId;
use keyed::Layouts;
use merge::{Order, merge};
use vectors::{written_batches, written_rows};

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

/// The first `most` rows of `input` sorted by `keys`, and of WITH TIES the
/// peers of the last, in a work area of `db`, for the statement of `tx`
/// (`sort/top.rs`).
#[inline(never)]
pub(super) fn first(keys: &[SortKey], input: RowIter, most: Most, db: &State, tx: TxId) -> RowIter {
    match top::top(keys, input, most, &mut Work::of(db, tx)) {
        Ok(rows) => rows,
        Err(e) => failed(e),
    }
}

/// \[DESIGN\] (RULES.md D380) A sort of its input's first rows reads the
/// columns its keys do not late once it keeps fewer than one in this many
/// of the rows estimated.
const LATE_SHARE: usize = 4;

/// Of a sort by `keys` of the first `most` rows of an input of `est` rows
/// estimated, if known: whether the columns its keys do not read are read
/// late (RULES.md D380), and the bound of its first key told its scans, of
/// a key whose nulls sort last (D379).
pub(super) fn topped_of(keys: &[SortKey], most: Most, est: Option<f64>) -> (bool, Option<Arc<Threshold>>) {
    let rows = est.map(|e| crate::spatial::curves::linearize::count(e, usize::MAX >> 1_u32));
    let late = rows.is_none_or(|rows| most.rows.saturating_mul(LATE_SHARE) < rows);
    let first = keys.first().map(SortKey::order).filter(|o| !o.nulls_first);
    (late, first.map(|o| Arc::new(Threshold::new(o.descending))))
}

/// The first `most` rows of `input`, a stream's batches each holding every
/// column of its rows, sorted by `keys`, and of WITH TIES the peers of the
/// last, in a work area of `db`, for the statement of `tx`, raising the
/// bound `bound` of its first key if given (`sort/top/batched.rs`).
#[inline(never)]
pub(super) fn first_batches(
    keys: &[SortKey],
    input: super::batch::Batches,
    most: (Most, Option<Arc<Threshold>>),
    db: &State,
    tx: TxId,
) -> RowIter {
    match top::top_batches(keys, input, most, &mut Work::of(db, tx), db.functions.kernels_shared()) {
        Ok(rows) => rows,
        Err(e) => failed(e),
    }
}

/// The first `most` rows of `input`, a morsel's batches each holding every
/// column of its rows, sorted by `keys`, in a work area of `db` for the
/// statement of `tx`, as a sort of its first rows holds them, so no more
/// are sorted nor made (`sort/top/batched.rs`): a run of such a sort run
/// in parallel.
///
/// # Errors
///
/// As reading the input, or a spill, fails.
pub(super) fn first_of_morsel(keys: &[SortKey], input: super::batch::Batches, most: usize, (db, tx): (&State, TxId)) -> Result<Vec<Row>> {
    let most = (Most { rows: most, ties: false }, None);
    top::top_batches(keys, input, most, &mut Work::of(db, tx), db.functions.kernels_shared())?.collect()
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
    layouts: Layouts,
}

impl Sorter {
    /// Rows to be sorted by `keys`, each of which every row has.
    pub(super) fn new(keys: Vec<SortKey>) -> Sorter {
        Sorter { keys, rows: Vec::new(), bytes: 0, runs: Vec::new(), layouts: Layouts::default() }
    }

    /// Adds `row`, the rows held written to a run of `work` once they
    /// outgrow it.
    pub(super) fn push(&mut self, row: Row, work: &mut Work) -> Result<()> {
        self.bytes = self.bytes.saturating_add(row_bytes(&row));
        self.rows.push(row);
        if self.bytes > work.limit() {
            self.runs.push(written_rows(&self.keys, std::mem::take(&mut self.rows), work, &mut self.layouts)?);
            self.bytes = 0;
        }
        Ok(())
    }

    /// Adds `rows`, a run of a sort run in parallel past its work area,
    /// written to a run of `work` (RULES.md D380).
    pub(super) fn push_run(&mut self, rows: Vec<Row>, work: &mut Work) -> Result<()> {
        if !rows.is_empty() {
            self.runs.push(written_rows(&self.keys, rows, work, &mut self.layouts)?);
        }
        Ok(())
    }

    /// Adds `batches`, a run of a sort run in parallel past its work area,
    /// sorted, written to a run of `work` (RULES.md D380).
    pub(super) fn push_batches(&mut self, batches: Vec<crate::storage::Batch>, work: &mut Work) -> Result<()> {
        if batches.iter().any(|b| b.rows > 0) {
            self.runs.push(written_batches(&self.keys, batches, work, &mut self.layouts)?);
        }
        Ok(())
    }

    /// The rows added, sorted: in memory if they fit `work`, else merged
    /// from its runs.
    pub(super) fn sorted(self, work: &mut Work) -> Result<RowIter> {
        let Sorter { keys, rows, mut runs, mut layouts, .. } = self;
        if runs.is_empty() {
            work.look()?;
            return Ok(looked(Box::new(sorted_rows(&keys, rows).into_iter().map(Ok)), work.stop()));
        }
        if !rows.is_empty() {
            runs.push(written_rows(&keys, rows, work, &mut layouts)?);
        }
        merged(&keys, runs, work)
    }
}

/// \[DESIGN\] The rows a sort held in memory gives between looks for a
/// cancel (RULES.md D380): few, as a cancel counts within the fetch of a
/// cursor it lands in alone (D44), and a look costs a few atomic reads.
const SORT_RUN_ROWS: usize = 1_024;

/// `rows`, a sort's held in memory, given while the statement `stop`
/// cancels, if known, goes on: looked at once every `SORT_RUN_ROWS` rows.
fn looked(rows: RowIter, stop: Option<Arc<crate::txn::Stop>>) -> RowIter {
    Box::new(looked_every(rows, (stop, SORT_RUN_ROWS)))
}

/// `items` given while the statement `stop` cancels, if known, goes on,
/// looked at once every `every` items: at a cancel, its error, then that
/// error again at every ask, as a scan's rows cut short give it
/// (`txn::checked`).
fn looked_every<T: Send + 'static>(
    items: impl Iterator<Item = Result<T>> + Send + 'static,
    (stop, every): (Option<Arc<crate::txn::Stop>>, usize),
) -> impl Iterator<Item = Result<T>> + Send + 'static {
    let (mut items, mut cut, mut n) = (items, None::<crate::error::SqlError>, 0_usize);
    std::iter::from_fn(move || {
        if let Some(e) = &cut {
            return Some(Err(e.clone()));
        }
        let at = n;
        n = n.wrapping_add(1);
        let looked = stop.as_deref().filter(|_| at.is_multiple_of(every.max(1))).map_or(Ok(()), crate::txn::Stop::check);
        if let Err(e) = looked {
            cut = Some(e.clone());
            return Some(Err(e));
        }
        items.next()
    })
}

/// The rows of sorted `runs`, merged by `keys`, in passes of as many as
/// `work` reads at once, until one is left.
fn merged(keys: &[SortKey], mut runs: Vec<Box<dyn SpillRun>>, work: &mut Work) -> Result<RowIter> {
    let (order, width) = (Arc::new(Order::new(keys, work.types().clone(), work.lobs().clone())), work.width());
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

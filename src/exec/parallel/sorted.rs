//! \[DESIGN\] A sort run in parallel (RULES.md D294), as Leis et al.,
//! "Morsel-Driven Parallelism" (2014), §4.5, sort: each morsel's rows
//! sorted by a thread, a run, by their normal keys (RULES.md D364); then
//! the runs cut at keys sampled of them into ranges, each range's rows of
//! every run moved to it, in the runs' order, and sorted again by their
//! keys by a thread of its own, and the ranges given in order. A range's
//! rows are sorted, not merged, since a radix sort of their keys costs
//! less than a merge's comparisons of rows, and no row is copied. Each
//! sort is stable, and a key's peers all fall in one range, so rows of
//! equal keys keep the order they come in, as the serial sort keeps it
//! (`exec/sort.rs`). Of a sort of which only the first rows are asked
//! for, each run keeps those of its own alone.
//!
//! The runs are charged to one work area (RULES.md D38); past it, the sort
//! gives way to the serial one, which spills, its rows read again in the
//! statement's snapshot.

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::{Arc, Mutex};

use super::pipeline::Pipeline;
use super::run::Morsels;
use super::{Held, Limit, Streamed};
use crate::db::State;
use crate::error::Result;
use crate::exec::sort::sorted_rows;
use crate::exec::{RowIter, Runner, broken, compare_rows};
use crate::logical::SortKey;
use crate::physical::Plan;
use crate::spill::row_bytes;
use crate::storage::Row;
use crate::txn::TxId;

/// \[DESIGN\] The ranges a sort's runs are merged in, by thread: more than
/// one each, so a range of more rows than the others delays little.
const RANGES_PER_THREAD: usize = 4;

/// \[DESIGN\] The keys sampled of each run to cut the ranges at.
const SAMPLES_PER_RUN: usize = 64;

/// [X3.135-1986 §8.3 GR3b] The rows of the sort `plan` over a stream,
/// sorted by morsels and merged by ranges on up to `degree` threads.
///
/// # Errors
///
/// As reading the stream fails.
pub(super) fn sorted(plan: &Plan, (db, tx): (&Arc<State>, TxId), degree: usize) -> Result<Held<RowIter>> {
    let Plan::Sort { keys, input, most, .. } = plan else { return Err(broken("not a sort run in parallel")) };
    if most.is_some_and(|m| m.ties) {
        return Ok(Held::GaveWay);
    }
    let most = most.map(|m| m.rows);
    let limit = Limit::of(db);
    let Held::Kept(pipeline) = Pipeline::of(input, (db, tx), &limit)? else { return Ok(Held::GaveWay) };
    let (pipeline, keys, db_for) = (Arc::new(pipeline), Arc::new(keys.clone()), Arc::clone(db));
    let (count, keys_for) = (pipeline.count(), Arc::clone(&keys));
    let work = Box::new(move |m: usize| {
        let mut run = sorted_rows(&keys_for, pipeline.morsel(m, &Runner::new(&db_for, tx))?);
        if let Some(rows) = most {
            run.truncate(rows);
        }
        Ok(run)
    });
    let (mut runs, from) = (Vec::new(), limit.held());
    for (done, run) in (1_usize..).zip(Morsels::start(db, tx, (count, degree), work)) {
        let run = run?;
        if !limit.take(run.iter().map(row_bytes).sum()) || !limit.on_course(from, (done, count)) {
            return Ok(Held::GaveWay);
        }
        runs.push(run);
    }
    let ranges = merged(runs, keys, (db, tx, degree));
    let rows: RowIter = Box::new(Streamed::of(db, tx, ranges));
    Ok(Held::Kept(limit.keeping(match most {
        Some(n) => Box::new(rows.take(n)),
        None => rows,
    })))
}

/// `runs`, each sorted by `keys`, merged by ranges on up to `degree`
/// threads: the rows of each range in order.
fn merged(runs: Vec<Vec<Row>>, keys: Arc<Vec<SortKey>>, (db, tx, degree): (&Arc<State>, TxId, usize)) -> Morsels<Vec<Row>> {
    let ranges = degree.saturating_mul(RANGES_PER_THREAD).max(1);
    let splitters = splitters(&runs, &keys, ranges);
    let mut parts: Vec<Vec<Row>> = (0..=splitters.len()).map(|_| Vec::new()).collect();
    for run in runs {
        // The place in the run past each range's last row.
        let cuts: Vec<usize> = splitters.iter().map(|s| run.partition_point(|r| compare_rows(&keys, r, s) == Ordering::Less)).collect();
        let (mut rows, mut at) = (run.into_iter(), 0_usize);
        for (part, cut) in parts.iter_mut().zip(cuts.into_iter().chain(std::iter::once(usize::MAX))) {
            let taken = cut.saturating_sub(at);
            part.extend(rows.by_ref().take(taken));
            at = at.saturating_add(taken);
        }
    }
    let count = parts.len();
    let parts: Arc<Vec<Mutex<Vec<Row>>>> = Arc::new(parts.into_iter().map(Mutex::new).collect());
    let work = Box::new(move |range: usize| {
        let rows = match parts.get(range) {
            Some(part) => std::mem::take(&mut *crate::sync::lock(part)?),
            None => Vec::new(),
        };
        Ok(sorted_rows(&keys, rows))
    });
    Morsels::start(db, tx, (count, degree), work)
}

/// Rows sampled of `runs` at even places, sorted by `keys`, cut into
/// `ranges` ranges: the first row of each range after the first.
fn splitters(runs: &[Vec<Row>], keys: &[SortKey], ranges: usize) -> Vec<Row> {
    let mut sample: Vec<&Row> = Vec::new();
    for run in runs {
        let step = run.len().div_ceil(SAMPLES_PER_RUN).max(1);
        sample.extend(run.iter().step_by(step));
    }
    sample.sort_by(|a, b| compare_rows(keys, a, b));
    let step = sample.len().div_ceil(ranges).max(1);
    let mut cuts: Vec<Row> = sample.iter().skip(step).step_by(step).map(|r| (*r).clone()).collect();
    // A splitter equal to the one before cuts no range.
    cuts.dedup_by(|a, b| compare_rows(keys, a, b) == Ordering::Equal);
    cuts
}

/// The head of a run in a merge: its next row, and the run's place.
struct Head<'a> {
    row: &'a Row,
    run: usize,
    at: usize,
    keys: &'a [SortKey],
}

impl Ord for Head<'_> {
    /// Reversed, so the heap's greatest is the least row, of the earliest
    /// run of a tie.
    fn cmp(&self, other: &Self) -> Ordering {
        compare_rows(self.keys, other.row, self.row).then_with(|| other.run.cmp(&self.run))
    }
}

impl PartialOrd for Head<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Head<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Head<'_> {}

/// The rows of `outs`, each sorted by `keys`, merged, a tie taken from
/// the earlier.
pub(super) fn merge_by(outs: &[Vec<Row>], keys: &[SortKey]) -> Vec<Row> {
    let slices: Vec<&[Row]> = outs.iter().map(Vec::as_slice).collect();
    merge(&slices, keys)
}

/// The rows of `slices`, each sorted by `keys`, merged, a tie taken from
/// the earlier slice.
fn merge(slices: &[&[Row]], keys: &[SortKey]) -> Vec<Row> {
    let mut out = Vec::with_capacity(slices.iter().map(|s| s.len()).sum());
    let mut heap: BinaryHeap<Head<'_>> =
        slices.iter().enumerate().filter_map(|(run, s)| Some(Head { row: s.first()?, run, at: 0, keys })).collect();
    while let Some(Head { row: least, run, at, .. }) = heap.pop() {
        out.push(least.clone());
        let next = at.saturating_add(1);
        if let Some(row) = slices.get(run).and_then(|s| s.get(next)) {
            heap.push(Head { row, run, at: next, keys });
        }
    }
    out
}

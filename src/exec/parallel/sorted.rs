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
//! A morsel read by batch whose batches hold every column of its rows is
//! sorted as its batches, by keys made of their vectors (RULES.md D380).
//! Of a whole sort its run is held as sorted batches, made rows by the
//! threads once every run is in; of one of its first few rows alone, its
//! run is held as such a sort's of a stream holds it (D365), so only the
//! rows it keeps are sorted and made; of more, the morsel is sorted whole
//! and only those made. Any other is sorted as its rows, read by
//! batch if it is.
//!
//! The runs are charged to one work area (RULES.md D38), a run of batches
//! its vectors' bytes, and its rows' too before they are made. Past it,
//! each run is written to its spill area as the threads give it, and the
//! runs are merged by a tree of losers (D380), so a sort past its work area
//! is still sorted by the threads, and its rows are read once.

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::{Arc, Mutex};

use super::pipeline::Pipeline;
use super::run::Morsels;
use super::{Held, Limit, Streamed};
use crate::db::State;
use crate::error::Result;
use crate::exec::sort::{Sorter, first_of_morsel, rows_of_batches, sorted_morsel, sorted_morsel_batches, sorted_rows};
use crate::exec::work::Work;
use crate::exec::{RowIter, Runner, broken, compare_rows};
use crate::logical::SortKey;
use crate::physical::Plan;
use crate::spill::row_bytes;
use crate::storage::{Batch, Row};
use crate::txn::TxId;

/// \[DESIGN\] The ranges a sort's runs are merged in, by thread: more than
/// one each, so a range of more rows than the others delays little.
const RANGES_PER_THREAD: usize = 4;

/// \[DESIGN\] The most first rows a morsel's run holds as a sort of its
/// first rows does (RULES.md D365), each row past the greatest held not
/// taken; of more, nearly every row is taken, and the morsel is sorted
/// whole by its keys for less.
const FEW_FIRST: usize = crate::storage::BATCH_ROWS;

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
    let each = Box::new(move |m: usize| morsel_run(&pipeline, m, (&keys_for, most), (&db_for, tx)));
    let (mut runs, from) = (Vec::new(), limit.held());
    let mut spilled: Option<(Sorter, Work)> = None;
    for (done, run) in (1_usize..).zip(Morsels::start(db, tx, (count, degree), each)) {
        let run = run?;
        if let Some((sorter, work)) = spilled.as_mut() {
            run.push_to(sorter, work)?;
        } else if !limit.take(run.bytes()) || !limit.on_course(from, (done, count)) {
            spilled = Some(spilled_of(&keys, runs.drain(..).chain(std::iter::once(run)), (db, tx))?);
        } else {
            runs.push(run);
        }
    }
    // The runs held as vectors are made rows to be merged in memory, if
    // those rows fit too.
    if spilled.is_none() && !limit.take(runs.iter().map(Run::made_bytes).sum()) {
        spilled = Some(spilled_of(&keys, runs.drain(..), (db, tx))?);
    }
    if let Some((sorter, mut work)) = spilled {
        let rows = sorter.sorted(&mut work)?;
        return Ok(Held::Kept(match most {
            Some(n) => Box::new(rows.take(n)),
            None => rows,
        }));
    }
    let ranges = merged(made(runs, (db, tx, degree))?, keys, (db, tx, degree));
    let rows: RowIter = Box::new(Streamed::of(db, tx, ranges));
    Ok(Held::Kept(limit.keeping(match most {
        Some(n) => Box::new(rows.take(n)),
        None => rows,
    })))
}

/// Morsel `m` of `pipeline`, sorted by `keys`, its first `most` rows
/// alone if given: by its batches if it is read by batch and they hold
/// every column of its rows, else as its rows (RULES.md D380).
///
/// # Errors
///
/// As reading the morsel fails.
fn morsel_run(pipeline: &Pipeline, m: usize, (keys, most): (&[SortKey], Option<usize>), (db, tx): (&Arc<State>, TxId)) -> Result<Run> {
    let whole = pipeline.batched.as_ref().is_some_and(|b| b.layout.iter().copied().eq(0..b.width));
    if whole {
        let kept = pipeline.morsel_kept(m, (db, tx))?;
        return match most.map(|n| (n, n <= FEW_FIRST)) {
            Some((n, true)) => first_of_morsel(keys, kept, n, (db, tx)).map(Run::Rows),
            Some((n, false)) => sorted_morsel(keys, kept, n, crate::db::kernels::chosen(db)).map(Run::Rows),
            None => sorted_morsel_batches(keys, kept, crate::db::kernels::chosen(db)).map(Run::Batches),
        };
    }
    let mut run = sorted_rows(keys, pipeline.morsel_rows(m, (db, tx), &Runner::new(db, tx))?);
    if let Some(rows) = most {
        run.truncate(rows);
    }
    Ok(Run::Rows(run))
}

/// \[DESIGN\] (RULES.md D380) A morsel's run, sorted: its rows, or, of a
/// whole sort of batches, its rows held as vectors, charged as such, made
/// rows only to be merged in memory.
enum Run {
    /// Its rows.
    Rows(Vec<Row>),
    /// Its rows as batches, in order.
    Batches(Vec<Batch>),
}

impl Run {
    /// The bytes it holds.
    fn bytes(&self) -> usize {
        match self {
            Run::Rows(rows) => rows.iter().map(row_bytes).sum(),
            Run::Batches(batches) => batches.iter().map(Batch::bytes).sum(),
        }
    }

    /// The bytes its rows take more, once made, of one held as vectors.
    fn made_bytes(&self) -> usize {
        let Run::Batches(batches) = self else { return 0 };
        let row = |b: &Batch| b.columns.len().saturating_mul(size_of::<crate::value::Value>()).saturating_add(size_of::<Row>());
        batches.iter().map(|b| b.rows.saturating_mul(row(b))).sum()
    }

    /// Writes it to a run of `sorter`'s, of `work`'s spill area.
    fn push_to(self, sorter: &mut Sorter, work: &mut Work) -> Result<()> {
        match self {
            Run::Rows(rows) => sorter.push_run(rows, work),
            Run::Batches(batches) => sorter.push_batches(batches, work),
        }
    }
}

/// The rows of `runs`, each made on up to `degree` threads of `db` for the
/// statement of `tx`, in order.
///
/// # Errors
///
/// As a thread fails.
fn made(runs: Vec<Run>, (db, tx, degree): (&Arc<State>, TxId, usize)) -> Result<Vec<Vec<Row>>> {
    let count = runs.len();
    let runs: Arc<Vec<Mutex<Option<Run>>>> = Arc::new(runs.into_iter().map(|r| Mutex::new(Some(r))).collect());
    let each = Box::new(move |at: usize| {
        let run = runs.get(at).map(|r| crate::sync::lock(r).map(|mut r| r.take())).transpose()?.flatten();
        Ok(match run {
            Some(Run::Rows(rows)) => rows,
            Some(Run::Batches(batches)) => rows_of_batches(&batches),
            None => Vec::new(),
        })
    });
    Morsels::start(db, tx, (count, degree), each).collect()
}

/// A sorter of the runs of a sort past its work area, `runs` those given
/// so far, each written to a spill area of a work area of `db` for the
/// statement of `tx` (RULES.md D380).
///
/// # Errors
///
/// As writing a run fails.
fn spilled_of(keys: &[SortKey], runs: impl Iterator<Item = Run>, (db, tx): (&Arc<State>, TxId)) -> Result<(Sorter, Work)> {
    let (mut sorter, mut work) = (Sorter::new(keys.to_vec()), Work::of(db, tx));
    for run in runs {
        run.push_to(&mut sorter, &mut work)?;
    }
    Ok((sorter, work))
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

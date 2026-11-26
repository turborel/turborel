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

use super::charged::{Taken, Tally, gather_batches, gather_rows};
use super::pipeline::Pipeline;
use super::run::Morsels;
use super::{Held, Limit, Streamed};
use crate::error::Result;
use crate::exec::Db;
use crate::exec::batch::{Batches, rows_of};
use crate::exec::sort::{Sorter, first_of_morsel, rows_of_batches, sorted_morsel, sorted_morsel_batches, sorted_rows};
use crate::exec::work::Work;
use crate::exec::{RowIter, Runner, broken, compare_rows};
use crate::logical::{Most, SortKey};
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

/// A sort's runs written to its spill area: the sorter, and the work area
/// whose spill area holds them.
type Spilling = (Sorter, Work);

/// [X3.135-1986 §8.3 GR3b] The rows of the sort `plan` over a stream,
/// sorted by morsels and merged by ranges on up to `degree` threads.
///
/// # Errors
///
/// As reading the stream fails.
pub(super) fn sorted(plan: &Plan, (db, tx): (&Db, TxId), degree: usize) -> Result<Held<RowIter>> {
    let Plan::Sort { keys, input, most: asked, .. } = plan else { return Err(broken("not a sort run in parallel")) };
    // Of a sort that asks the last row's peers too, each morsel is sorted
    // whole, as which of its rows are peers of the last given is known
    // only of every run merged (RULES.md D1229).
    let most = asked.filter(|m| !m.ties).map(|m| m.rows);
    let limit = Limit::of(db);
    let Held::Kept(pipeline) = Pipeline::of(input, (db, tx), &limit)? else { return Ok(Held::GaveWay) };
    let (pipeline, keys, db_for, within) = (Arc::new(pipeline), Arc::new(keys.clone()), db.clone(), limit.clone());
    let (count, keys_for) = (pipeline.count(), Arc::clone(&keys));
    let each = Box::new(move |m: usize| morsel_run(&pipeline, m, (&keys_for, most), (&db_for, tx), &within));
    // What it held before its morsels, read before a thread starts on one.
    let from = limit.held();
    let morsels = Morsels::start(db, tx, (count, degree), each);
    let (mut runs, mut spilled) = held_runs(morsels, &keys, (db, tx, count), (&limit, from))?;
    // The runs held as vectors are made rows to be merged in memory, if
    // those rows fit too.
    if spilled.is_none() && !limit.take(runs.iter().map(Run::made_bytes).sum()) {
        spilled = Some(spilled_of(&keys, runs.drain(..), (db, tx))?);
    }
    if let Some((sorter, mut work)) = spilled {
        return Ok(Held::Kept(first_asked(sorter.sorted(&mut work)?, *asked, keys)));
    }
    let ranges = merged(made(runs, (db, tx, degree))?, Arc::clone(&keys), (db, tx, degree));
    let rows: RowIter = Box::new(Streamed::of(db, tx, ranges));
    Ok(Held::Kept(limit.keeping(first_asked(rows, *asked, keys))))
}

/// [ISO/IEC 9075-2:2023 §7.17 GR5, GR6] Of `rows`, sorted by `keys`, the
/// first `asked` rows, if a number of them is asked, and of WITH TIES each
/// row after them that is a peer of the last under the keys; else every
/// row.
fn first_asked(rows: RowIter, asked: Option<Most>, keys: Arc<Vec<SortKey>>) -> RowIter {
    match asked {
        None => rows,
        Some(Most { rows: n, ties: false }) => Box::new(rows.take(n)),
        Some(Most { rows: n, ties: true }) => Box::new(WithTies { rows, left: n, keys, last: None }),
    }
}

/// The first rows of a sort and the peers of the last of them: the rows
/// still to give before the last, and the last given once none are left.
struct WithTies {
    rows: RowIter,
    left: usize,
    keys: Arc<Vec<SortKey>>,
    last: Option<Row>,
}

impl Iterator for WithTies {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Result<Row>> {
        if self.left == 0 && self.last.is_none() {
            return None;
        }
        let row = match self.rows.next()? {
            Ok(row) => row,
            Err(e) => return Some(Err(e)),
        };
        if self.left > 0 {
            self.left = self.left.saturating_sub(1);
            if self.left == 0 {
                self.last = Some(row.clone());
            }
            return Some(Ok(row));
        }
        let peer = self.last.as_ref().is_some_and(|last| compare_rows(&self.keys, last, &row) == Ordering::Equal);
        if !peer {
            self.last = None;
            return None;
        }
        Some(Ok(row))
    }
}

/// The runs of `morsels`, `count` of `pipeline`'s sorted by `keys`, held
/// within `limit`; or, once they outgrow it, written to a sorter's spill
/// area as they come, a morsel that outgrew the grant written as its run
/// if it was sorted, else as its rows, those its thread gathered and the
/// rest of its thread's read, never read again (RULES.md D1150).
///
/// # Errors
///
/// As reading a morsel or writing a run fails.
fn held_runs(
    morsels: Morsels<Made>,
    keys: &[SortKey],
    (db, tx, count): (&Db, TxId, usize),
    (limit, from): (&Limit, usize),
) -> Result<(Vec<Run>, Option<Spilling>)> {
    let (mut runs, mut read) = (Vec::new(), 0_usize);
    let mut spilled: Option<Spilling> = None;
    for (done, made) in (1_usize..).zip(morsels) {
        let run = match made? {
            Made::Whole(run, bytes) => {
                limit.give(bytes);
                run
            }
            Made::Past(past) => {
                let (sorter, work) = match spilled.as_mut() {
                    Some(spilled) => spilled,
                    None => spilled.insert(spilled_of(keys, runs.drain(..), (db, tx))?),
                };
                match past {
                    Past::Sorted(run) => run.push_to(sorter, work)?,
                    Past::Read(rows) => sorter.push_rows(rows, work)?,
                }
                continue;
            }
        };
        if let Some((sorter, work)) = spilled.as_mut() {
            run.push_to(sorter, work)?;
            continue;
        }
        read = read.saturating_add(run.bytes());
        if !limit.take(run.bytes()) || !limit.on_course((from, read), (done, count)) {
            spilled = Some(spilled_of(keys, runs.drain(..).chain(std::iter::once(run)), (db, tx))?);
        } else {
            runs.push(run);
        }
    }
    Ok((runs, spilled))
}

/// Morsel `m` of `pipeline`, sorted by `keys`, its first `most` rows
/// alone if given: by its batches if it is read by batch and they hold
/// every column of its rows, else as its rows (RULES.md D380). A run that
/// holds the morsel whole is charged to `limit` as it is read; past the
/// grant, the morsel is given as its rows, those gathered and the rest of
/// its read, or as its run if that outgrew the grant once sorted (RULES.md
/// D1150).
///
/// # Errors
///
/// As reading the morsel fails.
fn morsel_run(
    pipeline: &Arc<Pipeline>,
    m: usize,
    (keys, most): (&[SortKey], Option<usize>),
    (db, tx): (&Db, TxId),
    limit: &Limit,
) -> Result<Made> {
    let tally = Tally::of(limit);
    let kernel = crate::db::kernels::chosen(db);
    let whole = pipeline.batched.as_ref().filter(|b| b.layout.iter().copied().eq(0..b.width)).map(|b| b.width);
    let run = if let Some(width) = whole {
        let kept = pipeline.morsel_kept(m, (db, tx))?;
        if let Some(n) = most.filter(|&n| n <= FEW_FIRST) {
            Run::Rows(first_of_morsel(keys, kept, n, (db, tx))?)
        } else {
            let (batches, rest) = gather_batches(kept, &tally)?;
            if tally.past() {
                let read: Batches = Box::new(batches.into_iter().map(Ok).chain(rest));
                return Ok(read_past(&tally, rows_of(read, (0..width).collect(), width)));
            }
            let batches: Batches = Box::new(batches.into_iter().map(Ok));
            match most {
                Some(n) => Run::Rows(sorted_morsel(keys, batches, n, kernel)?),
                None => Run::Batches(sorted_morsel_batches(keys, batches, kernel)?),
            }
        }
    } else {
        let (rows, rest) = gather_rows(pipeline.morsel_rows(m, (db, tx), Runner::new(db, tx))?, &tally)?;
        if tally.past() {
            return Ok(read_past(&tally, Box::new(rows.into_iter().map(Ok).chain(rest))));
        }
        let mut run = sorted_rows(keys, rows);
        if let Some(first) = most {
            run.truncate(first);
        }
        Run::Rows(run)
    };
    tally.settle(run.bytes());
    Ok(match tally.taken(()) {
        Taken::Whole((), bytes) => Made::Whole(run, bytes),
        Taken::Past => Made::Past(Past::Sorted(run)),
    })
}

/// A morsel past the grant as it was read, `rows` its rows, its charge
/// given back.
fn read_past(tally: &Tally, rows: RowIter) -> Made {
    tally.given_up();
    Made::Past(Past::Read(rows))
}

/// What a thread made of a sort's morsel: its run, with the bytes it
/// holds of the grant; or what is left of one past the grant.
enum Made {
    Whole(Run, usize),
    Past(Past),
}

/// A sort's morsel past the grant: its run, if it outgrew the grant once
/// sorted; else its rows, those its thread gathered and the rest of its
/// read, for the reader to write as they are read.
enum Past {
    Sorted(Run),
    Read(RowIter),
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
fn made(runs: Vec<Run>, (db, tx, degree): (&Db, TxId, usize)) -> Result<Vec<Vec<Row>>> {
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
fn spilled_of(keys: &[SortKey], runs: impl Iterator<Item = Run>, (db, tx): (&Db, TxId)) -> Result<(Sorter, Work)> {
    let (mut sorter, mut work) = (Sorter::new(keys.to_vec()), Work::of(db, tx));
    for run in runs {
        run.push_to(&mut sorter, &mut work)?;
    }
    Ok((sorter, work))
}

/// `runs`, each sorted by `keys`, merged by ranges on up to `degree`
/// threads: the rows of each range in order.
fn merged(runs: Vec<Vec<Row>>, keys: Arc<Vec<SortKey>>, (db, tx, degree): (&Db, TxId, usize)) -> Morsels<Vec<Row>> {
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

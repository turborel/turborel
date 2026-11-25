//! \[DESIGN\] Grouping and DISTINCT run in parallel (RULES.md D293), as
//! Leis et al., "Morsel-Driven Parallelism" (2014), §4.4, aggregate: each
//! morsel's rows grouped apart, its groups partly made, each set
//! function's state written (`Accumulator::write_state`), and split by
//! their keys into partitions; then each partition's pieces merged by a
//! thread of its own (`Accumulator::merge`), in the order of the morsels,
//! so each set function takes its values in the order it would alone.
//!
//! A pipeline read by batch groups each morsel in the executor's table
//! (`exec/table.rs`), its groups split among the partitions by the high
//! bits of their hashes and handed over whole (`table/made.rs`), each
//! partition merged into a table of its own in waves, each group found by
//! the hash it came with and its states merged slot by slot (`waves.rs`,
//! RULES.md D356).
//!
//! Each group is numbered by its first row, morsel and place, after the
//! seeds, numbered first, and the groups are given in that order, as the
//! serial grouping gives them (`exec/group.rs`), DISTINCT each row the
//! first time it comes: each partition orders its own groups on its
//! thread, and the statement's thread merges the partitions' orders. A
//! filter over the grouping that cannot raise, its HAVING condition, is
//! judged of each partition's groups by the thread that finished them,
//! so only the groups it keeps are put in order (RULES.md D1073). Of a
//! grouping over another whose grouping columns hold its own, the first's
//! groups are split by those columns alone and each partition's thread
//! makes the second's groups of its own (`regrouped.rs`, RULES.md D1113).
//! The pieces a grouping holds are charged to one work grant (RULES.md
//! D38, D298); past it, or bound past it as its first
//! morsels show, it gives way to the serial grouping, which spills, its
//! rows read again in the statement's snapshot.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::sync::{Arc, Mutex, PoisonError};

mod regrouped;
mod waves;

use self::regrouped::Outer;

use super::batched::ByBatch;
use super::pipeline::Pipeline;
use super::probe::{PARTS, part_of};
use super::run::Morsels;
use super::{Held, Limit};
use crate::error::{Result, SqlState};
use crate::exec::Db;
use crate::exec::batch::{Read, Told, read_for};
use crate::exec::group::{Spec, admitted, group_key};
use crate::exec::table::{GroupTable, Seed};
use crate::exec::{RowIter, Runner, broken, eval, eval_cond, warn};
use crate::functions::Accumulator;
use crate::logical::{AggCall, Cond, Truth};
use crate::physical::Plan;
use crate::storage::Row;
use crate::txn::TxId;
use crate::value::Value;

/// \[DESIGN\] The bits of a group's number its row's place in its morsel
/// takes: more than any morsel's rows need.
const PLACE_BITS: u32 = 32;

/// \[DESIGN\] The bytes a piece takes beside its key, values and states:
/// its vectors and number.
const PIECE_BYTES: usize = 96;

/// A group partly made of a morsel's rows: its first row's number, its
/// key, its grouping values and each set function's state written.
struct Piece {
    first: u64,
    key: Vec<u8>,
    values: Row,
    states: Vec<Vec<u8>>,
}

/// A group being made: its first row's number, its key, its grouping
/// values and each set function's state.
type Making = (u64, Vec<u8>, Row, Vec<Box<dyn Accumulator>>);

/// A group being merged: its first row's number, its grouping values and
/// each set function's state.
type Merging = (u64, Row, Vec<Box<dyn Accumulator>>);

/// One partition's pieces, by morsel in order.
type Chunks = Vec<Vec<Piece>>;

/// Each partition's groups, finished with their numbers, in the order of
/// their first rows; or that the grouping gave way.
type Runs = Held<Vec<Vec<(u64, Row)>>>;

/// What a grouping groups: by `Spec`, or, of DISTINCT, by every column;
/// and, of a pipeline read by batch, what it reads of each batch
/// (`exec/batch/groups/morsel.rs`) and the seed its keys are hashed by.
struct Grouping {
    spec: Spec,
    /// \[DESIGN\] (RULES.md D1073) The condition a group is kept by, of
    /// a filter over the grouping, its HAVING, if any.
    having: Option<Cond>,
    distinct: bool,
    read: Option<Read>,
    seed: Seed,
    /// \[DESIGN\] (RULES.md D1113) The grouping over it, of a pair the
    /// part runs whole, which each partition's thread makes of its groups.
    outer: Option<Outer>,
}

impl Grouping {
    /// [X3.135-1986 §5.23 GR1] The groups of `groups` its HAVING
    /// condition is true of, or all of none, judged by the thread that
    /// finished them, so only the groups kept are put in order and given
    /// (RULES.md D1073). The condition cannot raise, so the order it is
    /// judged in shows nowhere (RULES.md D15).
    fn kept(&self, mut groups: Vec<(u64, Row)>, runner: &Runner) -> Result<Vec<(u64, Row)>> {
        let Some(cond) = &self.having else { return Ok(groups) };
        let mut failed = None;
        groups.retain(|(_, row)| match eval_cond(cond, row, runner) {
            Ok(truth) => truth == Truth::True,
            Err(e) => {
                failed.get_or_insert(e);
                false
            }
        });
        failed.map_or(Ok(groups), Err)
    }

    /// \[DESIGN\] (RULES.md D1111) The groups of `table`, finished, that
    /// its HAVING condition is true of, or all of none, as `kept` keeps
    /// them: the condition judged of each group's row before the group
    /// takes a row of its own; in the order of their first rows. Of a
    /// pair, the second grouping's groups of them, then those kept
    /// (`ended`).
    fn kept_of(&self, table: &GroupTable, runner: &Runner) -> Result<Vec<(u64, Row)>> {
        let cond = self.having.as_ref().filter(|_| self.outer.is_none());
        let mut groups = match cond {
            Some(cond) => table.finished_if(|row| Ok(eval_cond(cond, row, runner)? == Truth::True))?,
            None => table.finished()?,
        };
        groups.sort_unstable_by_key(|g| g.0);
        if self.outer.is_none() {
            return Ok(groups);
        }
        self.ended(groups, runner)
    }

    /// The groups a partition gives of `groups`, its own finished, in the
    /// order of their first rows: of a pair, the second grouping's of them
    /// (RULES.md D1113); those its HAVING condition keeps.
    fn ended(&self, groups: Vec<(u64, Row)>, runner: &Runner) -> Result<Vec<(u64, Row)>> {
        let groups = match &self.outer {
            Some(outer) => outer.regrouped(groups, runner)?,
            None => groups,
        };
        self.kept(groups, runner)
    }

    /// The lanes of its keys its groups are split among the partitions
    /// by: of a pair, the second grouping's columns (RULES.md D1113); else
    /// none, each group by its whole key.
    fn lanes(&self) -> &[usize] {
        self.outer.as_ref().map_or(&[], Outer::lanes)
    }

    /// The partition of a group of key `key` and grouping values
    /// `values`: of a pair, by its values of the second grouping's columns
    /// alone (RULES.md D1113).
    fn part(&self, key: &[u8], values: &Row) -> Result<usize> {
        let lanes = self.lanes();
        if lanes.is_empty() {
            return Ok(part_of(key, &self.seed));
        }
        let picked = lanes.iter().map(|&g| values.get(g).cloned().ok_or_else(|| broken("a grouping column past its row")));
        Ok(part_of(&crate::spill::norm_row(&picked.collect::<Result<Row>>()?), &self.seed))
    }

    /// The grouping values of `row`.
    fn values(&self, row: &Row) -> Result<Row> {
        if self.distinct {
            return Ok(row.clone());
        }
        self.spec.groups.iter().map(|&g| row.get(g).cloned().ok_or_else(|| broken("a grouping column past its row"))).collect()
    }
}

/// Whether `plan` is a part this module runs: a grouping, DISTINCT, or a
/// filter over a grouping, its HAVING condition (RULES.md D1073).
pub(super) fn is_one(plan: &Plan) -> bool {
    let grouping = if let Plan::Filter { input, .. } = plan { input } else { plan };
    matches!(grouping, Plan::Aggregate { .. } | Plan::Distinct { .. })
}

/// [X3.135-1986 §5.22 GR2] The groups of the grouping `plan` over a
/// stream, made by morsels on up to `degree` threads; or [X3.135-1986
/// §5.25 GR2] DISTINCT's rows of one.
///
/// # Errors
///
/// As reading the stream, or a set function, fails.
pub(super) fn grouped(plan: &Plan, (db, tx): (&Db, TxId), degree: usize) -> Result<Held<RowIter>> {
    grouped_told(plan, (db, tx), degree, &[])
}

/// \[DESIGN\] (RULES.md D1063) As `grouped`, the grouping's input told
/// `told`, runtime filters of its columns a join above the part tells
/// it: a group they drop gives no row the join keeps.
///
/// # Errors
///
/// As `grouped`.
pub(in crate::exec) fn grouped_told(plan: &Plan, (db, tx): (&Db, TxId), degree: usize, told: &[Told]) -> Result<Held<RowIter>> {
    let (plan, having) = if let Plan::Filter { cond, input, .. } = plan { (&**input, Some(cond.clone())) } else { (plan, None) };
    // Of a pair (RULES.md D1113), the part's pipeline is the first
    // grouping's input, told no filter, as those a join tells it are of
    // the second's columns.
    let (plan, outer, told) = match Outer::of(plan) {
        Some((outer, first)) => (first, Some(outer), &[][..]),
        None => (plan, None, told),
    };
    let seed = db.functions.seed().clone();
    let (grouping, input) = if let Plan::Aggregate { groups, collations, aggs, seeds, input, .. } = plan {
        let spec = Spec { groups: groups.clone(), collations: collations.clone(), aggs: aggs.clone(), seeds: seeds.clone() };
        (Grouping { spec, having, distinct: false, read: None, seed, outer }, input)
    } else if let Plan::Distinct { input, .. } = plan {
        let spec = Spec { groups: Vec::new(), collations: Vec::new(), aggs: Vec::new(), seeds: Vec::new() };
        (Grouping { spec, having, distinct: true, read: None, seed, outer }, input)
    } else {
        return Err(broken("not a grouping run in parallel"));
    };
    let limit = Limit::of(db);
    let Held::Kept(pipeline) = Pipeline::told(input, (db, tx), &limit, told)? else { return Ok(Held::GaveWay) };
    let mut grouping = grouping;
    grouping.read = pipeline.batched.as_ref().and_then(|by| read_of(&grouping, by));
    let grouping = Arc::new(grouping);
    let mut runs = if grouping.read.is_some() {
        let Held::Kept(runs) = waves::waved(pipeline, &grouping, (db, tx, degree), &limit)? else { return Ok(Held::GaveWay) };
        runs
    } else {
        let Held::Kept(runs) = merged_all(pipeline, &grouping, (db, tx, degree), &limit)? else { return Ok(Held::GaveWay) };
        runs
    };
    if runs.iter().all(Vec::is_empty) && grouping.spec.groups.is_empty() && !grouping.distinct {
        // [X3.135-1986 §5.23 GR1, §5.25 GR1] one group even of no rows.
        runs = vec![grouping.kept(vec![(0, finished(fresh(&grouping.spec.aggs), Vec::new())?)], &Runner::new(db, tx))?];
    }
    Ok(Held::Kept(limit.keeping(Box::new(Ordered::of(runs)))))
}

/// The groups of each partition of `grouping`'s pipeline read a row at a
/// time, each finished with its number, in the order of their first rows:
/// its morsels' pieces made on up to `degree` threads, then each
/// partition's merged by a thread; given way past `limit`.
///
/// # Errors
///
/// As reading the pipeline, or a set function, fails.
fn merged_all(pipeline: Pipeline, grouping: &Arc<Grouping>, (db, tx, degree): (&Db, TxId, usize), limit: &Limit) -> Result<Runs> {
    let Held::Kept(chunks) = pieces(pipeline, Arc::clone(grouping), (db, tx, degree), limit)? else { return Ok(Held::GaveWay) };
    let chunks: Arc<Vec<Mutex<Chunks>>> = Arc::new(chunks.into_iter().map(Mutex::new).collect());
    let (merging, db_for) = (Arc::clone(grouping), db.clone());
    let merge = Box::new(move |p: usize| {
        let taken = chunks.get(p).map(|c| std::mem::take(&mut *c.lock().unwrap_or_else(PoisonError::into_inner))).unwrap_or_default();
        merging.ended(merged(&merging, p, taken)?, &Runner::new(&db_for, tx))
    });
    Morsels::start(db, tx, (PARTS, degree), merge).collect::<Result<Vec<_>>>().map(Held::Kept)
}

/// The groups of each partition, each partition's in the order of their
/// first rows, given in that order: of the partitions' first groups not
/// yet given, the earliest, as a merge of sorted runs takes them.
struct Ordered {
    runs: Vec<std::vec::IntoIter<(u64, Row)>>,
    /// Each run's group next given, if any.
    heads: Vec<Option<Row>>,
    /// The first rows' numbers of the heads, each with its run, earliest
    /// first.
    order: BinaryHeap<Reverse<(u64, usize)>>,
}

impl Ordered {
    /// The groups of the runs `runs`, each in order.
    fn of(runs: Vec<Vec<(u64, Row)>>) -> Ordered {
        let mut ordered = Ordered { runs: Vec::new(), heads: Vec::new(), order: BinaryHeap::with_capacity(runs.len()) };
        for (at, run) in runs.into_iter().enumerate() {
            let mut run = run.into_iter();
            let head = run.next().map(|(first, row)| {
                ordered.order.push(Reverse((first, at)));
                row
            });
            ordered.runs.push(run);
            ordered.heads.push(head);
        }
        ordered
    }
}

impl Iterator for Ordered {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Result<Row>> {
        let Reverse((_, at)) = self.order.pop()?;
        let (Some(head), Some(run)) = (self.heads.get_mut(at), self.runs.get_mut(at)) else {
            return Some(Err(broken("a partition's groups are missing")));
        };
        let row = head.take();
        *head = run.next().map(|(first, next)| {
            self.order.push(Reverse((first, at)));
            next
        });
        row.map(Ok)
    }
}

/// The pieces of `pipeline`'s rows under `grouping`, each morsel's made
/// by a thread of up to `degree`, by partition, each partition's by
/// morsel in order; given way past `limit`.
fn pieces(pipeline: Pipeline, grouping: Arc<Grouping>, (db, tx, degree): (&Db, TxId, usize), limit: &Limit) -> Result<Held<Vec<Chunks>>> {
    let (pipeline, db_for) = (Arc::new(pipeline), db.clone());
    let count = pipeline.count();
    let work = Box::new(move |m: usize| {
        let runner = Runner::new(&db_for, tx);
        let rows = pipeline.morsel(m, &runner)?;
        let parts = morsel_pieces(&grouping, (m, rows), &runner)?;
        Ok(parts.into_iter().map(|made| (pieces_bytes(&made), made)).collect::<Vec<_>>())
    });
    let (mut chunks, from): (Vec<Chunks>, usize) = ((0..PARTS).map(|_| Vec::new()).collect(), limit.held());
    for (done, morsel) in (1_usize..).zip(Morsels::start(db, tx, (count, degree), work)) {
        if !handed(morsel?, &mut chunks, limit) {
            return Ok(Held::GaveWay);
        }
        if !limit.on_course(from, (done, count)) {
            return Ok(Held::GaveWay);
        }
    }
    Ok(Held::Kept(chunks))
}

/// Hands a morsel's pieces, by partition, each with the bytes they hold,
/// to the partitions' `chunks`, charged to `limit`: whether they fit it.
fn handed(parts: Vec<(usize, Vec<Piece>)>, chunks: &mut [Chunks], limit: &Limit) -> bool {
    for (part, (bytes, made)) in chunks.iter_mut().zip(parts) {
        if !limit.take(bytes) {
            return false;
        }
        part.push(made);
    }
    true
}

/// The bytes `made`, a morsel's pieces of one partition, hold: counted by
/// the worker that made them, so the statement's thread only adds them up.
fn pieces_bytes(made: &[Piece]) -> usize {
    made.iter()
        .map(|p| {
            PIECE_BYTES
                .saturating_add(p.key.len())
                .saturating_add(crate::spill::row_bytes(&p.values))
                .saturating_add(p.states.iter().map(Vec::len).sum::<usize>())
        })
        .sum()
}

/// \[DESIGN\] (RULES.md D354) What `grouping` reads of each batch of a
/// pipeline read `by` batch, if it reads them so: of DISTINCT, every
/// column of batches holding every column of the rows; else its grouping
/// columns and set functions as a grouping by batch reads them, no seed
/// nor collation among them.
fn read_of(grouping: &Grouping, by: &ByBatch) -> Option<Read> {
    if grouping.distinct {
        let whole = by.layout.iter().copied().eq(0..by.width);
        return whole.then(|| read_for(&by.layout, &[], &by.layout)).flatten();
    }
    let spec = &grouping.spec;
    if !spec.seeds.is_empty() || !spec.collations.is_empty() {
        return None;
    }
    read_for(&spec.groups, &spec.aggs, &by.layout)
}

/// The number of the first row of morsel `m`: after the seeds, numbered
/// first, its morsel's number in the high bits.
fn first_of(grouping: &Grouping, m: usize) -> u64 {
    let seeds = u64::try_from(grouping.spec.seeds.len()).unwrap_or(u64::MAX);
    seeds.saturating_add(u64::try_from(m).unwrap_or(u64::MAX).checked_shl(PLACE_BITS).unwrap_or(u64::MAX))
}

/// The pieces of morsel `m`'s `rows`, by partition, its expressions
/// evaluated by `runner`.
fn morsel_pieces(grouping: &Grouping, (m, rows): (usize, Vec<Row>), runner: &Runner) -> Result<Vec<Vec<Piece>>> {
    let base = first_of(grouping, m);
    let mut found: HashMap<Vec<u8>, usize> = HashMap::new();
    let mut made: Vec<Making> = Vec::new();
    for (place, row) in (0_u64..).zip(rows) {
        let values = grouping.values(&row)?;
        let key = group_key(&grouping.spec, &values)?;
        let at = if let Some(&at) = found.get(&key) {
            at
        } else {
            found.insert(key.clone(), made.len());
            made.push((base.saturating_add(place), key, values, fresh(&grouping.spec.aggs)));
            made.len().saturating_sub(1)
        };
        let states = made.get_mut(at).map(|g| &mut g.3).ok_or_else(|| broken("a group of a morsel is missing"))?;
        feed(&grouping.spec.aggs, states, &row, runner)?;
    }
    let mut parts: Vec<Vec<Piece>> = (0..PARTS).map(|_| Vec::new()).collect();
    for (first, key, values, states) in made {
        let states = states.iter().map(|s| written(s.as_ref())).collect();
        parts.get_mut(grouping.part(&key, &values)?).ok_or_else(|| broken("a partition of a grouping is missing"))?.push(Piece {
            first,
            key,
            values,
            states,
        });
    }
    Ok(parts)
}

/// The state `acc` writes.
fn written(acc: &dyn Accumulator) -> Vec<u8> {
    let mut out = Vec::new();
    acc.write_state(&mut out);
    out
}

/// The set functions of a new group, fed nothing.
fn fresh(aggs: &[AggCall]) -> Vec<Box<dyn Accumulator>> {
    aggs.iter().map(|a| a.func.0.accumulator(a.result.clone())).collect()
}

/// Feeds each set function of `states` its argument of `row`, as the
/// serial grouping does (`exec/group.rs`) of one not DISTINCT nor an
/// inverse distribution function, which a grouping run in parallel has
/// none of (`physical/parallel.rs`).
fn feed(aggs: &[AggCall], states: &mut [Box<dyn Accumulator>], row: &Row, runner: &Runner) -> Result<()> {
    for (a, acc) in aggs.iter().zip(states.iter_mut()) {
        // [ISO/IEC 9075-2:2023 §10.9 GR4] a row its filter is not true of
        // is not aggregated, its argument not computed.
        if !admitted(a.filter.as_ref(), row, runner)? {
            continue;
        }
        let Some(e) = &a.arg else {
            // [X3.135-1986 §5.8 GR4b] COUNT(*) counts rows.
            acc.update(&Value::Null)?;
            continue;
        };
        let v = eval(e, row, runner)?;
        if v.is_null() {
            // [ISO/IEC 9075:1992 §6.5 GR1b] a null eliminated warns.
            warn(runner, SqlState::NULL_VALUE_ELIMINATED);
            continue;
        }
        acc.update(&v)?;
    }
    Ok(())
}

/// The groups of partition `p`, each finished, with its number, in the
/// order of their first rows: its seeds', then its pieces merged in order.
fn merged(grouping: &Grouping, p: usize, chunks: Chunks) -> Result<Vec<(u64, Row)>> {
    let mut groups = merged_pieces(grouping, p, chunks)?;
    groups.sort_unstable_by_key(|g| g.0);
    Ok(groups)
}

/// The groups of partition `p` of pieces: its seeds', then its pieces
/// merged in order, each finished, with its number.
fn merged_pieces(grouping: &Grouping, p: usize, chunks: Chunks) -> Result<Vec<(u64, Row)>> {
    let aggs = &grouping.spec.aggs;
    let mut groups: HashMap<Vec<u8>, Merging> = HashMap::new();
    for (n, seed) in (0_u64..).zip(&grouping.spec.seeds) {
        let key = group_key(&grouping.spec, seed)?;
        if grouping.part(&key, seed)? == p {
            groups.entry(key).or_insert_with(|| (n, seed.clone(), fresh(aggs)));
        }
    }
    for piece in chunks.into_iter().flatten() {
        let Piece { first, key, values, states } = piece;
        let group = groups.entry(key).or_insert_with(|| (first, values, fresh(aggs)));
        for (acc, state) in group.2.iter_mut().zip(&states) {
            acc.merge(state)?;
        }
    }
    groups.into_values().map(|(first, values, states)| Ok((first, finished(states, values)?))).collect()
}

/// The row of a finished group: its grouping values, then each set
/// function's result.
fn finished(states: Vec<Box<dyn Accumulator>>, mut row: Row) -> Result<Row> {
    for acc in states {
        row.push(acc.finish()?);
    }
    Ok(row)
}

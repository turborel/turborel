//! \[DESIGN\] Grouping and DISTINCT run in parallel (RULES.md D293), as
//! Leis et al., "Morsel-Driven Parallelism" (2014), §4.4, aggregate: each
//! morsel's rows grouped apart, its groups partly made, each set
//! function's state written (`Accumulator::write_state`), and split by
//! their keys into partitions; then each partition's pieces merged by a
//! thread of its own (`Accumulator::merge`), in the order of the morsels,
//! so each set function takes its values in the order it would alone.
//!
//! Each group is numbered by its first row, morsel and place, after the
//! seeds, numbered first, and the groups are given in that order, as the
//! serial grouping gives them (`exec/group.rs`), DISTINCT each row the
//! first time it comes. The pieces a grouping holds are charged to one
//! work area (RULES.md D38); past it, it gives way to the serial grouping,
//! which spills, its rows read again in the statement's snapshot.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use super::pipeline::Pipeline;
use super::probe::{PARTS, part_of};
use super::run::Morsels;
use super::{Held, Limit};
use crate::db::State;
use crate::error::{Result, SqlState};
use crate::exec::group::{Spec, admitted, group_key};
use crate::exec::{RowIter, Runner, broken, eval, warn};
use crate::functions::Accumulator;
use crate::logical::AggCall;
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

/// One partition's pieces, by morsel in order.
type Chunks = Vec<Vec<Piece>>;

/// What a grouping groups: by `Spec`, or, of DISTINCT, by every column.
struct Grouping {
    spec: Spec,
    distinct: bool,
}

impl Grouping {
    /// The grouping values of `row`.
    fn values(&self, row: &Row) -> Result<Row> {
        if self.distinct {
            return Ok(row.clone());
        }
        self.spec.groups.iter().map(|&g| row.get(g).cloned().ok_or_else(|| broken("a grouping column past its row"))).collect()
    }
}

/// [X3.135-1986 §5.22 GR2] The groups of the grouping `plan` over a
/// stream, made by morsels on up to `degree` threads; or [X3.135-1986
/// §5.25 GR2] DISTINCT's rows of one.
///
/// # Errors
///
/// As reading the stream, or a set function, fails.
pub(super) fn grouped(plan: &Plan, (db, tx): (&Arc<State>, TxId), degree: usize) -> Result<Held<RowIter>> {
    let (grouping, input) = if let Plan::Aggregate { groups, collations, aggs, seeds, input, .. } = plan {
        let spec = Spec { groups: groups.clone(), collations: collations.clone(), aggs: aggs.clone(), seeds: seeds.clone() };
        (Grouping { spec, distinct: false }, input)
    } else if let Plan::Distinct { input } = plan {
        let spec = Spec { groups: Vec::new(), collations: Vec::new(), aggs: Vec::new(), seeds: Vec::new() };
        (Grouping { spec, distinct: true }, input)
    } else {
        return Err(broken("not a grouping run in parallel"));
    };
    let limit = Limit::of(db);
    let Held::Kept(pipeline) = Pipeline::of(input, (db, tx), &limit)? else { return Ok(Held::GaveWay) };
    let grouping = Arc::new(grouping);
    let Held::Kept(chunks) = pieces(pipeline, Arc::clone(&grouping), (db, tx, degree), &limit)? else { return Ok(Held::GaveWay) };
    let chunks: Arc<Vec<Mutex<Chunks>>> = Arc::new(chunks.into_iter().map(Mutex::new).collect());
    let merging = Arc::clone(&grouping);
    let merge = Box::new(move |p: usize| {
        let taken = chunks.get(p).map(|c| std::mem::take(&mut *c.lock().unwrap_or_else(PoisonError::into_inner))).unwrap_or_default();
        merged(&merging, p, taken)
    });
    let mut groups: Vec<(u64, Row)> = Vec::new();
    for part in Morsels::start(db, tx, (PARTS, degree), merge) {
        groups.extend(part?);
    }
    if groups.is_empty() && grouping.spec.groups.is_empty() && !grouping.distinct {
        // [X3.135-1986 §5.23 GR1, §5.25 GR1] one group even of no rows.
        groups.push((0, finished(fresh(&grouping.spec.aggs), Vec::new())?));
    }
    groups.sort_unstable_by_key(|g| g.0);
    Ok(Held::Kept(Box::new(groups.into_iter().map(|(_, row)| Ok(row)))))
}

/// The pieces of `pipeline`'s rows under `grouping`, each morsel's made
/// by a thread of up to `degree`, by partition, each partition's by
/// morsel in order; given way past `limit`.
fn pieces(pipeline: Pipeline, grouping: Arc<Grouping>, (db, tx, degree): (&Arc<State>, TxId, usize), limit: &Limit) -> Result<Held<Vec<Chunks>>> {
    let (pipeline, db_for) = (Arc::new(pipeline), Arc::clone(db));
    let count = pipeline.count();
    let work = Box::new(move |m: usize| {
        let runner = Runner::new(&db_for, tx);
        let rows = pipeline.morsel(m, &runner)?;
        morsel_pieces(&grouping, (m, rows), &runner)
    });
    let mut chunks: Vec<Chunks> = (0..PARTS).map(|_| Vec::new()).collect();
    for split in Morsels::start(db, tx, (count, degree), work) {
        for (part, made) in chunks.iter_mut().zip(split?) {
            let bytes = made.iter().map(|p| PIECE_BYTES + p.key.len() + crate::spill::row_bytes(&p.values) + p.states.iter().map(Vec::len).sum::<usize>()).sum();
            if !limit.take(bytes) {
                return Ok(Held::GaveWay);
            }
            part.push(made);
        }
    }
    Ok(Held::Kept(chunks))
}

/// The pieces of morsel `m`'s `rows`, by partition, its expressions
/// evaluated by `runner`.
fn morsel_pieces(grouping: &Grouping, (m, rows): (usize, Vec<Row>), runner: &Runner) -> Result<Vec<Vec<Piece>>> {
    let seeds = u64::try_from(grouping.spec.seeds.len()).unwrap_or(u64::MAX);
    let base = seeds.saturating_add(u64::try_from(m).unwrap_or(u64::MAX).checked_shl(PLACE_BITS).unwrap_or(u64::MAX));
    let mut found: HashMap<Vec<u8>, usize> = HashMap::new();
    let mut made: Vec<(u64, Vec<u8>, Row, Vec<Box<dyn Accumulator>>)> = Vec::new();
    for (place, row) in (0_u64..).zip(rows) {
        let values = grouping.values(&row)?;
        let key = group_key(&grouping.spec, &values)?;
        let at = match found.get(&key) {
            Some(&at) => at,
            None => {
                found.insert(key.clone(), made.len());
                made.push((base.saturating_add(place), key, values, fresh(&grouping.spec.aggs)));
                made.len().saturating_sub(1)
            }
        };
        let states = made.get_mut(at).map(|g| &mut g.3).ok_or_else(|| broken("a group of a morsel is missing"))?;
        feed(&grouping.spec.aggs, states, &row, runner)?;
    }
    let mut parts: Vec<Vec<Piece>> = (0..PARTS).map(|_| Vec::new()).collect();
    for (first, key, values, states) in made {
        let states = states.iter().map(|s| written(s.as_ref())).collect();
        parts.get_mut(part_of(&key)).ok_or_else(|| broken("a partition of a grouping is missing"))?.push(Piece { first, key, values, states });
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

/// The groups of partition `p`: its seeds', then its pieces merged in
/// order, each finished, with its number.
fn merged(grouping: &Grouping, p: usize, chunks: Chunks) -> Result<Vec<(u64, Row)>> {
    let aggs = &grouping.spec.aggs;
    let mut groups: HashMap<Vec<u8>, (u64, Row, Vec<Box<dyn Accumulator>>)> = HashMap::new();
    for (n, seed) in (0_u64..).zip(&grouping.spec.seeds) {
        let key = group_key(&grouping.spec, seed)?;
        if part_of(&key) == p {
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

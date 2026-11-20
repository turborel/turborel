//! \[DESIGN\] (RULES.md D355, D356) A grouping run in parallel over a
//! pipeline read by batch, merged in waves: each morsel grouped in the
//! executor's table and handed over whole (`exec/table/made.rs`), and once
//! the morsels handed over hold half the room left of the grant, the
//! partitions merge them, each on a thread of its own, into a table of
//! their own, the morsels in order. Between waves a partition's table is
//! frozen as a morsel's groups are, its columns of states as they are, so
//! another thread takes it up. The grant then holds the partitions' tables
//! in place of the morsels merged, so a grouping of many groups holds what
//! its groups hold and one wave, not every morsel's groups at once. The
//! last wave finishes each partition's groups, in the order of their first
//! rows.

use std::sync::{Arc, Mutex, PoisonError};

use super::super::pipeline::Pipeline;
use super::super::probe::PARTS;
use super::super::run::Morsels;
use super::super::{Held, Limit};
use super::{Grouping, Runs, first_of};
use crate::db::State;
use crate::error::Result;
use crate::exec::batch::morsel_groups;
use crate::exec::table::{GroupTable, Made};
use crate::exec::{Runner, broken};
use crate::storage::Row;
use crate::txn::TxId;

/// A partition's groups between waves, frozen; none before its first.
type Frozen = Arc<Vec<Mutex<Option<Made>>>>;

/// The groups of each partition of `grouping`'s pipeline, each finished
/// with its number, in the order of their first rows, made on up to
/// `degree` threads; given way past `limit`.
///
/// # Errors
///
/// As reading the pipeline, or a set function, fails.
pub(super) fn waved(
    pipeline: Pipeline,
    grouping: &Arc<Grouping>,
    (db, tx, degree): (&Arc<State>, TxId, usize),
    limit: &Limit,
) -> Result<Runs> {
    let (pipeline, db_for, making) = (Arc::new(pipeline), Arc::clone(db), Arc::clone(grouping));
    let count = pipeline.count();
    let work = Box::new(move |m: usize| {
        let runner = Runner::new(&db_for, tx);
        let read = making.read.as_ref().ok_or_else(|| broken("a grouping by batch reads no batch"))?;
        let batches = pipeline.morsel_kept(m, (&db_for, tx))?;
        let made = morsel_groups(read, &making.spec.aggs, batches, (&runner, first_of(&making, m), &making.seed))?.made(PARTS)?;
        let bytes = made.bytes();
        Ok((Arc::new(made), bytes))
    });
    let frozen: Frozen = Arc::new((0..PARTS).map(|_| Mutex::new(None)).collect());
    let (mut wave, mut wave_bytes, mut parts_bytes) = (Vec::new(), 0_usize, 0_usize);
    for morsel in Morsels::start(db, tx, (count, degree), work) {
        let (made, bytes) = morsel?;
        if !limit.take(bytes) {
            return Ok(Held::GaveWay);
        }
        wave.push(made);
        wave_bytes = wave_bytes.saturating_add(bytes);
        if wave_bytes.saturating_mul(2) < limit.most().saturating_sub(parts_bytes) {
            continue;
        }
        let held = frozen_wave((grouping, &frozen), std::mem::take(&mut wave), (db, tx, degree))?;
        limit.give(wave_bytes.saturating_add(parts_bytes));
        (wave_bytes, parts_bytes) = (0, held);
        if !limit.take(held) {
            return Ok(Held::GaveWay);
        }
    }
    finished_wave((grouping, &frozen), wave, (db, tx, degree)).map(Held::Kept)
}

/// The table of partition `p` of `grouping`: its groups `frozen` after the
/// waves before, taken up, then the morsels of `wave` merged in order.
///
/// # Errors
///
/// As `Accumulator::merge`; a sum past 128 bits.
fn thawed_and_merged(grouping: &Grouping, (frozen, p): (&Frozen, usize), wave: &[Arc<Made>]) -> Result<GroupTable> {
    let read = grouping.read.as_ref().ok_or_else(|| broken("a grouping by batch reads no batch"))?;
    let held = frozen.get(p).and_then(|slot| slot.lock().unwrap_or_else(PoisonError::into_inner).take());
    let mut table = match held {
        Some(made) => GroupTable::thawed(made, &grouping.spec.aggs, &grouping.seed)?,
        None => GroupTable::new(read.groups.len(), &grouping.spec.aggs, &grouping.seed, usize::MAX),
    };
    for made in wave {
        if made.groups_of(p) > 0 {
            table.absorb(made, p)?;
        }
    }
    Ok(table)
}

/// Merges the morsels of `wave` into each partition's table on up to
/// `degree` threads, each frozen again: the bytes the tables hold.
///
/// # Errors
///
/// As merging a partition fails.
fn frozen_wave(
    (grouping, frozen): (&Arc<Grouping>, &Frozen),
    wave: Vec<Arc<Made>>,
    (db, tx, degree): (&Arc<State>, TxId, usize),
) -> Result<usize> {
    let (grouping, kept, wave) = (Arc::clone(grouping), Arc::clone(frozen), Arc::new(wave));
    let freeze = Box::new(move |p: usize| {
        let made = thawed_and_merged(&grouping, (&kept, p), &wave)?.frozen()?;
        let bytes = made.bytes();
        if let Some(slot) = kept.get(p) {
            *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(made);
        }
        Ok(bytes)
    });
    Morsels::start(db, tx, (PARTS, degree), freeze).try_fold(0_usize, |sum, bytes| Ok(sum.saturating_add(bytes?)))
}

/// Merges the morsels of the last `wave` into each partition's table on up
/// to `degree` threads: each partition's groups finished, with their
/// numbers, in the order of their first rows.
///
/// # Errors
///
/// As merging or finishing a partition fails.
fn finished_wave(
    (grouping, frozen): (&Arc<Grouping>, &Frozen),
    wave: Vec<Arc<Made>>,
    (db, tx, degree): (&Arc<State>, TxId, usize),
) -> Result<Vec<Vec<(u64, Row)>>> {
    let (grouping, kept, wave) = (Arc::clone(grouping), Arc::clone(frozen), Arc::new(wave));
    let finish = Box::new(move |p: usize| {
        let mut groups = thawed_and_merged(&grouping, (&kept, p), &wave)?.finished()?;
        groups.sort_unstable_by_key(|g| g.0);
        Ok(groups)
    });
    Morsels::start(db, tx, (PARTS, degree), finish).collect()
}

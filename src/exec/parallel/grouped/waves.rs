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
//!
//! A morsel of few groups beside its rows, as few as the statement's
//! thread merges while the others group the next morsels, is merged there
//! straight into one table, with no partition's thread to wake nor table
//! to make; the morsels after it of many groups take that table to the
//! partitions first, so each partition still merges its morsels in order.

use std::sync::{Arc, Mutex, PoisonError};

use super::super::pipeline::Pipeline;
use super::super::probe::PARTS;
use super::super::run::Morsels;
use super::super::{Held, Limit};
use super::{Grouping, Runs, first_of};
use crate::error::Result;
use crate::exec::Db;
use crate::exec::batch::morsel_groups;
use crate::exec::table::{GroupTable, Made};
use crate::exec::{Runner, broken};
use crate::storage::Row;
use crate::txn::TxId;

/// \[DESIGN\] The rows of a morsel each of its groups stands for, on each
/// thread, at least, for the statement's thread to merge the morsel's
/// table itself: merging a group costs about what grouping a row or two
/// does, so the statement's thread merges a morsel's groups in the time the
/// threads group the rows of the next morsel each.
const DIRECT_ROWS: usize = 2;

/// A partition's groups between waves, frozen; none before its first.
type Frozen = Arc<Vec<Mutex<Option<Made>>>>;

/// What a morsel hands over: its groups whole, few beside its rows, for
/// the statement's thread to merge; or split among the partitions.
enum Handed {
    Whole(Made),
    Parted(Arc<Made>),
}

/// A grouping's waves as its morsels come: the partitions' groups frozen,
/// the wave not yet merged, the table the statement's thread merges, and
/// the bytes each holds of the grant.
struct Waves<'a> {
    grouping: &'a Arc<Grouping>,
    run: (&'a Db, TxId, usize),
    limit: &'a Limit,
    frozen: Frozen,
    wave: Vec<Arc<Made>>,
    wave_bytes: usize,
    parts_bytes: usize,
    /// Whether the partitions merged a wave.
    waved: bool,
    direct: Option<GroupTable>,
    direct_bytes: usize,
}

/// The groups of each partition of `grouping`'s pipeline, each finished
/// with its number, in the order of their first rows, made on up to
/// `degree` threads; given way past `limit`.
///
/// # Errors
///
/// As reading the pipeline, or a set function, fails.
pub(super) fn waved(pipeline: Pipeline, grouping: &Arc<Grouping>, (db, tx, degree): (&Db, TxId, usize), limit: &Limit) -> Result<Runs> {
    let (pipeline, db_for, making) = (Arc::new(pipeline), db.clone(), Arc::clone(grouping));
    let count = pipeline.count();
    let work = Box::new(move |m: usize| {
        let runner = Runner::new(&db_for, tx);
        let read = making.read.as_ref().ok_or_else(|| broken("a grouping by batch reads no batch"))?;
        let mut batches = pipeline.morsel_kept(m, (&db_for, tx))?;
        // A groupjoin's probe rows, each tagged with its group (`joined.rs`).
        if let Some(tag) = &making.tag {
            batches = super::joined::tagged(tag, batches, &db_for);
        }
        let table = morsel_groups(read, &making.spec.aggs, batches, (&runner, first_of(&making, m), &making.seed))?;
        if table.len().saturating_mul(DIRECT_ROWS).saturating_mul(degree) <= table.taken {
            return table.frozen().map(Handed::Whole);
        }
        table.made(PARTS).map(|made| Handed::Parted(Arc::new(made)))
    });
    let frozen: Frozen = Arc::new((0..PARTS).map(|_| Mutex::new(None)).collect());
    let mut waves = Waves {
        grouping,
        run: (db, tx, degree),
        limit,
        frozen,
        wave: Vec::new(),
        wave_bytes: 0,
        parts_bytes: 0,
        waved: false,
        direct: None,
        direct_bytes: 0,
    };
    for morsel in Morsels::start(db, tx, (count, degree), work) {
        let held = match morsel? {
            Handed::Whole(made) => waves.direct(&made)?,
            Handed::Parted(made) => waves.flushed()? && waves.parted(made)?,
        };
        if !held {
            return Ok(Held::GaveWay);
        }
    }
    waves.finished()
}

impl Waves<'_> {
    /// Merges `made`, a morsel's groups whole, into the statement thread's
    /// table: whether the grant holds what that table holds.
    ///
    /// # Errors
    ///
    /// As `GroupTable::absorb_whole`.
    fn direct(&mut self, made: &Made) -> Result<bool> {
        let grouping = self.grouping;
        let width = grouping.read.as_ref().map_or(0, |read| read.groups.len());
        let table = self.direct.get_or_insert_with(|| GroupTable::new(width, &grouping.spec.aggs, &grouping.seed, usize::MAX));
        table.absorb_whole(made)?;
        let bytes = table.bytes();
        let more = bytes.saturating_sub(self.direct_bytes);
        self.direct_bytes = bytes;
        Ok(self.limit.take(more))
    }

    /// The statement thread's table, if any, handed to the partitions as a
    /// morsel's groups are, before the morsels after it: whether the grant
    /// holds it.
    ///
    /// # Errors
    ///
    /// As `GroupTable::made`, or merging a wave.
    fn flushed(&mut self) -> Result<bool> {
        let Some(table) = self.direct.take() else { return Ok(true) };
        let made = table.made(PARTS)?;
        self.limit.give(self.direct_bytes);
        self.direct_bytes = 0;
        self.parted(Arc::new(made))
    }

    /// Adds `made`, split among the partitions, to the wave, and merges the
    /// wave once it holds half the room left of the grant: whether the
    /// grant holds what is held.
    ///
    /// # Errors
    ///
    /// As merging a partition fails.
    fn parted(&mut self, made: Arc<Made>) -> Result<bool> {
        let bytes = made.bytes();
        if !self.limit.take(bytes) {
            return Ok(false);
        }
        self.wave.push(made);
        self.wave_bytes = self.wave_bytes.saturating_add(bytes);
        if self.wave_bytes.saturating_mul(2) < self.limit.most().saturating_sub(self.parts_bytes) {
            return Ok(true);
        }
        let held = frozen_wave((self.grouping, &self.frozen), std::mem::take(&mut self.wave), self.run)?;
        self.limit.give(self.wave_bytes.saturating_add(self.parts_bytes));
        (self.wave_bytes, self.parts_bytes, self.waved) = (0, held, true);
        Ok(self.limit.take(held))
    }

    /// Each partition's groups finished, in the order of their first rows:
    /// of the statement thread's table alone if no morsel went to the
    /// partitions, else of the last wave, that table in it.
    ///
    /// # Errors
    ///
    /// As finishing a group, or merging a partition, fails.
    fn finished(mut self) -> Result<Runs> {
        if !self.waved && self.wave.is_empty() {
            let Some(table) = self.direct.take() else { return Ok(Held::Kept(Vec::new())) };
            let mut groups = table.finished()?;
            groups.sort_unstable_by_key(|g| g.0);
            return Ok(Held::Kept(vec![groups]));
        }
        if !self.flushed()? {
            return Ok(Held::GaveWay);
        }
        finished_wave((self.grouping, &self.frozen), self.wave, self.run).map(Held::Kept)
    }
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
fn frozen_wave((grouping, frozen): (&Arc<Grouping>, &Frozen), wave: Vec<Arc<Made>>, (db, tx, degree): (&Db, TxId, usize)) -> Result<usize> {
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
    (db, tx, degree): (&Db, TxId, usize),
) -> Result<Vec<Vec<(u64, Row)>>> {
    let (grouping, kept, wave) = (Arc::clone(grouping), Arc::clone(frozen), Arc::new(wave));
    let finish = Box::new(move |p: usize| {
        let mut groups = thawed_and_merged(&grouping, (&kept, p), &wave)?.finished()?;
        groups.sort_unstable_by_key(|g| g.0);
        Ok(groups)
    });
    Morsels::start(db, tx, (PARTS, degree), finish).collect()
}

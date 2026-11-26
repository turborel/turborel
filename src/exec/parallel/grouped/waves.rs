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
//! A morsel's groups of a value no other morsel's rows can hold are
//! finished by the thread that grouped it, and go to no partition
//! (`owned.rs`).
//!
//! A morsel of few groups beside its rows, as few as the statement's
//! thread merges while the others group the next morsels, is merged there
//! straight into one table, with no partition's thread to wake nor table
//! to make; the morsels after it of many groups take that table to the
//! partitions first, so each partition still merges its morsels in order.

use std::sync::atomic::{AtomicUsize, Ordering};
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
use crate::spill::row_bytes;
use crate::storage::Row;
use crate::txn::TxId;

use self::owned::Owned;

mod owned;

/// \[DESIGN\] The rows of a morsel each of its groups stands for, on each
/// thread, at least, for the statement's thread to merge the morsel's
/// table itself: merging a group costs about what grouping a row or two
/// does, so the statement's thread merges a morsel's groups in the time the
/// threads group the rows of the next morsel each.
const DIRECT_ROWS: usize = 2;

/// A partition's groups between waves, frozen; none before its first.
type Frozen = Arc<Vec<Mutex<Option<Made>>>>;

/// What a morsel hands over: its groups whole, few beside its rows, for
/// the statement's thread to merge; or split among the partitions; or
/// those it owns finished, and the others, if any, split among them.
enum Handed {
    Whole(Made),
    Parted(Arc<Made>),
    Owned(Vec<(u64, Row)>, Option<Arc<Made>>),
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
    /// The groups the morsels owned, finished, in the order of their first
    /// rows, and the bytes they hold.
    owned: Vec<(u64, Row)>,
    owned_bytes: usize,
}

/// The groups of each partition of `grouping`'s pipeline, each finished
/// with its number, in the order of their first rows, made on up to
/// `degree` threads; given way past `limit`.
///
/// # Errors
///
/// As reading the pipeline, or a set function, fails.
pub(super) fn waved(pipeline: Pipeline, grouping: &Arc<Grouping>, (db, tx, degree): (&Db, TxId, usize), limit: &Limit) -> Result<Runs> {
    let owned = Owned::of(&pipeline, grouping);
    let (pipeline, db_for, making) = (Arc::new(pipeline), db.clone(), Arc::clone(grouping));
    let count = pipeline.count();
    // The groups the morsel last grouped made, the room the next is given.
    let room = AtomicUsize::new(0);
    let work = Box::new(move |m: usize| {
        let runner = Runner::new(&db_for, tx);
        let read = making.read.as_ref().ok_or_else(|| broken("a grouping by batch reads no batch"))?;
        let batches = pipeline.morsel_kept(m, (&db_for, tx))?;
        let expected = room.load(Ordering::Relaxed);
        let table = morsel_groups(read, &making.spec.aggs, batches, (&runner, first_of(&making, m), &making.seed), expected)?;
        room.store(table.len(), Ordering::Relaxed);
        handed(&making, (m, table, degree), owned.as_ref(), &runner)
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
        owned: Vec::new(),
        owned_bytes: 0,
    };
    for morsel in Morsels::start(db, tx, (count, degree), work) {
        let held = match morsel? {
            Handed::Whole(made) => waves.direct(&made)?,
            Handed::Parted(made) => waves.flushed()? && waves.parted(made)?,
            Handed::Owned(finished, left) => {
                waves.owned(finished)
                    && match left {
                        Some(made) => waves.flushed()? && waves.parted(made)?,
                        None => true,
                    }
            }
        };
        if !held {
            return Ok(Held::GaveWay);
        }
    }
    waves.finished()
}

/// What morsel `m` hands over of `table`, its groups, grouped on one of
/// `degree` threads: whole, if few beside its rows; else those it owns of
/// `owned` finished, and the others split among the partitions.
///
/// # Errors
///
/// As finishing a group, or making the morsel's groups, fails.
fn handed(grouping: &Grouping, (m, table, degree): (usize, GroupTable, usize), owned: Option<&Owned>, runner: &Runner) -> Result<Handed> {
    if table.len().saturating_mul(DIRECT_ROWS).saturating_mul(degree) <= table.taken {
        return table.frozen().map(Handed::Whole);
    }
    if let Some(mine) = owned.and_then(|o| o.of_morsel(m, &table)) {
        let finished = grouping.kept_among(&table, &mine, runner)?;
        let left = mine.iter().any(|&own| !own).then(|| table.made_of(PARTS, grouping.lanes(), &mine)).transpose()?;
        return Ok(Handed::Owned(finished, left.map(Arc::new)));
    }
    table.made(PARTS, grouping.lanes()).map(|made| Handed::Parted(Arc::new(made)))
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

    /// Adds `finished`, the groups a morsel owned, to those owned before,
    /// in the order of their first rows as the morsels come in order:
    /// whether the grant holds them.
    fn owned(&mut self, finished: Vec<(u64, Row)>) -> bool {
        let bytes: usize = finished.iter().map(|(_, row)| row_bytes(row).saturating_add(size_of::<(u64, Row)>())).sum();
        self.owned_bytes = self.owned_bytes.saturating_add(bytes);
        self.owned.extend(finished);
        self.limit.take(bytes)
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
        let made = table.made(PARTS, self.grouping.lanes())?;
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
    /// partitions, else of the last wave, that table in it; and the groups
    /// the morsels owned, one more run.
    ///
    /// # Errors
    ///
    /// As finishing a group, or merging a partition, fails.
    fn finished(mut self) -> Result<Runs> {
        let owned = std::mem::take(&mut self.owned);
        let mut runs = if !self.waved && self.wave.is_empty() {
            match self.direct.take() {
                Some(table) => vec![self.grouping.kept_of(&table, &Runner::new(self.run.0, self.run.1))?],
                None => Vec::new(),
            }
        } else {
            if !self.flushed()? {
                return Ok(Held::GaveWay);
            }
            finished_wave((self.grouping, &self.frozen), self.wave, self.run)?
        };
        if !owned.is_empty() {
            runs.push(owned);
        }
        Ok(Held::Kept(runs))
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
    // Each of the wave's groups may be new, so room for all is made once.
    if let Some(first) = wave.first() {
        table.reserve(first, wave.iter().map(|made| made.groups_of(p)).sum());
    }
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
    let (grouping, kept, wave, db_for) = (Arc::clone(grouping), Arc::clone(frozen), Arc::new(wave), db.clone());
    let finish = Box::new(move |p: usize| {
        let table = thawed_and_merged(&grouping, (&kept, p), &wave)?;
        grouping.kept_of(&table, &Runner::new(&db_for, tx))
    });
    Morsels::start(db, tx, (PARTS, degree), finish).collect()
}

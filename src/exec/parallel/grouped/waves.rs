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
//! That holds of a grouping of few groups in all. Once the statement
//! thread's table holds more groups than a morsel has rows, the grouping
//! has many, and from then on every morsel goes to the partitions, that
//! table first (RULES.md D1247).
//!
//! Partitions that outgrow the grant are held in the spill from then on,
//! each partition's table written by the thread that merged it, and merged
//! back a partition at a time at the end (`spilled.rs`), so the threads'
//! work is kept and the grouping does not give way.
//!
//! \[DESIGN\] (RULES.md D1260) Where the morsels' tables do not reduce
//! their rows, the morsels stop making them (`Reduction`). A morsel's
//! table that ends with nearly a group a row found nothing to merge, and
//! its lookups and slots were spent for nothing, as the partition finds
//! every key again. Such a grouping's morsels hand each row on as a group
//! of its own, and one morsel in several still makes a table, so data
//! that begins to reduce is seen.

mod spilled;
mod wide;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use super::super::pipeline::Pipeline;
use super::super::probe::PARTS;
use super::super::run::{Lead, Morsels, Work};
use super::super::{Held, Limit};
use super::runs::Run;
use super::{Grouping, Waved, first_of};
use crate::error::Result;
use crate::exec::Db;
use crate::exec::batch::morsel_groups;
use crate::exec::table::{GroupTable, Made};
use crate::exec::{Runner, broken};
use crate::txn::TxId;
use spilled::Spilled;
pub(super) use spilled::given_of;
use wide::{Kept, WIDE, Widen, kept_of, thawed_and_merged};

/// \[DESIGN\] The rows of a morsel each of its groups stands for, on each
/// thread, at least, for the statement's thread to merge the morsel's
/// table itself: merging a group costs about what grouping a row or two
/// does, so the statement's thread merges a morsel's groups in the time the
/// threads group the rows of the next morsel each.
const DIRECT_ROWS: usize = 2;

/// Whether a morsel of `groups` groups made of `taken` rows on one of
/// `degree` threads is handed whole to the statement's thread: its groups
/// few beside its rows (`DIRECT_ROWS`), while morsels are `still` handed
/// so, the grouping's groups few too (RULES.md D1247).
fn few(still: bool, (groups, taken): (usize, usize), degree: usize) -> bool {
    still && groups.saturating_mul(DIRECT_ROWS).saturating_mul(degree) <= taken
}

/// \[DESIGN\] (RULES.md D1260) The groups a thousand rows of a morsel
/// must leave for the next morsels to be handed on ungrouped. From a
/// sweep with the hand-over forced on and forced off, over groupings of
/// integer keys whose morsels leave from 640 to 1000 groups a thousand
/// rows. Where every group is new to its partition the two were level at
/// 640 and the hand-over ahead at 790 and above, by more the nearer to a
/// group a row. Where the partitions merge the rows anyway, the keys few
/// in all, it was ahead at 730 and above. So the bound is 750: above the
/// ratio measured level, below the least measured ahead of both kinds.
const APART_PER_MILLE: usize = 750;

/// \[DESIGN\] (RULES.md D1260) The morsels of which one makes a table
/// though the others do not, to see whether the rows reduce now. One in
/// sixteen gives back a sixteenth of what the others save, and data that
/// turns to few groups is grouped apart again within sixteen morsels.
const PROBE_EVERY: usize = 16;

/// The parts of a ratio told in a thousand.
const MILLE: usize = 1000;

/// \[DESIGN\] (RULES.md D1260) What the morsels' tables tell of their
/// rows: the groups a thousand rows left in the table made last, none
/// while no table is made yet. The workers measure it of the rows they
/// group, and no estimate enters it.
struct Reduction {
    left: AtomicUsize,
}

impl Reduction {
    /// Nothing told yet.
    fn new() -> Reduction {
        Reduction { left: AtomicUsize::new(0) }
    }

    /// Whether morsel `m` hands its rows on ungrouped: the table made
    /// last left more than `APART_PER_MILLE` groups a thousand rows, and
    /// `m` is not one of those that probe.
    fn apart(&self, m: usize) -> bool {
        !m.is_multiple_of(PROBE_EVERY) && self.left.load(Ordering::Relaxed) > APART_PER_MILLE
    }

    /// Tells that a morsel's table took `rows` rows and ended with
    /// `groups` groups.
    fn tell(&self, rows: usize, groups: usize) {
        if let Some(left) = groups.saturating_mul(MILLE).checked_div(rows) {
            self.left.store(left, Ordering::Relaxed);
        }
    }
}

/// A partition's groups between waves, frozen, a table's or a wide
/// partition's (`wide.rs`); none before its first.
type Frozen = Arc<Vec<Mutex<Option<Kept>>>>;

/// What a morsel hands over: its groups whole, few beside its rows, for
/// the statement's thread to merge; or split among the partitions.
enum Handed {
    Whole(Made),
    Parted(Arc<Made>),
}

impl Handed {
    /// The bytes it holds, ahead of the statement's thread until read.
    fn bytes(&self) -> usize {
        match self {
            Handed::Whole(made) => made.bytes(),
            Handed::Parted(made) => made.bytes(),
        }
    }
}

/// \[DESIGN\] (RULES.md D1262) The morsels a thread a grouping's run may
/// lead the statement's thread by, while the grant has room for what they
/// hold. From a sweep of 2, 4, 8, 16, 32, 64 and 128 a thread over
/// groupings whose morsels cost unevenly, of few groups and of many, and
/// over a join's probe that makes several rows of each, on eight threads
/// and on two: the time falls as the lead grows to 16 and is level past
/// it, so 16, the least that is level, as a longer lead only holds more.
const LEAD_PER_THREAD: usize = 16;

/// \[DESIGN\] (RULES.md D1247, D1260) Morsels a test names by their
/// numbers, to be handed whole or to hand their rows on ungrouped,
/// whatever their groups and the grouping's: none named in a statement,
/// whose morsels are handed as `waved_as` and `Reduction` say.
pub(super) type Forced = Option<fn(usize) -> bool>;

/// How a grouping's waves are run: as a statement runs them, the default,
/// or as a test asks, which also reads what they did.
pub(super) struct How {
    /// The morsels handed whole, if a test names them.
    pub(super) forced: Forced,
    /// \[DESIGN\] (RULES.md D1260) The morsels that hand their rows on
    /// ungrouped, if a test names them, whatever the tables told.
    pub(super) apart: Forced,
    /// \[DESIGN\] (RULES.md D1262) The morsels a thread the run leads its
    /// reader by.
    pub(super) lead: usize,
    /// \[DESIGN\] (RULES.md D1282) The partitions a thread the runs that
    /// merge a wave and finish the partitions lead their reader by. A
    /// statement's runs lead by every partition. The reader keeps each
    /// partition's result until the last is in, so a partition done
    /// early holds nothing more for it, and the threads do not stand
    /// idle behind one partition that holds most of the groups.
    pub(super) parts: usize,
    /// \[DESIGN\] (RULES.md D1300) When a partition goes wide, and the
    /// waves merged into wide partitions.
    pub(super) widen: Widen,
    /// The waves the partitions merged, and the times their tables were
    /// written to the spill.
    pub(super) counted: (AtomicUsize, AtomicUsize),
}

impl Default for How {
    fn default() -> How {
        How {
            forced: None,
            apart: None,
            lead: LEAD_PER_THREAD,
            parts: PARTS,
            widen: Widen::read_as_they_lie(WIDE),
            counted: (AtomicUsize::new(0), AtomicUsize::new(0)),
        }
    }
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
    /// Where the partitions are held once they outgrow the grant.
    spilled: Option<Arc<Spilled>>,
    /// Whether morsels are still handed whole, and the groups the table
    /// they are merged into may hold while they are.
    whole: (Arc<AtomicBool>, usize),
    how: &'a How,
}

/// The groups of each partition of `grouping`'s pipeline, each finished
/// with its number, in the order of their first rows, made on up to
/// `degree` threads; given way past `limit`.
///
/// # Errors
///
/// As reading the pipeline, or a set function, fails.
pub(super) fn waved(pipeline: Pipeline, grouping: &Arc<Grouping>, run: (&Db, TxId, usize), limit: &Limit) -> Result<Waved> {
    waved_as(pipeline, grouping, run, limit, &How::default())
}

/// \[DESIGN\] (RULES.md D1247) As `waved`, run as `how` says, the morsels
/// it names handed whole and no others, if it names any. A morsel is handed whole while
/// the statement thread's table is small, its groups no more than a
/// morsel's rows, EXECUTOR.MORSEL_ROWS: no table a thread makes of a
/// morsel holds more, so up to there a group merges for what a row or two
/// cost to group, as `DIRECT_ROWS` counts. Past it every merge misses the
/// processor's cache and the table's keys grow by copying, on one thread,
/// so the grouping is one of many groups and its morsels are the
/// partitions' from then on. Which of the morsels being grouped as the
/// table passes that were already handed whole depends on the threads'
/// timing. They are merged in their order all the same, into a table
/// taken to the partitions before the next morsel of many groups, so the
/// groups, their order and each set function's result do not.
///
/// # Errors
///
/// As `waved`.
pub(super) fn waved_as(
    pipeline: Pipeline,
    grouping: &Arc<Grouping>,
    (db, tx, degree): (&Db, TxId, usize),
    limit: &Limit,
    how: &How,
) -> Result<Waved> {
    let count = pipeline.count();
    let whole = Arc::new(AtomicBool::new(true));
    let work = grouped(pipeline, grouping, (db, tx, degree), (Arc::clone(&whole), how.forced, how.apart));
    let mut waves = Waves::of(grouping, (db, tx, degree), limit, (whole, how));
    // [DESIGN] (RULES.md D1262) The threads lead the statement's thread
    // while what they hold ahead of it fits what the grant has left.
    let left = limit.clone();
    let room = Box::new(move || left.most().saturating_sub(left.held()));
    let lead = Lead { wide: how.lead, size: Handed::bytes, room };
    for morsel in Morsels::led(db, tx, (count, degree), work, Some(lead)) {
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

/// The work of a morsel of `pipeline` under `grouping` on one of `degree`
/// threads: its rows grouped in a table of its own, handed whole if its
/// groups are few (`few`) while morsels are `still` handed so, or if
/// `forced` names it, else split among the partitions. Its rows are
/// handed on ungrouped if `ungrouped` names it, or, of none named, as
/// the tables made before it tell (`Reduction`).
fn grouped(
    pipeline: Pipeline,
    grouping: &Arc<Grouping>,
    (db, tx, degree): (&Db, TxId, usize),
    (still, forced, ungrouped): (Arc<AtomicBool>, Forced, Forced),
) -> Work<Handed> {
    let (pipeline, db_for, making) = (Arc::new(pipeline), db.clone(), Arc::clone(grouping));
    // The groups the morsel last grouped made, the room the next is given.
    let (room, reduction) = (AtomicUsize::new(0), Reduction::new());
    Box::new(move |m: usize| {
        let runner = Runner::new(&db_for, tx);
        let read = making.read.as_ref().ok_or_else(|| broken("a grouping by batch reads no batch"))?;
        let batches = pipeline.morsel_kept(m, (&db_for, tx))?;
        let apart = ungrouped.map_or_else(|| reduction.apart(m), |names| names(m));
        let expected = room.load(Ordering::Relaxed);
        let asked = (&runner, first_of(&making, m), &making.seed);
        let table = morsel_groups(read, &making.spec.aggs, batches, asked, (expected, apart))?;
        room.store(table.len(), Ordering::Relaxed);
        if !apart {
            reduction.tell(table.taken, table.len());
        }
        let few = few(still.load(Ordering::Relaxed), (table.len(), table.taken), degree);
        if forced.map_or(few, |named| named(m)) {
            return table.frozen().map(Handed::Whole);
        }
        table.made(PARTS, making.lanes()).map(|made| Handed::Parted(Arc::new(made)))
    })
}

impl<'a> Waves<'a> {
    /// The waves of `grouping`, none merged yet, run as `run` and `how`
    /// say within `limit`, morsels handed whole while `whole` stands.
    fn of(
        grouping: &'a Arc<Grouping>,
        run: (&'a Db, TxId, usize),
        limit: &'a Limit,
        (whole, how): (Arc<AtomicBool>, &'a How),
    ) -> Waves<'a> {
        Waves {
            grouping,
            run,
            limit,
            frozen: Arc::new((0..PARTS).map(|_| Mutex::new(None)).collect()),
            wave: Vec::new(),
            wave_bytes: 0,
            parts_bytes: 0,
            waved: false,
            direct: None,
            direct_bytes: 0,
            spilled: None,
            whole: (whole, run.0.settings.count(crate::settings::EXECUTOR_MORSEL_ROWS).max(1)),
            how,
        }
    }
}

impl Waves<'_> {
    /// Merges `made`, a morsel's groups whole, into the statement thread's
    /// table, handed to the partitions once the grant does not hold it, or
    /// once it holds more groups than a morsel has rows, after which no
    /// morsel is handed whole: whether the grant holds what is held.
    ///
    /// # Errors
    ///
    /// As `GroupTable::absorb_whole`.
    fn direct(&mut self, made: &Made) -> Result<bool> {
        let grouping = self.grouping;
        let width = grouping.read.as_ref().map_or(0, |read| read.groups.len());
        let table = self.direct.get_or_insert_with(|| GroupTable::new(width, &grouping.spec.aggs, &grouping.seed, usize::MAX));
        table.absorb_whole(made)?;
        if table.len() > self.whole.1 {
            // A grouping of many groups: its morsels are the partitions'
            // from now on, this table first (RULES.md D1247).
            self.whole.0.store(false, Ordering::Relaxed);
            return self.flushed();
        }
        let bytes = table.bytes();
        let more = bytes.saturating_sub(self.direct_bytes);
        self.direct_bytes = bytes;
        if self.limit.take(more) {
            return Ok(true);
        }
        // The table outgrows the grant: to the partitions, which go to the
        // spill if they must.
        self.flushed()
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
            self.limit.give(bytes);
            // What is held goes to the spill, to make room for it; if it
            // does not fit even then, it goes there itself.
            if !self.spilled_all()? {
                return Ok(false);
            }
            if !self.limit.take(bytes) {
                self.limit.give(bytes);
                let Some(spilled) = &self.spilled else { return Ok(false) };
                return spilled.write_parts(self.regrouped(made)?.as_ref());
            }
        }
        self.wave.push(made);
        self.wave_bytes = self.wave_bytes.saturating_add(bytes);
        if self.wave_bytes.saturating_mul(2) < self.limit.most().saturating_sub(self.parts_bytes) {
            return Ok(true);
        }
        self.merged()
    }

    /// \[DESIGN\] (RULES.md D1260) `made`, a morsel's groups about to be
    /// written to the spill as they are, grouped first if its rows were
    /// handed on ungrouped. A partition read back is split by its groups'
    /// hashes where it outgrows the grant, and rows of one key share a
    /// hash, so a key the morsel held many times must be one group there,
    /// as the morsel's table would have made it.
    ///
    /// # Errors
    ///
    /// As `GroupTable::absorb_whole` or `GroupTable::made`.
    fn regrouped(&self, made: Arc<Made>) -> Result<Arc<Made>> {
        if !made.repeats() {
            return Ok(made);
        }
        let grouping = self.grouping;
        let width = grouping.read.as_ref().map_or(0, |read| read.groups.len());
        let mut table = GroupTable::new(width, &grouping.spec.aggs, &grouping.seed, usize::MAX);
        table.absorb_whole(&made)?;
        table.made(PARTS, grouping.lanes()).map(Arc::new)
    }

    /// Merges the wave into the partitions, held in memory while the grant
    /// holds them, else in the spill: whether every group could be held.
    ///
    /// # Errors
    ///
    /// As merging a partition, or writing it to the spill, fails.
    fn merged(&mut self) -> Result<bool> {
        let wave = std::mem::take(&mut self.wave);
        let parts = (self.grouping, &self.frozen, self.spilled.as_ref());
        let Some(held) = frozen_wave(parts, wave, self.run, (self.how.parts, self.how.widen.clone()))? else { return Ok(false) };
        self.how.counted.0.fetch_add(1, Ordering::Relaxed);
        self.limit.give(self.wave_bytes.saturating_add(self.parts_bytes));
        (self.wave_bytes, self.parts_bytes, self.waved) = (0, held, true);
        if self.limit.take(held) {
            return Ok(true);
        }
        self.limit.give(held);
        self.spilled_all()
    }

    /// Writes every partition's table held in memory to the spill, each by
    /// a thread, then the wave not yet merged, so the partitions are held
    /// there from now on: whether every group could be written.
    ///
    /// # Errors
    ///
    /// SQLCODE -918 if the spill area cannot be reserved; as writing to it
    /// fails.
    fn spilled_all(&mut self) -> Result<bool> {
        if self.spilled.is_some() && self.wave.is_empty() {
            return Ok(true);
        }
        let spilled = Arc::clone(self.spilled.get_or_insert_with(|| Arc::new(Spilled::new(self.run.0, self.run.1, self.limit))));
        self.how.counted.1.fetch_add(1, Ordering::Relaxed);
        let (frozen, grouping, (db, tx, degree)) = (Arc::clone(&self.frozen), Arc::clone(self.grouping), self.run);
        let write = Box::new(move |p: usize| {
            let held = frozen.get(p).and_then(|slot| slot.lock().unwrap_or_else(PoisonError::into_inner).take());
            held.map_or(Ok(true), |kept| spilled.write(p, &kept.into_made(&grouping)?))
        });
        let written =
            Morsels::start(db, tx, (PARTS, degree), write).try_fold(true, |all, done| Ok::<_, crate::error::SqlError>(all && done?))?;
        self.limit.give(self.parts_bytes);
        self.parts_bytes = 0;
        Ok(written && (self.wave.is_empty() || self.merged()?))
    }

    /// Each partition's groups finished, in the order of their first rows:
    /// of the statement thread's table alone if no morsel went to the
    /// partitions, else of the last wave, that table in it.
    ///
    /// # Errors
    ///
    /// As finishing a group, or merging a partition, fails.
    fn finished(mut self) -> Result<Waved> {
        // A table that went to the spill as it was, no wave merged, is
        // the partitions' too, though `waved` does not tell of it
        // (RULES.md D1276).
        if !self.waved && self.wave.is_empty() && self.spilled.is_none() {
            let Some(table) = self.direct.take() else { return Ok(Held::Kept((Vec::new(), Vec::new()))) };
            let groups = self.grouping.kept_of(table, &Runner::new(self.run.0, self.run.1))?;
            return Ok(Held::Kept((vec![groups], Vec::new())));
        }
        if !self.flushed()? {
            return Ok(Held::GaveWay);
        }
        let Some(spilled) = self.spilled.clone() else {
            let led = (self.how.parts, self.how.widen.clone());
            let runs = finished_wave((self.grouping, &self.frozen), self.wave, self.run, led)?;
            return Ok(Held::Kept((runs, Vec::new())));
        };
        // The last wave to the spill too, then each partition read back.
        if !self.wave.is_empty() && !self.merged()? {
            return Ok(Held::GaveWay);
        }
        let finished = spilled::finished(self.grouping, spilled, self.run, self.limit.most())?;
        Ok(finished.map_or(Held::GaveWay, |given| Held::Kept((Vec::new(), given))))
    }
}

/// Merges the morsels of `wave` into each partition's groups on up to
/// `degree` threads, each frozen again, or written to `spilled` if the
/// partitions are held there, a partition past `widen` groups wide
/// (`wide.rs`): the bytes the partitions held in memory hold; none if one
/// could not be written. The run leads its reader by `wide` partitions a
/// thread (RULES.md D1282).
///
/// # Errors
///
/// As merging a partition, or writing it to the spill, fails.
fn frozen_wave(
    (grouping, frozen, spilled): (&Arc<Grouping>, &Frozen, Option<&Arc<Spilled>>),
    wave: Vec<Arc<Made>>,
    (db, tx, degree): (&Db, TxId, usize),
    (wide, widen): (usize, Widen),
) -> Result<Option<usize>> {
    let (grouping, kept, wave, spilled) = (Arc::clone(grouping), Arc::clone(frozen), Arc::new(wave), spilled.cloned());
    let freeze = Box::new(move |p: usize| {
        let merged = thawed_and_merged(&grouping, (&kept, p), &wave, &widen)?;
        if let Some(spilled) = &spilled {
            return Ok(spilled.write(p, &merged.into_table()?.frozen()?)?.then_some(0));
        }
        let again = merged.frozen()?;
        let bytes = again.bytes();
        if let Some(slot) = kept.get(p) {
            *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(again);
        }
        Ok(Some(bytes))
    });
    Morsels::led(db, tx, (PARTS, degree), freeze, Some(Lead::unmetered(wide)))
        .try_fold(Some(0_usize), |sum, bytes| Ok(sum.zip(bytes?).map(|(sum, bytes)| sum.saturating_add(bytes))))
}

/// Merges the morsels of the last `wave` into each partition's groups on
/// up to `degree` threads, a partition past `widen` groups wide and
/// gathered into a table at the end (`wide.rs`): each partition's groups
/// finished, with their numbers, in the order of their first rows. The run
/// leads its reader by `wide` partitions a thread (RULES.md D1282).
///
/// # Errors
///
/// As merging or finishing a partition fails.
fn finished_wave(
    (grouping, frozen): (&Arc<Grouping>, &Frozen),
    wave: Vec<Arc<Made>>,
    (db, tx, degree): (&Db, TxId, usize),
    (wide, widen): (usize, Widen),
) -> Result<Vec<Run>> {
    let (grouping, kept, wave, db_for) = (Arc::clone(grouping), Arc::clone(frozen), Arc::new(wave), db.clone());
    let finish = Box::new(move |p: usize| {
        let merged = thawed_and_merged(&grouping, (&kept, p), &wave, &widen)?;
        kept_of(&grouping, merged, &Runner::new(&db_for, tx))
    });
    Morsels::led(db, tx, (PARTS, degree), finish, Some(Lead::unmetered(wide))).collect()
}

// Unit tests: the groups whichever morsels are handed whole (RULES.md
// R-TEST-1).
#[cfg(test)]
mod tests;

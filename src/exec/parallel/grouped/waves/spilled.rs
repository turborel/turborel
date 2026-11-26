//! \[DESIGN\] (RULES.md D1226) A grouping run in parallel whose partitions
//! outgrow its grant holds them in the spill, not in memory, and does not
//! give way. Once the partitions' tables after a wave pass the grant, each
//! partition's thread writes the table it holds to a run of the grouping's
//! spill area, and the waves after write each partition's table, merged of
//! that wave alone, to a run of its own. A partition is then its runs, in
//! the order of the waves, and nothing of it is held of the grant.
//!
//! At the end each partition's runs are read back, a piece at a time, and
//! merged in that order into one table, which is finished as a partition
//! held in memory is. As waves take the morsels in order, a group new to
//! a later wave has a later first row than every group of the waves
//! before; and a group of several waves keeps its first wave's, the
//! earliest, as a merge keeps the earlier of two. So the table holds its
//! groups in the order of their first rows, as one held in memory does.
//!
//! The partitions are merged on as many threads at once as the grant holds
//! the largest of: a partition is counted by what its runs' groups held as
//! they were written. A partition that holds more than half the grant is
//! split, a piece at a time, into as many parts again by the next bits of
//! the hash it was found by (`Made::part_at`), each part to a run of its
//! own, and each part finished alone, up to SPILL.PARTITION_DEPTH levels.
//! A partition past them, or a value no run can hold (`keys/written.rs`),
//! gives way.
//!
//! Each part's groups finished go to a run of their own, each numbered by
//! its first row, and the statement's thread reads the runs back as it
//! merges them in that order, so no more of the groups is held at once
//! than the threads finish together.

use std::sync::{Arc, Mutex, PoisonError};

use super::super::super::Limit;
use super::super::super::probe::PARTS;
use super::super::super::run::Morsels;
use super::super::{Given, Grouping};
use crate::error::Result;
use crate::exec::table::{GroupTable, Made, PIECE};
use crate::exec::work::Work;
use crate::exec::partition::{numbered, unnumbered};
use crate::exec::{Db, Runner, broken};
use crate::spill::{Records, SpilledLobs, SpillRun};
use crate::storage::Row;
use crate::txn::TxId;
use crate::value::types::TypeRegistry;

/// A partition held in the spill: its runs, in the order written, and the
/// bytes their groups held as they were written.
#[derive(Default)]
pub(super) struct Part {
    runs: Vec<Box<dyn SpillRun>>,
    bytes: usize,
}

/// A grouping's spill: the work area its runs are made in, and each
/// partition's runs.
pub(super) struct Spilled {
    work: Mutex<Work>,
    parts: Vec<Mutex<Part>>,
}

/// The finished groups of a partition, a run of them each part that kept
/// any; none if it gave way.
type Finished = Option<Vec<Box<dyn SpillRun>>>;

/// The groups of a run of finished groups read back, each with its number,
/// what their records name held by `lobs`, and the spill whose area holds
/// the run, kept while it is read.
struct Read {
    records: Records,
    types: TypeRegistry,
    lobs: SpilledLobs,
    _spilled: Arc<Spilled>,
}

impl Iterator for Read {
    type Item = Result<(u64, Row)>;

    fn next(&mut self) -> Option<Result<(u64, Row)>> {
        let record = self.records.next()?;
        Some(record.and_then(|record| unnumbered(&record, &self.types, &self.lobs)))
    }
}

impl Spilled {
    /// The spill of a grouping of `db` run by `tx`, its area of the grant
    /// `limit` holds.
    pub(super) fn new(db: &Db, tx: TxId, limit: &Limit) -> Spilled {
        let work = Work::of_grant(db, tx, Arc::clone(&limit.grant));
        Spilled { work: Mutex::new(work), parts: (0..PARTS).map(|_| Mutex::new(Part::default())).collect() }
    }

    /// A new run of its area.
    fn run(&self) -> Result<Box<dyn SpillRun>> {
        self.work.lock().unwrap_or_else(PoisonError::into_inner).run()
    }

    /// Writes `rows`, groups finished in the order of their first rows,
    /// each numbered by its first, to a run of its own; none of no group.
    ///
    /// # Errors
    ///
    /// As `write`.
    fn rows(&self, rows: Vec<(u64, Row)>) -> Result<Option<Box<dyn SpillRun>>> {
        if rows.is_empty() {
            return Ok(None);
        }
        let lobs = self.work.lock().unwrap_or_else(PoisonError::into_inner).lobs().clone();
        let mut run = self.run()?;
        for (n, row) in rows {
            run.push(&numbered(n, &row, &lobs))?;
        }
        Ok(Some(run))
    }

    /// The groups `rows` wrote to `run`, read back in their order.
    ///
    /// # Errors
    ///
    /// The area's errors.
    fn given(self: &Arc<Self>, run: Box<dyn SpillRun>) -> Result<Given> {
        let (types, lobs) = {
            let work = self.work.lock().unwrap_or_else(PoisonError::into_inner);
            (work.types().clone(), work.lobs().clone())
        };
        Ok(Box::new(Read { records: run.into_records()?, types, lobs, _spilled: Arc::clone(self) }))
    }

    /// Writes `made`, the groups of partition `p` frozen whole, to a run
    /// of its own after the partition's others: whether every group could
    /// be written.
    ///
    /// # Errors
    ///
    /// SQLCODE -918 if the area cannot be reserved; the area's errors.
    pub(super) fn write(&self, p: usize, made: &Made) -> Result<bool> {
        let places: Vec<u32> = (0..made.len()).map(crate::exec::table::narrow).collect();
        self.write_places(p, made, &places)
    }

    /// Writes the groups of each partition of `made`, a morsel's split
    /// among them, to a run of its own after the partition's others:
    /// whether every group could be written.
    ///
    /// # Errors
    ///
    /// As `write`.
    pub(super) fn write_parts(&self, made: &Made) -> Result<bool> {
        for p in 0..PARTS {
            if !self.write_places(p, made, made.part_places(p))? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Writes the groups of `made` at `places` to a run of partition `p`'s
    /// own after its others: whether every group could be written.
    fn write_places(&self, p: usize, made: &Made, places: &[u32]) -> Result<bool> {
        if places.is_empty() {
            return Ok(true);
        }
        let mut run = self.run()?;
        let mut record = Vec::new();
        for piece in places.chunks(PIECE) {
            record.clear();
            if !made.put_groups(piece, &mut record) {
                return Ok(false);
            }
            run.push(&record)?;
        }
        let Some(slot) = self.parts.get(p) else { return Err(broken("a partition past the grouping's")) };
        let mut part = slot.lock().unwrap_or_else(PoisonError::into_inner);
        part.runs.push(run);
        let share = made.bytes().saturating_mul(places.len()).checked_div(made.len()).unwrap_or(0);
        part.bytes = part.bytes.saturating_add(share);
        Ok(true)
    }

    /// Takes partition `p`'s runs.
    fn take(&self, p: usize) -> Part {
        self.parts.get(p).map(|slot| std::mem::take(&mut *slot.lock().unwrap_or_else(PoisonError::into_inner))).unwrap_or_default()
    }

    /// The bytes partition `p`'s runs' groups held.
    fn bytes_of(&self, p: usize) -> usize {
        self.parts.get(p).map_or(0, |slot| slot.lock().unwrap_or_else(PoisonError::into_inner).bytes)
    }
}

/// Each partition of `spilled` finished, its groups in the order of their
/// first rows, a run of them each part it was split into, read back as
/// they are given; on as many of `degree` threads at once as the grant of
/// `most` bytes holds the largest of; none if a partition gave way.
///
/// # Errors
///
/// As reading a run back, merging a group, or finishing one fails.
pub(super) fn finished(
    grouping: &Arc<Grouping>,
    spilled: Arc<Spilled>,
    (db, tx, degree): (&Db, TxId, usize),
    most: usize,
) -> Result<Option<Vec<Given>>> {
    let half = most.div_ceil(2).max(1);
    let largest = (0..PARTS).map(|p| spilled.bytes_of(p).min(half)).max().unwrap_or(0).max(1);
    let threads = half.checked_div(largest).unwrap_or(1).clamp(1, degree.max(1));
    let depth = spilled.work.lock().unwrap_or_else(PoisonError::into_inner).depth();
    let (grouping, db_for, kept) = (Arc::clone(grouping), db.clone(), Arc::clone(&spilled));
    let finish = Box::new(move |p: usize| {
        let part = spilled.take(p);
        finished_part(&grouping, part, (&spilled, 0, depth), (&Runner::new(&db_for, tx), half))
    });
    let mut runs = Vec::new();
    for part in Morsels::start(db, tx, (PARTS, threads), finish) {
        let Some(part) = part? else { return Ok(None) };
        runs.extend(part);
    }
    runs.into_iter().map(|run| kept.given(run)).collect::<Result<Vec<_>>>().map(Some)
}

/// `part`, at `level` levels of `depth` below the first, finished whole
/// if its groups held at most `half` the grant, else split and each part
/// finished alone.
fn finished_part(
    grouping: &Grouping,
    part: Part,
    (spilled, level, depth): (&Spilled, usize, usize),
    (runner, half): (&Runner, usize),
) -> Result<Finished> {
    if part.bytes <= half {
        let table = merged(grouping, part)?;
        return Ok(Some(spilled.rows(grouping.kept_of(table, runner)?.into_rows())?.into_iter().collect()));
    }
    if level >= depth {
        return Ok(None);
    }
    let Some(parts) = split(grouping, part, (spilled, level))? else { return Ok(None) };
    let mut runs = Vec::new();
    for each in parts {
        let Some(finished) = finished_part(grouping, each, (spilled, level.saturating_add(1), depth), (runner, half))? else {
            return Ok(None);
        };
        runs.extend(finished);
    }
    Ok(Some(runs))
}

/// A table of the groups of `part`'s runs, read back a piece at a time and
/// merged in the order written.
fn merged(grouping: &Grouping, part: Part) -> Result<GroupTable> {
    let width = grouping.read.as_ref().map_or(0, |read| read.groups.len());
    let mut table = GroupTable::new(width, &grouping.spec.aggs, &grouping.seed, usize::MAX);
    for run in part.runs {
        for record in run.into_records()? {
            table.absorb_whole(&Made::read(&record?, &grouping.spec.aggs)?)?;
        }
    }
    Ok(table)
}

/// `part`, at `level`, split into `PARTS` parts by the next bits of the
/// hash it was found by, each to a run of its own, its groups in the
/// order they came; none if a group could not be written.
fn split(grouping: &Grouping, part: Part, (spilled, level): (&Spilled, usize)) -> Result<Option<Vec<Part>>> {
    let mut parts: Vec<Part> = (0..PARTS).map(|_| Part::default()).collect();
    let (by, below) = ((grouping.lanes(), &grouping.seed), u32::try_from(level.saturating_add(1)).unwrap_or(u32::MAX));
    let (mut places, mut record): (Vec<Vec<u32>>, Vec<u8>) = ((0..PARTS).map(|_| Vec::new()).collect(), Vec::new());
    for run in part.runs {
        for bytes in run.into_records()? {
            let made = Made::read(&bytes?, &grouping.spec.aggs)?;
            places.iter_mut().for_each(Vec::clear);
            for g in 0..made.len() {
                if let Some(at) = places.get_mut(made.part_at(g, by, (PARTS, below))) {
                    at.push(crate::exec::table::narrow(g));
                }
            }
            for (into, at) in parts.iter_mut().zip(&places).filter(|(_, at)| !at.is_empty()) {
                record.clear();
                if !made.put_groups(at, &mut record) {
                    return Ok(None);
                }
                if into.runs.is_empty() {
                    into.runs.push(spilled.run()?);
                }
                if let Some(last) = into.runs.last_mut() {
                    last.push(&record)?;
                }
                let share = made.bytes().saturating_mul(at.len()).checked_div(made.len()).unwrap_or(0);
                into.bytes = into.bytes.saturating_add(share);
            }
        }
    }
    Ok(Some(parts))
}

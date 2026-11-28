//! \[DESIGN\] (RULES.md D1300) A partition of a grouping's waves, merged
//! as a table while it holds few groups and adopted where its groups lie
//! once it holds many (`exec/table/made/wide.rs`). A partition goes wide
//! when its table, a Made merged into it, holds more groups than `WIDE`,
//! and stays wide for the rest of the statement. The count is the
//! table's own, so the partitions that go wide, and when, follow from the
//! Mades each wave holds and their order alone. Which morsels a wave
//! holds may differ from run to run as before. The groups, their order
//! and each set function's result do not, as a wide partition holds what
//! the table would.
//!
//! Between waves a wide partition is kept as it is, its Mades adopted and
//! its table of its own frozen. The first rows of a sort are picked of a
//! wide partition where its groups lie (`top.rs`). Whatever else reads a
//! partition's groups reads a table, and a wide partition gathered into
//! one for it is written twice, so a partition goes wide only of a
//! grouping whose reader reads the groups where they lie (`lies`). Such a
//! partition is still gathered before it is written to the spill.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, PoisonError};

use super::super::Grouping;
use super::super::runs::Run;
use super::Frozen;
use crate::error::Result;
use crate::exec::table::{Adopted, GroupTable, Made, Wide};
use crate::exec::{Runner, broken};

/// \[DESIGN\] (RULES.md D1300) The groups a partition's table holds at
/// most before the partition goes wide. A merge into a table costs the
/// same a group for tables of 4,096 groups up to this many, on one
/// thread and on eight at once, of keys of integers and of texts, and
/// more past it, as the slots and the keys a held row is compared with
/// leave the second-level cache. Slots for this many groups are half a
/// mebibyte.
pub(super) const WIDE: usize = 1 << 15;

/// When a grouping's partitions go wide: past how many groups a table of
/// one holds, whether whatever reads them, and how many times a wave was
/// merged into a wide one, which a test reads.
#[derive(Clone)]
pub(in crate::exec::parallel::grouped) struct Widen {
    pub(in crate::exec::parallel::grouped) past: usize,
    /// Whether a partition goes wide whatever reads its groups, as a test
    /// asks so that the gathered table is compared too. A statement's
    /// partitions go wide only where they are read as they lie.
    every: bool,
    pub(in crate::exec::parallel::grouped) merged: Arc<AtomicUsize>,
}

impl Widen {
    /// Wide past `past` groups whatever reads the partition, no wave
    /// merged yet.
    pub(in crate::exec::parallel::grouped) fn past(past: usize) -> Widen {
        Widen { past, every: true, merged: Arc::new(AtomicUsize::new(0)) }
    }

    /// Wide past `past` groups of a grouping whose reader reads the
    /// groups where they lie, as a statement's partitions go.
    pub(in crate::exec::parallel::grouped) fn read_as_they_lie(past: usize) -> Widen {
        Widen { every: false, ..Widen::past(past) }
    }
}

/// \[DESIGN\] (RULES.md D1300) Whether what reads a partition of
/// `grouping` reads its groups where they lie: the first rows of a sort,
/// and no second grouping over it, whose table reads the first's groups
/// as a table (RULES.md D1281). Only of such a grouping does a partition
/// go wide. Any other reader has the wide partition gathered into a
/// table, which writes every group a second time and saves nothing a
/// merge into the table would have cost.
fn lies(grouping: &Grouping) -> bool {
    grouping.top.is_some() && grouping.outer.is_none()
}

/// A partition's groups between waves, as they go to another thread: a
/// table's frozen, or a wide partition's.
pub(super) enum Kept {
    Table(Made),
    Wide(Adopted),
}

impl Kept {
    /// The bytes it holds.
    pub(super) fn bytes(&self) -> usize {
        match self {
            Kept::Table(made) => made.bytes(),
            Kept::Wide(adopted) => adopted.bytes(),
        }
    }

    /// Its groups as a table's frozen, a wide partition's gathered, as
    /// the spill writes a partition.
    ///
    /// # Errors
    ///
    /// As `Accumulator::merge`; SQLCODE -910 if an accumulator writes no
    /// state.
    pub(super) fn into_made(self, grouping: &Grouping) -> Result<Made> {
        match self {
            Kept::Table(made) => Ok(made),
            Kept::Wide(adopted) => Wide::thawed(adopted, &grouping.spec.aggs, &grouping.seed)?.into_table()?.frozen(),
        }
    }
}

/// A partition's groups while a wave is merged into them.
pub(super) enum Merging {
    Table(Box<GroupTable>),
    Wide(Box<Wide>),
}

impl Merging {
    /// Itself with partition `p` of `made` merged into it, a table that
    /// then holds more than `widen` groups gone wide.
    fn absorbed(self, made: &Arc<Made>, p: usize, widen: usize) -> Result<Merging> {
        match self {
            Merging::Table(mut table) => {
                table.absorb(made, p)?;
                if table.len() > widen { Wide::of(*table).map(|wide| Merging::Wide(Box::new(wide))) } else { Ok(Merging::Table(table)) }
            }
            Merging::Wide(mut wide) => {
                wide.absorb(made, made.part_places(p))?;
                Ok(Merging::Wide(wide))
            }
        }
    }

    /// As it is kept between waves.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if an accumulator writes no state.
    pub(super) fn frozen(self) -> Result<Kept> {
        match self {
            Merging::Table(table) => table.frozen().map(Kept::Table),
            Merging::Wide(wide) => wide.frozen().map(Kept::Wide),
        }
    }

    /// Its groups as a table, a wide partition's gathered, in the order
    /// of their first rows.
    ///
    /// # Errors
    ///
    /// As `Accumulator::merge`; SQLCODE -910 if an accumulator writes no
    /// state.
    pub(super) fn into_table(self) -> Result<GroupTable> {
        match self {
            Merging::Table(table) => Ok(*table),
            Merging::Wide(wide) => wide.into_table(),
        }
    }
}

/// The groups a partition of `grouping` gives of `merged`, finished, as
/// `Grouping::kept_of` gives a table's. Of a wide partition asked the
/// first rows of a sort, and no second grouping over it, they are picked
/// where the groups lie (`top.rs`). For every other reader it is
/// gathered into a table.
///
/// # Errors
///
/// As finishing a group fails.
pub(super) fn kept_of(grouping: &Grouping, merged: Merging, runner: &Runner) -> Result<Run> {
    let wide = match merged {
        Merging::Table(table) => return grouping.kept_of(*table, runner),
        Merging::Wide(wide) => *wide,
    };
    // Of a pair the first rows are the second grouping's table's (RULES.md
    // D1281), which reads the first's groups as a table.
    match grouping.top.as_ref().filter(|_| lies(grouping)) {
        Some(top) => Ok(Run::Rows(top.of_wide(wide, &grouping.spec.aggs)?.into_iter().map(|row| (0, row)).collect())),
        None => grouping.kept_of(wide.into_table()?, runner),
    }
}

/// Partition `p` of `grouping`: its groups `frozen` after the waves
/// before, taken up, then the morsels of `wave` merged in order, as a
/// table until it holds more groups than `widen` says and wide from then
/// on, which `widen` counts, if its groups are read where they lie
/// (`lies`) or `widen` says whatever reads them.
///
/// # Errors
///
/// As `Accumulator::merge`; a sum past 128 bits.
pub(super) fn thawed_and_merged(grouping: &Grouping, (frozen, p): (&Frozen, usize), wave: &[Arc<Made>], widen: &Widen) -> Result<Merging> {
    let read = grouping.read.as_ref().ok_or_else(|| broken("a grouping by batch reads no batch"))?;
    let (aggs, seed) = (&grouping.spec.aggs, &grouping.seed);
    let past = if widen.every || lies(grouping) { widen.past } else { usize::MAX };
    let held = frozen.get(p).and_then(|slot| slot.lock().unwrap_or_else(PoisonError::into_inner).take());
    let mut merging = match held {
        Some(Kept::Table(made)) => Merging::Table(Box::new(GroupTable::thawed(made, aggs, seed)?)),
        Some(Kept::Wide(adopted)) => Merging::Wide(Box::new(Wide::thawed(adopted, aggs, seed)?)),
        None => Merging::Table(Box::new(GroupTable::new(read.groups.len(), aggs, seed, usize::MAX))),
    };
    // Each of the wave's groups may be new, so a table's room for them is
    // made once, no more than it holds before it goes wide.
    if let (Merging::Table(table), Some(first)) = (&mut merging, wave.first()) {
        let handed: usize = wave.iter().map(|made| made.groups_of(p)).sum();
        table.reserve(first, handed.min(past.saturating_add(first.groups_of(p))));
    }
    for made in wave {
        if made.groups_of(p) > 0 {
            merging = merging.absorbed(made, p, past)?;
        }
    }
    if let Merging::Wide(_) = &merging {
        widen.merged.fetch_add(1, Ordering::Relaxed);
    }
    Ok(merging)
}

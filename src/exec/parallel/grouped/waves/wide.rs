//! \[DESIGN\] (RULES.md D1300) A partition of a grouping's waves, merged
//! as a table while it holds few groups and adopted where its groups lie
//! once it holds many (`exec/table/made/wide.rs`). A partition goes wide
//! when its table, a Made merged into it, holds more groups than `WIDE`
//! and the wave's entries are mostly new groups or on course to make
//! many (RULES.md D1302), and stays wide for the rest of the statement. The
//! counts are the partition's own, so the partitions that go wide, and
//! when, follow from the Mades each wave holds and their order alone. Which morsels a wave
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
use crate::exec::table::{Adopted, GroupTable, Made, WaveRun, Wide};
use crate::exec::{Runner, broken};

/// \[DESIGN\] (RULES.md D1300) The groups a partition's table holds at
/// most before the partition goes wide. A merge into a table costs the
/// same a group for tables of 4,096 groups up to this many, on one
/// thread and on eight at once, of keys of integers and of texts, and
/// more past it, as the slots and the keys a held row is compared with
/// leave the second-level cache. Slots for this many groups are half a
/// mebibyte.
pub(super) const WIDE: usize = 1 << 15;

/// \[DESIGN\] (RULES.md D1301) The groups and rows a sub-part of a wide
/// partition holds, or about, while a wave's rows are looked up in it.
/// Slots for this many are 32,768 words, a quarter of a mebibyte, and
/// the rows looked up beside them as much, so a sub-part's lookups stay
/// within a second-level cache of a mebibyte, the least of the
/// processors a server runs on. Merges of keys of texts, a third of
/// them held already, into partitions of two million groups cost least
/// a group at this size among 4,096, 16,384, 65,536 and 262,144, and
/// merges of integer keys all new cost the same at each.
pub(super) const SUB: usize = 1 << 14;

/// \[DESIGN\] (RULES.md D1302) The most entries in a thousand, of those
/// a wave has handed a partition so far, that may have met a group the
/// partition held already for it to go wide as a partition of entries
/// mostly new. Of eight groupings of a hundred million rows, counted a
/// partition as its table passed `WIDE`, the four of entries mostly new
/// had seen at most 57 in a thousand meet a held group, and the others
/// at least 192. This is the middle of the two.
pub(super) const MET: usize = 125;

/// \[DESIGN\] (RULES.md D1302) The groups a partition is on course to
/// hold once the wave is merged from which it goes wide whatever share
/// of its entries met a held group. Of the same eight groupings, the
/// partitions that ran faster as tables were on course for 115,000
/// groups at most, and those that ran faster wide for 329,000 at least,
/// one among them a fifth of whose entries had met a held group.
/// This is the middle of the two in the logarithm, about 194,000, as
/// three times 65,536.
pub(super) const COURSE: usize = 3 << 16;

/// When a grouping's partitions go wide: past how many groups a table of
/// one holds, whether whatever reads them, of how many of a wave's
/// entries met a held group or on course for how many groups, and how
/// many times a wave was merged into a wide one, which a test reads.
#[derive(Clone)]
pub(in crate::exec::parallel::grouped) struct Widen {
    pub(in crate::exec::parallel::grouped) past: usize,
    /// Whether a partition goes wide whatever reads its groups, as a test
    /// asks so that the gathered table is compared too. A statement's
    /// partitions go wide only where they are read as they lie.
    every: bool,
    /// \[DESIGN\] (RULES.md D1302) The most entries in a thousand of a
    /// wave's that may have met a held group for a partition to go
    /// wide by its entries: `MET` of a statement's, and a thousand, so
    /// whatever share met one, as a test asks of groupings whose
    /// entries mostly do.
    met: usize,
    /// \[DESIGN\] (RULES.md D1302) The groups a partition is on
    /// course to hold from which it goes wide whatever its entries met:
    /// `COURSE` of a statement's.
    course: usize,
    /// \[DESIGN\] (RULES.md D1301) The groups and rows a sub-part of a
    /// wide partition holds, or about.
    sub: usize,
    pub(in crate::exec::parallel::grouped) merged: Arc<AtomicUsize>,
}

/// A thousand, the entries a share of them is counted in.
const THOUSAND: usize = 1000;

/// \[DESIGN\] (RULES.md D1302) What a partition has counted of a wave
/// while it merges it as a table: the groups it held when the wave
/// began, the entries the wave hands it in all, those handed so far,
/// and the groups they made. An entry is what a Made holds for the
/// partition to look up once (`Made::groups_of`): a group of a
/// morsel's table, a row handed on ungrouped (RULES.md D1260), or a
/// run of rows of one key (RULES.md D1323).
#[derive(Clone, Copy)]
struct Counted {
    began: usize,
    handed: usize,
    so_far: usize,
    made: usize,
}

impl Widen {
    /// Wide past `past` groups whatever reads the partition and whatever
    /// share of a wave's entries met a held group, no wave merged yet.
    pub(in crate::exec::parallel::grouped) fn past(past: usize) -> Widen {
        Widen { past, every: true, met: THOUSAND, course: COURSE, sub: SUB, merged: Arc::new(AtomicUsize::new(0)) }
    }

    /// Wide past `past` groups of a grouping whose reader reads the
    /// groups where they lie, where a wave's entries are mostly new
    /// groups (`MET`) or the partition is on course for many (`COURSE`),
    /// as a statement's partitions go.
    pub(in crate::exec::parallel::grouped) fn read_as_they_lie(past: usize) -> Widen {
        Widen { every: false, met: MET, ..Widen::past(past) }
    }

    /// This, a wide partition's sub-parts of `sub` groups and rows or
    /// about, as a test asks so that few groups fill many sub-parts.
    #[cfg(test)]
    pub(in crate::exec::parallel::grouped) fn parted(self, sub: usize) -> Widen {
        Widen { sub, ..self }
    }

    /// This, a partition on course for `course` groups or more going
    /// wide whatever its entries met, as a test asks of partitions of
    /// few groups.
    #[cfg(test)]
    pub(in crate::exec::parallel::grouped) fn on_course(self, course: usize) -> Widen {
        Widen { course, ..self }
    }

    /// \[DESIGN\] (RULES.md D1302) Whether a partition whose table has
    /// passed the groups it holds before it goes wide goes wide now, by
    /// what it `counted` of the wave. It does for either of two reasons.
    /// At most `met` in a thousand of the entries handed so far met a
    /// group it held, so nearly every entry is a group whose copy a
    /// wide partition saves, whatever the partition's size. Or the
    /// groups it held when the wave began, with those the wave is on
    /// course to make of all its entries at the rate of those handed so
    /// far, are `course` or more, a table so far past a cache that its
    /// lookups cost more than a wide partition's even where many
    /// entries meet a held group.
    fn goes_wide(&self, counted: Counted) -> bool {
        let met = counted.so_far.saturating_sub(counted.made);
        let mostly_new = met.saturating_mul(THOUSAND) <= self.met.saturating_mul(counted.so_far);
        let to_come = counted.made.saturating_mul(counted.handed).checked_div(counted.so_far).unwrap_or(0);
        mostly_new || counted.began.saturating_add(to_come) >= self.course
    }
}

/// \[DESIGN\] (RULES.md D1300) Whether what reads a partition of
/// `grouping` reads its groups where they lie. That is the first rows of
/// a sort whose every key names a set function, which picks its groups
/// before their rows are made (RULES.md D1256), with no second grouping
/// over it, whose table reads the first's groups as a table (RULES.md
/// D1281). Only of such a grouping does a partition go wide. Any other
/// reader, a sort by a grouping column among them, has the wide
/// partition gathered into a table, which writes every group a second
/// time and saves nothing a merge into the table would have cost.
fn lies(grouping: &Grouping) -> bool {
    let width = grouping.read.as_ref().map_or(0, |read| read.groups.len());
    let picked = grouping.top.as_ref().is_some_and(|top| top.sorted_by(width).is_some());
    picked && grouping.outer.is_none()
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
/// first rows of a sort by set functions, and no second grouping over
/// it, they are picked where the groups lie (`top/wide.rs`). For every
/// other reader it is gathered into a table.
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

/// \[DESIGN\] (RULES.md D1300, D1302) Merges into `table`, a
/// partition's, partition `p` of each Made of `rest`, in their order,
/// until it goes wide or they end. Once it holds more groups than
/// `widen` says, a Made merged, it goes wide if what it has counted of
/// the wave says so (`Widen::goes_wide`): the entries handed so far are
/// counted as their Mades are merged, the groups they made are those
/// the table holds more than it held when the wave began, and `handed`
/// is the entries the wave hands it in all. A table past that many
/// groups that does not go wide stays a table, is judged again after
/// each Made, and is given room once for the rest of the wave's
/// entries.
fn merged_while_a_table<'a>(
    mut table: Box<GroupTable>,
    rest: &mut impl Iterator<Item = &'a Arc<Made>>,
    (p, handed): (usize, usize),
    (past, widen): (usize, &Widen),
) -> Result<Merging> {
    let (mut counted, mut roomed) = (Counted { began: table.len(), handed, so_far: 0, made: 0 }, false);
    for made in rest {
        counted.so_far = counted.so_far.saturating_add(made.groups_of(p));
        table.absorb(made, p)?;
        if table.len() <= past {
            continue;
        }
        counted.made = table.len().saturating_sub(counted.began);
        if widen.goes_wide(counted) {
            return Ok(Merging::Wide(Box::new(Wide::of(*table))));
        }
        if !roomed {
            table.reserve(made, handed.saturating_sub(counted.so_far));
            roomed = true;
        }
    }
    Ok(Merging::Table(table))
}

/// Partition `p` of `grouping`: its groups `frozen` after the waves
/// before, taken up, then the morsels of `wave` merged in order, as a
/// table until it goes wide (`merged_while_a_table`) and wide from then
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
    let handed: usize = wave.iter().map(|made| made.groups_of(p)).sum();
    let mut rest = wave.iter().filter(|made| made.groups_of(p) > 0);
    if let Merging::Table(mut table) = merging {
        // Each of the wave's groups may be new, so a table's room for
        // them is made once, no more than it holds before it goes wide.
        if let Some(first) = wave.first() {
            table.reserve(first, handed.min(past.saturating_add(first.groups_of(p))));
        }
        merging = merged_while_a_table(table, &mut rest, (p, handed), (past, widen))?;
    }
    // A wide partition looks up what is left of the wave at once, a
    // sub-part at a time (RULES.md D1301).
    if let Merging::Wide(wide) = &mut merging {
        let runs: Vec<WaveRun<'_>> = rest.map(|made| (made, made.part_places(p))).collect();
        if !runs.is_empty() {
            wide.absorb_wave(&runs, widen.sub)?;
        }
        widen.merged.fetch_add(1, Ordering::Relaxed);
    }
    Ok(merging)
}

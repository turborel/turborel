//! \[DESIGN\] Groups made by batch (RULES.md D31, D38, D356): held in the
//! work area in the executor's table (`exec/table.rs`), in the order their
//! first rows came, each its grouping values and its set functions; each
//! batch's keys hashed a vector at a time, each row's group found or made
//! by its key in the vectors, no key copied unless its group is new, and
//! each set function's states fed the batch's rows in one loop, or each
//! group's rows gathered in order and fed to its accumulator a vector at
//! a time. Past the work area, the rows of
//! the groups not held are written by batch: of each batch, those of each
//! partition one record, of their vectors (`spilled.rs`). Each partition
//! is grouped again the same way. Of one that outgrows the work area
//! again, once the groups held first fill it, if every set function can
//! write its state (`Accumulator::write_state`) and they took enough rows
//! each (`PARTIAL_GAIN`), the later half of them is written as pieces, a
//! piece a group's first row's number, grouping values and states, and
//! their room holds groups partly made of the rows past those held,
//! written as pieces once it is full; none is made partly once those took
//! too few rows each. Pieces are merged in the order they were written
//! (`Accumulator::merge`), so each set function takes its values in the
//! order it would without them.

mod morsel;
mod spilled;
mod streamed;

pub(in crate::exec) use morsel::morsel_groups;
pub(in crate::exec) use streamed::streamed;

use std::sync::Arc;

use super::{Batches, Fed, Read, lanes};
use crate::db::State;
use crate::error::Result;
use crate::exec::group::Finished;
use crate::exec::table::{GroupTable, Key, Seed, hash_rows};
use crate::exec::work::Work;
use crate::exec::{RowIter, Runner};
use crate::logical::AggCall;
use crate::storage::sel::nth;
use crate::storage::{Batch, Column, Row, Sel};
use crate::txn::TxId;
use spilled::{Overflow, Piece, Queued};

/// The rows or pieces groups partly made take each, on average, for a
/// grouping to go on making them: past fewer, its pieces, each costing
/// more to write and read back than a row written by batch, would save
/// little (`Groups::flush`).
const PARTIAL_GAIN: usize = 4;

/// What a grouping groups and computes: what it reads of each batch, its
/// set functions, what hears their warnings, what its keys are hashed by,
/// and whether it makes groups partly before they spill.
#[derive(Clone, Copy)]
struct Grouping<'a> {
    read: &'a Read,
    aggs: &'a [AggCall],
    runner: &'a Runner,
    seed: &'a Seed,
    partial: bool,
}

/// The number of each row given of a batch, by where it came: from the
/// first given, or as written beside it.
#[derive(Clone, Copy)]
enum Numbers<'a> {
    From(u64),
    Given(&'a [u64]),
}

impl Numbers<'_> {
    /// The number of the `n`th row given.
    fn of(self, n: usize) -> u64 {
        match self {
            Numbers::From(first) => first.saturating_add(u64::try_from(n).unwrap_or(u64::MAX)),
            Numbers::Given(numbers) => numbers.get(n).copied().unwrap_or(u64::MAX),
        }
    }
}

/// The groups of the rows of `batches`, as `read` reads them, each its
/// grouping values and the finished set functions `aggs`, in the order
/// their first rows came, in a work area of `db`, for the statement of
/// `tx`. Groups are made partly before they spill if every set function
/// writes its state.
pub(super) fn grouped_all(read: &Read, aggs: &[AggCall], (db, tx): (&Arc<State>, TxId), batches: Batches) -> Result<RowIter> {
    let (mut work, runner) = (Work::of(db, tx), Runner::new(db, tx));
    let partial = aggs.iter().all(|a| a.func.0.accumulator(a.result.clone()).write_state(&mut Vec::new()));
    let how = Grouping { read, aggs, runner: &runner, seed: db.functions.seed(), partial };
    // Rows past the work area are written by batch, the cheapest to read
    // back once; groups are made partly only of a partition that outgrows
    // it again, whose rows would be written again (`spilled::regrouped`).
    let mut groups = Groups::new(how, work.limit(), Overflow::Partitions(None), false);
    let mut rows = 0_u64;
    for kept in batches {
        let (batch, given) = kept?;
        groups.feed((&batch, given.as_ref().map(Sel::as_slice)), Numbers::From(rows), how, &mut work)?;
        rows = rows.saturating_add(u64::try_from(given.as_ref().map_or(batch.rows, Sel::len)).unwrap_or(u64::MAX));
    }
    let (held, overflow, _) = groups.finished(&mut work)?;
    spilled::merged(held, overflow, how, &mut work)
}

/// \[DESIGN\] (RULES.md D31) Of a grouping by `groups`, one column of
/// `batch` whose integers are known as runs (`Ints::runs`), none null,
/// where each run of its rows `rows`, or all, ends among them, the rows of
/// each run alike, so of one group: none of another grouping, nor of a
/// null among the runs, of the null group.
fn run_ends(batch: &Batch, groups: &[usize], rows: Option<&[u32]>) -> Option<Vec<usize>> {
    let [at] = groups else { return None };
    let Some(Column::Ints(ints)) = batch.columns.get(*at) else { return None };
    ints.runs()?;
    let lengths: Vec<usize> = match rows {
        Some(rows) => ints.runs_of(rows)?.map(|(_, these)| these.len()).collect(),
        None => ints.run_lengths()?.map(|(_, length)| length).collect(),
    };
    let mut end = 0_usize;
    Some(
        lengths
            .into_iter()
            .filter(|&n| n > 0)
            .map(|n| {
                end = end.saturating_add(n);
                end
            })
            .collect(),
    )
}

/// A group of a grouping: one held, or one partly made, by its number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    Held(u32),
    Partial(u32),
}

/// Where a row or piece goes: to a group; to be written, a row by batch,
/// a piece as it is; or nowhere yet, as its group would be partly made
/// but those are full, or the groups held first fill the work area and
/// it is to be split (`Groups::split`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Placed {
    Into(Target),
    Written,
    Passed,
    Full,
    Split,
}

/// A key to place, and its hash.
type Hashed<'a> = (u64, Key<'a>);

/// Groups being made, in the work area.
struct Groups {
    /// The groups held, while they fit the work area.
    held: GroupTable,
    /// The work area's bytes.
    limit: usize,
    /// Whether, the set functions writing their states, the work area is
    /// still to be split once the groups held fill it.
    splits: bool,
    /// The groups partly made of the rows past them, in the other half,
    /// if the set functions write their states.
    partial: Option<GroupTable>,
    /// The bytes of the key of a row or piece written past the work area.
    key: Vec<u8>,
    /// The hash of each row of the batch being grouped.
    hashes: Vec<u64>,
    /// Where the rows or pieces of groups past the work area go.
    overflow: Overflow,
    /// The rows of the batch being grouped to be written there: each one's
    /// partition, place in the batch and number.
    pending: Vec<(usize, usize, u64)>,
    /// The pieces to be written there, queued to be written many at once.
    queued: Queued,
    /// Whether a group has been past it: every group not yet held is then,
    /// so no group is both held and written.
    spilled: bool,
    /// Whether it took in a piece, so its groups may not be in the order of
    /// their first rows.
    pieced: bool,
    /// The row an expression a set function takes is evaluated of.
    scratch: Row,
}

impl Groups {
    /// No groups yet of the grouping `how`, held while they take no more
    /// than `limit` bytes; if groups may be made `partial`ly, the area
    /// split once they fill it; what is past them written to `overflow`.
    fn new(how: Grouping<'_>, limit: usize, overflow: Overflow, partial: bool) -> Groups {
        Groups {
            held: GroupTable::new(how.read.groups.len(), how.aggs, how.seed, limit),
            limit,
            splits: partial,
            partial: None,
            key: Vec::new(),
            hashes: Vec::new(),
            overflow,
            pending: Vec::new(),
            queued: Queued::default(),
            spilled: false,
            pieced: false,
            scratch: Vec::new(),
        }
    }

    /// Feeds the rows of `batch` at `rows`, or all, numbered by `numbers`,
    /// to their groups' set functions, as `how` says; the rows of groups
    /// past the work area are written, by batch, in `work`, for their
    /// groups to be made after, or their groups made partly, written as
    /// pieces in `work` once those are full.
    fn feed(&mut self, (batch, rows): (&Batch, Option<&[u32]>), numbers: Numbers<'_>, how: Grouping<'_>, work: &mut Work) -> Result<()> {
        let lanes = lanes(&how.read.taken, batch, rows);
        hash_rows(batch, (&how.read.groups, rows), (how.seed, how.runner.kernels()), &mut self.hashes);
        // Of a grouping by one column of runs, a row of the run of the row
        // before it goes to that row's group, no key compared.
        let mut ends = run_ends(batch, &how.read.groups, rows).into_iter().flatten();
        let (mut boundary, mut last) = (0_usize, None);
        for n in 0..rows.map_or(batch.rows, <[u32]>::len) {
            let r = nth(rows, n);
            if n >= boundary {
                boundary = ends.next().unwrap_or(n.saturating_add(1));
            } else if let Some(target) = last {
                self.take(target, (n, r));
                continue;
            }
            let hashed = (self.hashes.get(n).copied().unwrap_or(0), Key::Row(batch, &how.read.groups, r));
            let mut placed = self.place(hashed, (numbers.of(n), Some(r)), work)?;
            while let Placed::Full | Placed::Split = placed {
                // The rows given so far fed, so the pieces written hold them.
                self.fed(batch, &lanes, how)?;
                self.make_room(placed, work)?;
                placed = self.place(hashed, (numbers.of(n), Some(r)), work)?;
            }
            last = if let Placed::Into(target) = placed { Some(target) } else { None };
            if let Some(target) = last {
                self.take(target, (n, r));
            }
        }
        self.overflow.write(batch, (&mut self.pending, &mut self.queued), work)?;
        self.fed(batch, &lanes, how)
    }

    /// Takes the `n`th row given, at `r` of its batch, into group `target`.
    fn take(&mut self, target: Target, (n, r): (usize, usize)) {
        match (target, &mut self.partial) {
            (Target::Held(g), _) => self.held.take(g, (n, r)),
            (Target::Partial(g), Some(partial)) => partial.take(g, (n, r)),
            (Target::Partial(_), None) => {}
        }
    }

    /// Feeds the rows taken of `batch` to their groups' set functions, of
    /// `lanes` where evaluated by vectors, as `how` says.
    fn fed(&mut self, batch: &Batch, lanes: &[Option<Fed<'_>>], how: Grouping<'_>) -> Result<()> {
        let taken = (how.read.taken.as_slice(), lanes);
        self.held.feed(batch, taken, (&mut self.scratch, how.runner))?;
        match &mut self.partial {
            Some(partial) => partial.feed(batch, taken, (&mut self.scratch, how.runner)),
            None => Ok(()),
        }
    }

    /// Where the row `r` of a batch, or a piece (no `r`), the `number`th,
    /// of key and hash `hashed`, goes: its group held, or made while those
    /// held fit, so a grouping holds one at least; past them, as `past`
    /// says.
    fn place(&mut self, hashed: Hashed<'_>, (number, r): (u64, Option<usize>), work: &mut Work) -> Result<Placed> {
        if let Some(g) = self.held.find(hashed.0, hashed.1) {
            return Ok(Placed::Into(Target::Held(g)));
        }
        if self.spilled || self.held.full() {
            return self.past(hashed, (number, r), work);
        }
        Ok(Placed::Into(Target::Held(self.held.make(hashed.0, hashed.1, number))))
    }

    /// Where a row or piece of a group not held goes: nowhere till the
    /// work area is split, if it may be and the groups held first fill
    /// it; its group partly made, or none while those are full; else a row
    /// is to be written (`pending`), its partition found in `work`, and a
    /// piece passed on.
    fn past(&mut self, (hash, key): Hashed<'_>, (number, r): (u64, Option<usize>), work: &mut Work) -> Result<Placed> {
        if self.splits {
            return Ok(Placed::Split);
        }
        self.spilled = true;
        let Some(partial) = &mut self.partial else {
            let Some(r) = r else { return Ok(Placed::Passed) };
            self.key.clear();
            key.put(self.held.width(), &mut self.key);
            let part = self.overflow.part_of(&self.key, work)?;
            self.pending.push((part, r, number));
            return Ok(Placed::Written);
        };
        if let Some(g) = partial.find(hash, key) {
            return Ok(Placed::Into(Target::Partial(g)));
        }
        if partial.full() {
            return Ok(Placed::Full);
        }
        Ok(Placed::Into(Target::Partial(partial.make(hash, key, number))))
    }

    /// Takes in `piece`, as `how` says: merged into its group, held or
    /// partly made, made if new, in the order pieces come; those partly
    /// made written in `work` once full.
    fn absorb(&mut self, piece: &Piece, how: Grouping<'_>, work: &mut Work) -> Result<()> {
        self.pieced = true;
        let key = Key::Values(&piece.values);
        let hashed = (crate::exec::table::hash_values(how.seed, &piece.values), key);
        let mut placed = self.place(hashed, (piece.first, None), work)?;
        while let Placed::Full | Placed::Split = placed {
            self.make_room(placed, work)?;
            placed = self.place(hashed, (piece.first, None), work)?;
        }
        // Groups not made partly: a piece of one not held goes on.
        if placed == Placed::Passed {
            self.key.clear();
            key.put(self.held.width(), &mut self.key);
            return self.overflow.write_piece(&self.key, piece, &mut self.queued, work);
        }
        let (table, g) = match (placed, &mut self.partial) {
            (Placed::Into(Target::Held(g)), _) => (&mut self.held, g),
            (Placed::Into(Target::Partial(g)), Some(partial)) => (partial, g),
            (Placed::Into(Target::Partial(_)), None) | (Placed::Written | Placed::Passed | Placed::Full | Placed::Split, _) => {
                return Err(spilled::lost());
            }
        };
        table.took();
        table.merge_written(g, (piece.first, &piece.states))
    }

    /// Makes room for what `placed` found none for: writes the groups
    /// partly made (`Full`), or splits the work area (`Split`).
    fn make_room(&mut self, placed: Placed, work: &mut Work) -> Result<()> {
        if placed == Placed::Split { self.split(work) } else { self.flush(work) }
    }

    /// Splits the work area, once the groups held first fill it, if they
    /// took `PARTIAL_GAIN` rows or pieces each, as keys that repeat do:
    /// the later half of them written as pieces, their room for groups
    /// partly made; else none is made partly, and rows past them written.
    fn split(&mut self, work: &mut Work) -> Result<()> {
        (self.splits, self.spilled) = (false, true);
        if self.held.taken < self.held.len().saturating_mul(PARTIAL_GAIN) {
            return Ok(());
        }
        let half = self.held.len().div_ceil(2);
        for g in half..self.held.len() {
            self.overflow.write_group((&self.held, g), &mut self.queued, work)?;
        }
        self.held.truncate(half);
        let room = self.limit.saturating_sub(self.held.bytes());
        self.partial = Some(GroupTable::new(self.held.width(), self.held.aggs(), self.held.seed(), room));
        Ok(())
    }

    /// Writes the groups partly made as pieces, in `work`, each to the
    /// partition of its key, and empties them; makes none partly from
    /// then on if they took fewer than `PARTIAL_GAIN` rows or pieces each,
    /// as a piece is written in place of no more rows.
    fn flush(&mut self, work: &mut Work) -> Result<()> {
        let Self { partial, overflow, queued, .. } = self;
        let Some(table) = partial else { return Ok(()) };
        let gained = table.taken >= table.len().saturating_mul(PARTIAL_GAIN);
        for g in 0..table.len() {
            overflow.write_group((table, g), queued, work)?;
        }
        table.truncate(0);
        if !gained {
            *partial = None;
        }
        Ok(())
    }

    /// The groups held, finished, each numbered by its first row, in that
    /// order; where the rows or pieces of the others went, those partly
    /// made written first, in `work`; and whether it still made groups
    /// partly, as what it wrote is grouped so.
    fn finished(mut self, work: &mut Work) -> Result<(Finished, Overflow, bool)> {
        self.flush(work)?;
        self.overflow.write_queued(&mut self.queued, work)?;
        let mut held = self.held.finished()?;
        // Groups made of pieces are made in the order pieces come, not of
        // their first rows: put in that order, as the merge reads them.
        if self.pieced {
            held.sort_by_key(|g| g.0);
        }
        Ok((held, self.overflow, self.partial.is_some()))
    }
}

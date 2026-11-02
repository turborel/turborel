//! \[DESIGN\] Groups made by batch (RULES.md D31, D38): held in the work
//! area, in the order their first rows came, each its grouping values and
//! its set functions; each row's key made of the vectors in a kept buffer
//! (`put_key`), an integer's of a fixed width; each group's rows of a
//! batch gathered in order by a counting sort into one buffer, its set
//! functions fed them a vector at a time. Past the work area, the rows of
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
//! order it would without them. Split from batch.rs (RULES.md R-SIZE-2).

mod spilled;
mod table;

use std::sync::Arc;

use super::{Batches, Read, fed_group, lanes, put_key};
use crate::db::State;
use crate::error::Result;
use crate::exec::group::Finished;
use crate::exec::work::Work;
use crate::exec::{RowIter, Runner};
use crate::functions::Accumulator;
use crate::logical::AggCall;
use crate::storage::{Batch, Cell, Column, Row};
use crate::txn::TxId;
use spilled::{Overflow, Piece, Queued};
use table::{Held, Table};

/// The rows or pieces groups partly made take each, on average, for a
/// grouping to go on making them: past fewer, its pieces, each costing
/// more to write and read back than a row written by batch, would save
/// little (`Groups::flush`).
const PARTIAL_GAIN: usize = 4;

/// What a grouping groups and computes: what it reads of each batch, its
/// set functions, what hears their warnings, and whether it makes groups
/// partly before they spill.
#[derive(Clone, Copy)]
struct Grouping<'a> {
    read: &'a Read,
    aggs: &'a [AggCall],
    runner: &'a Runner,
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
    let partial = aggs.iter().all(|a| a.func.0.accumulator(a.result).write_state(&mut Vec::new()));
    let how = Grouping { read, aggs, runner: &runner, partial };
    // Rows past the work area are written by batch, the cheapest to read
    // back once; groups are made partly only of a partition that outgrows
    // it again, whose rows would be written again (`spilled::regrouped`).
    let mut groups = Groups::new(work.limit(), Overflow::Partitions(None), false);
    let mut rows = 0_u64;
    for kept in batches {
        let (batch, given) = kept?;
        groups.feed((&batch, given.as_deref()), Numbers::From(rows), how, &mut work)?;
        rows = rows.saturating_add(u64::try_from(given.as_ref().map_or(batch.rows, Vec::len)).unwrap_or(u64::MAX));
    }
    let (held, overflow, _) = groups.finished(&mut work)?;
    spilled::merged(held, overflow, how, &mut work)
}

/// The rows `placed`, each its group's place among `groups` of them, its
/// place among the rows given and in the batch, gathered by group in order:
/// the places among those given, those in the batch, and where each
/// group's end.
fn gathered(placed: &[(usize, usize, usize)], groups: usize) -> (Vec<usize>, Vec<usize>, Vec<usize>) {
    // Already gathered, each group's rows after the last's, as of runs.
    if placed.is_sorted_by_key(|&(slot, ..)| slot) {
        let mut ends = vec![0_usize; groups];
        for (at, &(slot, ..)) in placed.iter().enumerate() {
            if let Some(end) = ends.get_mut(slot) {
                *end = at.saturating_add(1);
            }
        }
        return (placed.iter().map(|&(_, n, _)| n).collect(), placed.iter().map(|&(.., r)| r).collect(), ends);
    }
    let mut ends = vec![0_usize; groups];
    for &(slot, ..) in placed {
        if let Some(end) = ends.get_mut(slot) {
            *end = end.saturating_add(1);
        }
    }
    let mut at = 0_usize;
    let mut next: Vec<usize> = ends
        .iter_mut()
        .map(|end| {
            let first = at;
            at = at.saturating_add(*end);
            *end = at;
            first
        })
        .collect();
    let (mut given, mut read_rows) = (vec![0_usize; placed.len()], vec![0_usize; placed.len()]);
    for &(slot, n, r) in placed {
        let Some(to) = next.get_mut(slot) else { continue };
        if let (Some(g), Some(b)) = (given.get_mut(*to), read_rows.get_mut(*to)) {
            (*g, *b) = (n, r);
        }
        *to = to.saturating_add(1);
    }
    (given, read_rows, ends)
}

/// \[DESIGN\] (RULES.md D31) Of a grouping by `groups`, one column of
/// `batch` whose integers are known as runs (`Ints::runs`), none null,
/// where each run of its rows `rows`, or all, ends among them, the rows of
/// each run alike, so of one group: none of another grouping, nor of a
/// null among the runs, of the null group.
fn run_ends(batch: &Batch, groups: &[usize], rows: Option<&[usize]>) -> Option<Vec<usize>> {
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

/// A group of a grouping: one held, or one partly made, by its place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    Held(usize),
    Partial(usize),
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

/// The groups of a batch's rows, in the order they came, and of each row
/// its group's place among them, its place among those given and in the
/// batch.
type Segment = (Vec<Target>, Vec<(usize, usize, usize)>);

/// Groups being made, in the work area.
struct Groups {
    /// The groups held, while they fit the work area.
    held: Table,
    /// The work area's bytes.
    limit: usize,
    /// Whether, the set functions writing their states, the work area is
    /// still to be split once the groups held fill it.
    splits: bool,
    /// The groups partly made of the rows past them, in the other half,
    /// if the set functions write their states.
    partial: Option<Table>,
    /// The key of the row being grouped.
    key: Vec<u8>,
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
    /// No groups yet, held while they take no more than `limit` bytes; if
    /// groups may be made `partial`ly, the area split once they fill it;
    /// what is past them written to `overflow`.
    fn new(limit: usize, overflow: Overflow, partial: bool) -> Groups {
        let (key, pending, scratch) = (Vec::new(), Vec::new(), Vec::new());
        Groups {
            held: Table::new(limit),
            limit,
            splits: partial,
            partial: None,
            key,
            overflow,
            pending,
            queued: Queued::default(),
            spilled: false,
            pieced: false,
            scratch,
        }
    }

    /// Feeds the rows of `batch` at `rows`, or all, numbered by `numbers`,
    /// to their groups' set functions, as `how` says; the rows of groups
    /// past the work area are written, by batch, in `work`, for their
    /// groups to be made after, or their groups made partly, written as
    /// pieces in `work` once those are full.
    fn feed(&mut self, (batch, rows): (&Batch, Option<&[usize]>), numbers: Numbers<'_>, how: Grouping<'_>, work: &mut Work) -> Result<()> {
        let lanes = lanes(&how.read.taken, batch, rows);
        let mut segment: Segment = (Vec::new(), Vec::new());
        // Of a grouping by one column of runs, a row of the run of the row
        // before it goes to that row's group, no key made.
        let mut ends = run_ends(batch, &how.read.groups, rows).into_iter().flatten();
        let (mut boundary, mut last) = (0_usize, None);
        for n in 0..rows.map_or(batch.rows, <[usize]>::len) {
            let r = rows.map_or(Some(n), |rows| rows.get(n).copied()).unwrap_or(n);
            if n >= boundary {
                boundary = ends.next().unwrap_or(n.saturating_add(1));
            } else if let Some(target) = last {
                self.took(target, (n, r), (how, &mut segment));
                continue;
            }
            // The key made in a buffer kept, no value copied unless its
            // group is new, nor made of an integer.
            self.key.clear();
            for &at in &how.read.groups {
                put_key(batch.cell(at, r), &mut self.key);
            }
            let values = || how.read.groups.iter().map(|&at| batch.cell(at, r).to_value()).collect();
            let mut placed = self.place((numbers.of(n), Some(r)), values, how, work)?;
            while let Placed::Full | Placed::Split = placed {
                // The rows given so far fed, so the pieces written hold them.
                self.fed((batch, &lanes), &mut segment, how)?;
                self.make_room(placed, work)?;
                placed = self.place((numbers.of(n), Some(r)), values, how, work)?;
            }
            if let (Placed::Into(target), false) = (placed, how.aggs.is_empty()) {
                let slot = self.slot_of(target, &mut segment.0);
                segment.1.push((slot, n, r));
            }
            last = if let Placed::Into(target) = placed { Some(target) } else { None };
        }
        self.overflow.write(batch, (&mut self.pending, &mut self.queued), work)?;
        self.fed((batch, &lanes), &mut segment, how)
    }

    /// Takes the `n`th row given, `r`, into group `target`, the group of
    /// the row before it, of the same run: counted as taken, and noted in
    /// `segment` to be fed, if `how` computes set functions.
    fn took(&mut self, target: Target, (n, r): (usize, usize), (how, segment): (Grouping<'_>, &mut Segment)) {
        let (table, _) = table_in((&mut self.held, &mut self.partial), target);
        if let Some(table) = table {
            table.taken = table.taken.saturating_add(1);
        }
        if !how.aggs.is_empty() {
            let slot = self.slot_of(target, &mut segment.0);
            segment.1.push((slot, n, r));
        }
    }

    /// Feeds each group of `segment` its rows of `batch`, gathered in
    /// order, a counting sort into one buffer, a vector at a time; and
    /// empties it.
    fn fed(&mut self, (batch, lanes): (&Batch, &[Option<super::Fed<'_>>]), segment: &mut Segment, how: Grouping<'_>) -> Result<()> {
        let (groups, placed) = segment;
        let (given, read_rows, ends) = gathered(placed, groups.len());
        let mut from = 0;
        for (&target, &end) in groups.iter().zip(&ends) {
            let (table, at) = table_in((&mut self.held, &mut self.partial), target);
            let Some(table) = table else { continue };
            if let Some(slot) = table.slots.get_mut(at) {
                *slot = 0;
            }
            let (these, read_these) = (given.get(from..end).unwrap_or_default(), read_rows.get(from..end).unwrap_or_default());
            from = end;
            let Some((_, _, states)) = table.groups.get_mut(at) else { continue };
            fed_group(states, (&how.read.taken, &mut self.scratch, how.runner), (batch, lanes), (these, read_these))?;
        }
        groups.clear();
        placed.clear();
        Ok(())
    }

    /// The place of group `target` among `groups`, the groups the batch
    /// has rows of, added if it is new.
    fn slot_of(&mut self, target: Target, groups: &mut Vec<Target>) -> usize {
        let (Some(table), at) = table_in((&mut self.held, &mut self.partial), target) else { return 0 };
        if table.slots.len() <= at {
            table.slots.resize(at.saturating_add(1), 0);
        }
        let Some(slot) = table.slots.get_mut(at) else { return 0 };
        if let Some(found) = slot.checked_sub(1) {
            return found;
        }
        groups.push(target);
        *slot = groups.len();
        groups.len().saturating_sub(1)
    }

    /// Where the row `r` of a batch, or a piece (no `r`), the `number`th,
    /// of the key made and grouping `values`, goes: its group held, or made
    /// while those held fit, so a grouping holds one at least; past them,
    /// as `past` says.
    #[expect(
        clippy::inline_always,
        reason = "R-DEV-1: a step of every row grouped, kept inline: as a call it adds a call a row, which a grouping's instructions show"
    )]
    #[inline(always)]
    fn place(&mut self, (number, r): (u64, Option<usize>), values: impl Fn() -> Row, how: Grouping<'_>, work: &mut Work) -> Result<Placed> {
        let at = if let Some(at) = self.held.get(&self.key) {
            at
        } else {
            if self.spilled || self.held.full() {
                return self.past(number, (r, values), how, work);
            }
            self.held.add(&self.key, fresh(number, values(), how))
        };
        self.held.taken = self.held.taken.saturating_add(1);
        Ok(Placed::Into(Target::Held(at)))
    }

    /// Where a row or piece of a group not held goes: nowhere till the
    /// work area is split, if it may be and the groups held first fill
    /// it; its group partly made, or none while those are full; else a row
    /// is to be written (`pending`), its partition found in `work`, and a
    /// piece passed on.
    #[expect(
        clippy::inline_always,
        reason = "R-DEV-1: a step of every row grouped, kept inline: as a call it adds a call a row, which a grouping's instructions show"
    )]
    #[inline(always)]
    fn past(&mut self, number: u64, (r, values): (Option<usize>, impl Fn() -> Row), how: Grouping<'_>, work: &mut Work) -> Result<Placed> {
        if self.splits {
            return Ok(Placed::Split);
        }
        self.spilled = true;
        let Some(partial) = &mut self.partial else {
            let Some(r) = r else { return Ok(Placed::Passed) };
            let part = self.overflow.part_of(&self.key, work)?;
            self.pending.push((part, r, number));
            return Ok(Placed::Written);
        };
        let at = if let Some(at) = partial.get(&self.key) {
            at
        } else {
            if partial.full() {
                return Ok(Placed::Full);
            }
            partial.add(&self.key, fresh(number, values(), how))
        };
        partial.taken = partial.taken.saturating_add(1);
        Ok(Placed::Into(Target::Partial(at)))
    }

    /// Takes in `piece`, as `how` says: merged into its group, held or
    /// partly made, made if new, in the order pieces come; those partly
    /// made written in `work` once full.
    fn absorb(&mut self, piece: &Piece, how: Grouping<'_>, work: &mut Work) -> Result<()> {
        self.pieced = true;
        self.key.clear();
        for v in &piece.values {
            put_key(Cell::Value(v), &mut self.key);
        }
        let values = || piece.values.clone();
        let mut placed = self.place((piece.first, None), values, how, work)?;
        while let Placed::Full | Placed::Split = placed {
            self.make_room(placed, work)?;
            placed = self.place((piece.first, None), values, how, work)?;
        }
        // Groups not made partly: a piece of one not held goes on.
        if placed == Placed::Passed {
            return self.overflow.write_piece(&self.key, piece, &mut self.queued, work);
        }
        let Placed::Into(target) = placed else { return Err(spilled::lost()) };
        let (table, at) = table_in((&mut self.held, &mut self.partial), target);
        let Some((first, _, states)) = table.and_then(|t| t.groups.get_mut(at)) else { return Err(spilled::lost()) };
        *first = (*first).min(piece.first);
        for (state, written) in states.iter_mut().zip(&piece.states) {
            state.merge(written)?;
        }
        Ok(())
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
        if self.held.taken < self.held.groups.len().saturating_mul(PARTIAL_GAIN) {
            return Ok(());
        }
        for (key, group) in self.held.split_off(self.held.groups.len().div_ceil(2)) {
            self.overflow.write_group((&key, &group), &mut self.queued, work)?;
        }
        self.partial = Some(Table::new(self.limit.saturating_sub(self.held.bytes())));
        Ok(())
    }

    /// Writes the groups partly made as pieces, in `work`, each to the
    /// partition of its key, and empties them; makes none partly from
    /// then on if they took fewer than `PARTIAL_GAIN` rows or pieces each,
    /// as a piece is written in place of no more rows.
    fn flush(&mut self, work: &mut Work) -> Result<()> {
        let Some(partial) = &mut self.partial else { return Ok(()) };
        let gained = partial.taken >= partial.groups.len().saturating_mul(PARTIAL_GAIN);
        for (key, group) in partial.drained() {
            self.overflow.write_group((&key, &group), &mut self.queued, work)?;
        }
        if !gained {
            self.partial = None;
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
        // Groups made of pieces are made in the order pieces come, not of
        // their first rows: put in that order, as the merge reads them.
        let mut groups = self.held.into_groups();
        if self.pieced {
            groups.sort_by_key(|g| g.0);
        }
        let held = groups
            .into_iter()
            .map(|(first, mut row, states)| {
                for state in &states {
                    row.push(state.finish()?);
                }
                Ok((first, row))
            })
            .collect::<Result<_>>()?;
        Ok((held, self.overflow, self.partial.is_some()))
    }
}

/// The table of `target`, of the groups `held` and those `partial`ly
/// made, if it has one, and its place there.
fn table_in<'t>((held, partial): (&'t mut Table, &'t mut Option<Table>), target: Target) -> (Option<&'t mut Table>, usize) {
    match target {
        Target::Held(at) => (Some(held), at),
        Target::Partial(at) => (partial.as_mut(), at),
    }
}

/// A new group, the `number`th row its first, of grouping values `values`,
/// its set functions those of `how`, fed nothing.
fn fresh(number: u64, values: Row, how: Grouping<'_>) -> Held {
    let states: Vec<Box<dyn Accumulator>> = how.aggs.iter().map(|a| a.func.0.accumulator(a.result)).collect();
    (number, values, states)
}

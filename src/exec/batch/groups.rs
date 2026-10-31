//! \[DESIGN\] Groups made by batch (RULES.md D31, D38): held in the work
//! area, in the order their first rows came, each its grouping values and
//! its set functions; each row's key made of the vectors in a kept buffer
//! (`put_key`), an integer's of a fixed width; each group's rows of a
//! batch gathered in order by a counting sort into one buffer, its set
//! functions fed them a vector at a time. Once the work area is full, the
//! rows of the groups not held are written by batch (`spilled.rs`): of
//! each batch, those of each partition one record, of their vectors, and
//! each partition grouped again by batch the same way. Split from
//! batch.rs (RULES.md R-SIZE-2).

mod spilled;

use std::collections::HashMap;
use std::sync::Arc;

use super::{Batches, Read, fed_group, lanes, put_key};
use crate::db::State;
use crate::error::Result;
use crate::exec::group::{Finished, STATE_BYTES};
use crate::exec::work::Work;
use crate::exec::{RowIter, Runner};
use crate::functions::Accumulator;
use crate::logical::AggCall;
use crate::spill::row_bytes;
use crate::storage::{Batch, Row};
use crate::txn::TxId;
use spilled::Overflow;

/// What a grouping groups and computes: what it reads of each batch, its
/// set functions, and what hears their warnings.
#[derive(Clone, Copy)]
struct Grouping<'a> {
    read: &'a Read,
    aggs: &'a [AggCall],
    runner: &'a Runner,
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
/// `tx`.
pub(super) fn grouped_all(read: &Read, aggs: &[AggCall], (db, tx): (&Arc<State>, TxId), batches: Batches) -> Result<RowIter> {
    let (mut work, runner) = (Work::of(db, tx), Runner::new(db, tx));
    let how = Grouping { read, aggs, runner: &runner };
    let mut groups = Groups::new(work.limit(), Overflow::Partitions(None));
    let mut rows = 0_u64;
    for kept in batches {
        let (batch, given) = kept?;
        groups.feed((&batch, given.as_deref()), Numbers::From(rows), how, &mut work)?;
        rows = rows.saturating_add(u64::try_from(given.as_ref().map_or(batch.rows, Vec::len)).unwrap_or(u64::MAX));
    }
    let (held, overflow) = groups.finished()?;
    spilled::merged(held, overflow, how, &mut work)
}

/// The rows `placed`, each its group's place among `groups` of them, its
/// place among the rows given and in the batch, gathered by group in order:
/// the places among those given, those in the batch, and where each
/// group's end.
fn gathered(placed: &[(usize, usize, usize)], groups: usize) -> (Vec<usize>, Vec<usize>, Vec<usize>) {
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

/// A group held: its first row's number, its grouping values and its set
/// functions.
type Held = (u64, Row, Vec<Box<dyn Accumulator>>);

/// Groups being made, in the work area.
struct Groups {
    /// Each group's place in `held`, by its grouping key.
    found: HashMap<Vec<u8>, usize>,
    /// Each group's first row's number, grouping values and set
    /// functions, in the order their first rows came.
    held: Vec<Held>,
    bytes: usize,
    limit: usize,
    /// The key of the row being grouped.
    key: Vec<u8>,
    /// Where the rows of groups past the work area go.
    overflow: Overflow,
    /// The rows of the batch being grouped to be written there: each one's
    /// partition, place in the batch and number.
    pending: Vec<(usize, usize, u64)>,
    /// Whether a group has been past it: every group not yet held is then,
    /// so no group is both held and written.
    spilled: bool,
    /// The row an expression a set function takes is evaluated of.
    scratch: Row,
    /// Of each group held, one more than its place among the groups a batch
    /// has rows of, or 0 if the batch has none of them yet.
    slots: Vec<usize>,
}

impl Groups {
    /// No groups yet, held while they take no more than `limit` bytes, the
    /// rows of others written to `overflow`.
    fn new(limit: usize, overflow: Overflow) -> Groups {
        let (found, held, key, pending, scratch, slots) = (HashMap::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
        Groups { found, held, bytes: 0, limit, key, overflow, pending, spilled: false, scratch, slots }
    }

    /// Feeds the rows of `batch` at `rows`, or all, numbered by `numbers`,
    /// to their groups' set functions, as `how` says; the rows of groups
    /// past the work area are written, by batch, in `work`, for their
    /// groups to be made after.
    fn feed(&mut self, (batch, rows): (&Batch, Option<&[usize]>), numbers: Numbers<'_>, how: Grouping<'_>, work: &mut Work) -> Result<()> {
        let lanes = lanes(&how.read.taken, batch, rows);
        // The groups of the batch's rows, in the order they came, and of each
        // row its group's place among them, and its place among those given
        // and in the batch.
        let (mut groups, mut placed): (Vec<usize>, Vec<(usize, usize, usize)>) = (Vec::new(), Vec::new());
        for n in 0..rows.map_or(batch.rows, <[usize]>::len) {
            let r = rows.map_or(Some(n), |rows| rows.get(n).copied()).unwrap_or(n);
            // The key made in a buffer kept, no value copied unless its
            // group is new, nor made of an integer.
            self.key.clear();
            for &at in &how.read.groups {
                put_key(batch.cell(at, r), &mut self.key);
            }
            if let (Some(at), false) = (self.place(numbers.of(n), (batch, r), how, work)?, how.aggs.is_empty()) {
                placed.push((self.slot_of(at, &mut groups), n, r));
            }
        }
        self.overflow.write(batch, &mut self.pending, work)?;
        // Each group's rows gathered, in order, a counting sort into one
        // buffer, and its set functions fed them a vector at a time.
        let (given, read_rows, ends) = gathered(&placed, groups.len());
        let mut from = 0;
        for (&at, &end) in groups.iter().zip(&ends) {
            if let Some(slot) = self.slots.get_mut(at) {
                *slot = 0;
            }
            let (these, read_these) = (given.get(from..end).unwrap_or_default(), read_rows.get(from..end).unwrap_or_default());
            from = end;
            let Some((_, _, states)) = self.held.get_mut(at) else { continue };
            fed_group(states, (&how.read.taken, &mut self.scratch, how.runner), (batch, &lanes), (these, read_these))?;
        }
        Ok(())
    }

    /// The place of group `at` among `groups`, the groups the batch has
    /// rows of, added if it is new.
    fn slot_of(&mut self, at: usize, groups: &mut Vec<usize>) -> usize {
        if self.slots.len() <= at {
            self.slots.resize(at.saturating_add(1), 0);
        }
        let Some(slot) = self.slots.get_mut(at) else { return 0 };
        if let Some(found) = slot.checked_sub(1) {
            return found;
        }
        groups.push(at);
        *slot = groups.len();
        groups.len().saturating_sub(1)
    }

    /// The place of the group of row `r` of `batch`, the `number`th, its
    /// key made: found, or made while the groups held fit the work area,
    /// so a grouping holds one at least; else the row is to be written
    /// (`pending`), its partition found in `work`, and it has none.
    fn place(&mut self, number: u64, (batch, r): (&Batch, usize), how: Grouping<'_>, work: &mut Work) -> Result<Option<usize>> {
        if let Some(&at) = self.found.get(self.key.as_slice()) {
            return Ok(Some(at));
        }
        if self.spilled || self.bytes > self.limit {
            self.spilled = true;
            let part = self.overflow.part_of(&self.key, work)?;
            self.pending.push((part, r, number));
            return Ok(None);
        }
        let values: Row = how.read.groups.iter().map(|&at| batch.cell(at, r).to_value()).collect();
        let bytes = self.key.len().saturating_add(row_bytes(&values)).saturating_add(how.aggs.len().saturating_mul(STATE_BYTES));
        self.bytes = self.bytes.saturating_add(bytes);
        self.held.push((number, values, how.aggs.iter().map(|a| a.func.0.accumulator(a.result)).collect()));
        let at = self.held.len().saturating_sub(1);
        self.found.insert(self.key.clone(), at);
        Ok(Some(at))
    }

    /// The groups held, finished, each numbered by its first row, in that
    /// order; and where the rows of the others went.
    fn finished(self) -> Result<(Finished, Overflow)> {
        let held = self
            .held
            .into_iter()
            .map(|(first, mut row, states)| {
                for state in &states {
                    row.push(state.finish()?);
                }
                Ok((first, row))
            })
            .collect::<Result<_>>()?;
        Ok((held, self.overflow))
    }
}

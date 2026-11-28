//! \[DESIGN\] (RULES.md D1324) The tables of a pair of groupings whose
//! plain set functions are kept by the second's columns alone. The first
//! grouping's table then holds keys and no state, and a table beside it,
//! the side, holds one group a value of the second's grouping columns
//! with each plain function's part, a column of counts or of exact
//! totals (RULES.md D1280). The second grouping's table is fed the
//! first's groups for its other set functions, and takes each plain
//! function's states from the side, a group's from the side group of its
//! key, by the loop two partitions' columns are merged by. A side group
//! lies in the partition its key's hash names, the hash the first's
//! groups of that key were parted by (`made.rs`), so the lookup locks
//! one partition's side at a time.

use std::sync::{Mutex, MutexGuard, PoisonError};

use super::made::part_of;
use super::states::{States, Typed, merge_column};
use super::{Columns, GroupTable, Key, Keys, Pending, Seed, Slots, narrow};
use crate::error::Result;
use crate::exec::batch::{Fed, Taken};
use crate::exec::{Runner, broken};
use crate::logical::AggCall;
use crate::storage::mask::index;
use crate::storage::{Batch, Row};

/// A side table's states: a part's column of counts or of exact totals
/// for each plain set function. Columns alone, so the table goes from
/// thread to thread as it is, where a table of accumulators is frozen
/// first.
pub(in crate::exec) struct Parts(Vec<Typed>);

impl Columns for Parts {
    fn grow(&mut self) {
        self.0.iter_mut().for_each(|part| part.grow_by(1));
    }

    fn truncate(&mut self, len: usize) {
        let kept: Vec<u32> = (0..len).map(narrow).collect();
        self.0.iter_mut().for_each(|part| *part = part.gathered(&kept));
    }

    fn bytes(&self) -> usize {
        self.0.iter().map(Typed::bytes).sum()
    }
}

/// A side's groups, a table a partition, none of a partition no row came
/// to yet.
pub(in crate::exec) type Sides = Vec<Mutex<Option<GroupTable<Parts>>>>;

/// The table of partition `part` among `sides`, locked.
fn locked(sides: &Sides, part: usize) -> Result<MutexGuard<'_, Option<GroupTable<Parts>>>> {
    let slot = sides.get(part).ok_or_else(|| broken("a side of no such partition"))?;
    Ok(slot.lock().unwrap_or_else(PoisonError::into_inner))
}

impl GroupTable {
    /// Whether each of `aggs`, one at least, is a part kept as a column
    /// of counts or of exact totals, which a second grouping merges as it
    /// is and in any order (`States::is_merged_whole`).
    pub(in crate::exec) fn parts_whole(aggs: &[AggCall]) -> bool {
        !aggs.is_empty() && aggs.iter().all(|agg| States::of(agg).is_merged_whole())
    }

    /// Of each place of `from` that names one of `aggs`, a part's column
    /// of no group, the kind `merging` keeps the function merging it in.
    pub(in crate::exec) fn part_kinds(aggs: &[AggCall], from: &[Option<usize>]) -> Vec<Option<Typed>> {
        from.iter().map(|at| at.and_then(|at| aggs.get(at)).and_then(|agg| States::of(agg).into_part())).collect()
    }

    /// Each of its groups with the partition its key's hash names among
    /// `parts` and that hash, by `seed`, in the order of the partitions:
    /// the hash a side's groups are placed by, of the whole key, as
    /// `Keys::hash_of` hashes it of any table.
    fn by_side(&self, seed: &Seed, parts: usize) -> Vec<(usize, u32, u64)> {
        let lanes: Vec<usize> = (0..self.width()).collect();
        let hashed = |g: usize| self.keys.hash_of(g, &lanes, seed);
        let mut by: Vec<(usize, u32, u64)> = (0..self.len()).map(|g| (part_of(hashed(g), parts), narrow(g), hashed(g))).collect();
        by.sort_unstable_by_key(|&(part, g, _)| (part, g));
        by
    }

    /// A side table of no group yet, of this table's key and a column of
    /// each of its parts' kinds.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if one of its set functions is kept in no part's
    /// column.
    fn side_table(&self) -> Result<GroupTable<Parts>> {
        let part = |states: &States| if let States::Part(typed) = states { Some(typed.gathered(&[])) } else { None };
        let parts =
            self.states.iter().map(part).collect::<Option<Vec<Typed>>>().ok_or_else(|| broken("a side's part kept in no column"))?;
        Ok(GroupTable {
            aggs: Vec::new(),
            keys: Keys::new(self.width()).of_seed(&self.seed),
            hashes: Vec::new(),
            firsts: Vec::new(),
            states: Parts(parts),
            slots: Slots::of(&[]),
            keyed: None,
            seed: self.seed.clone(),
            pending: Pending::default(),
            scratch: Vec::new(),
            limit: usize::MAX,
            taken: 0,
            merging: (Vec::new(), Vec::new()),
            apart: false,
            placed: None,
        })
    }

    /// Merges its groups, a morsel's side groups, into `sides`: each into
    /// the table of the partition its key's hash names, found there by
    /// its key or made, each part's states added to that group's. The
    /// bytes the tables grew by.
    ///
    /// # Errors
    ///
    /// As `Accumulator::merge`, a sum past 128 bits; SQLCODE -910 if a
    /// set function of it is kept in no part's column.
    pub(in crate::exec) fn merge_into_sides(&self, sides: &Sides) -> Result<usize> {
        let (by, mut grown) = (self.by_side(&self.seed, sides.len()), 0_usize);
        for found in by.chunk_by(|a, b| a.0 == b.0) {
            let mut held = locked(sides, found.first().map_or(0, |&(part, ..)| part))?;
            if held.is_none() {
                *held = Some(self.side_table()?);
            }
            let side = held.as_mut().ok_or_else(|| broken("a side table not made"))?;
            let before = side.bytes();
            let places: Vec<u32> = found.iter().map(|&(_, g, _)| g).collect();
            let made = |&(_, g, hash): &(usize, u32, u64)| side.found_or_made(hash, Key::Group(&self.keys, index(g)), self.first(index(g)));
            let groups: Vec<u32> = found.iter().map(made).collect();
            for (mine, theirs) in side.states.0.iter_mut().zip(&self.states) {
                let States::Part(part) = theirs else { return Err(broken("a side's part kept in no column")) };
                merge_column(mine, part, (&groups, &places))?;
            }
            grown = grown.saturating_add(side.bytes().saturating_sub(before));
        }
        Ok(grown)
    }

    /// Its groups as `into_batch` gives them, put in the order of their
    /// first rows if they are not in it, the order a second grouping's
    /// set functions take them in.
    ///
    /// # Errors
    ///
    /// As `into_batch`.
    pub(in crate::exec) fn into_ordered_batch(self) -> Result<(Batch, Vec<u64>)> {
        let (batch, firsts) = self.into_batch()?;
        if firsts.is_sorted() {
            return Ok((batch, firsts));
        }
        let mut order: Vec<usize> = (0..firsts.len()).collect();
        order.sort_unstable_by_key(|&g| firsts.get(g).copied().unwrap_or(u64::MAX));
        let picks: Vec<Option<usize>> = order.iter().copied().map(Some).collect();
        let columns = batch.columns.iter().map(|column| column.gathered(&picks)).collect();
        let ordered = order.iter().map(|&g| firsts.get(g).copied().unwrap_or(u64::MAX)).collect();
        Ok((Batch { columns, rows: batch.rows }, ordered))
    }

    /// Feeds the rows taken of `batch` as `feed` feeds them, but to no
    /// set function `from` names a part for, whose states come of the
    /// side (`merge_side`).
    ///
    /// # Errors
    ///
    /// As `feed`.
    pub(in crate::exec) fn feed_unmerged(
        &mut self,
        batch: &Batch,
        (taken, lanes): (&[Taken], &[Option<Fed<'_>>]),
        from: &[Option<usize>],
        (scratch, runner): (&mut Row, &Runner),
    ) -> Result<()> {
        if self.pending.groups.is_empty() {
            return Ok(());
        }
        let each = self.states.iter_mut().zip(&self.aggs).zip(taken.iter().zip(lanes)).zip(from);
        for (((states, agg), (take, lane)), part) in each {
            if part.is_none() {
                states.feed((&self.pending, agg), (batch, take, lane), (scratch, runner))?;
            }
        }
        self.pending.given.clear();
        self.pending.read.clear();
        self.pending.groups.clear();
        Ok(())
    }

    /// Merges into each of its groups the states of its side group, the
    /// group of its key among `sides`, hashed by `seed`: each set
    /// function `from` names a part for takes that part's column of the
    /// side as `States::merge_part` merges one.
    ///
    /// # Errors
    ///
    /// As `States::merge_part`; SQLCODE -910 if a group has no side
    /// group, or the side keeps no column of a part named.
    pub(in crate::exec) fn merge_side(&mut self, (sides, seed): (&Sides, &Seed), from: &[Option<usize>]) -> Result<()> {
        for found in self.by_side(seed, sides.len()).chunk_by(|a, b| a.0 == b.0) {
            let mut held = locked(sides, found.first().map_or(0, |&(part, ..)| part))?;
            let side = held.as_mut().ok_or_else(|| broken("a group of a pair in a partition of no side"))?;
            let groups: Vec<u32> = found.iter().map(|&(_, g, _)| g).collect();
            let mut places = Vec::with_capacity(found.len());
            for &(_, g, hash) in found {
                let place = side.find(hash, Key::Group(&self.keys, index(g)));
                places.push(place.ok_or_else(|| broken("a group of a pair with no side group"))?);
            }
            for (states, at) in self.states.iter_mut().zip(from) {
                let Some(at) = at else { continue };
                let part = side.states.0.get(*at).ok_or_else(|| broken("a side with no column of a part"))?;
                states.merge_part(part, (&groups, &places))?;
            }
        }
        Ok(())
    }
}

// Unit tests: a second table that takes its plain functions of a side
// gives what one merging the first's parts gives (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;

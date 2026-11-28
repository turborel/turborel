//! \[DESIGN\] (RULES.md D1313) A morsel's rows handed on ungrouped,
//! written once where they stay. A morsel whose rows a table would not
//! reduce hands each row on as a group of its own (RULES.md D1260), split
//! among the partitions as any morsel's groups are. Appending each row's
//! key, hash, first row's number and states in the order the rows come
//! and then putting every column in the partitions' order writes each
//! group twice. Here the morsel's rows are hashed first, the rows of each
//! partition counted, and each row given its place, after the rows
//! before it of its partition and of the partitions before. Then every
//! column is made at its size and each row's value written at its place:
//! the hashes, the first rows' numbers, the keys (`keys/placed.rs`), and
//! the states, grown once and fed each batch's rows with their places as
//! their groups, as a table's states are fed. What is handed over is what
//! the rows appended and gathered give, a partition's rows one after
//! another in the order they came.

use super::{Made, part_of};
use crate::error::Result;
use crate::exec::table::{GroupTable, Keys, Seed, narrow};
use crate::logical::AggCall;
use crate::storage::mask::index;
use crate::storage::{Batch, Sel};

/// Where the rows of a table written by partition lie: the place of each
/// row as it came, where each partition's rows begin and the last's end,
/// and the lanes of the keys the partitions are of, none of whole keys.
#[derive(Debug)]
pub(in crate::exec::table) struct Placed {
    places: Vec<u32>,
    starts: Vec<u32>,
    lanes: Vec<usize>,
}

/// Where each partition's rows begin among `parts` partitions, and the
/// last's end, and the place of each row of hash `by` as they come: its
/// partition's rows one after another in the order they came.
fn placed_by(by: &[u64], parts: usize) -> (Vec<u32>, Vec<u32>) {
    let mut starts = vec![0_u32; parts.saturating_add(1)];
    for &hash in by {
        if let Some(count) = starts.get_mut(part_of(hash, parts).saturating_add(1)) {
            *count = count.saturating_add(1);
        }
    }
    let mut sum = 0_u32;
    for start in &mut starts {
        sum = sum.saturating_add(*start);
        *start = sum;
    }
    let mut next = starts.clone();
    let places = by.iter().map(|&hash| {
        let Some(at) = next.get_mut(part_of(hash, parts)) else { return u32::MAX };
        let place = *at;
        *at = at.saturating_add(1);
        place
    });
    (starts, places.collect())
}

/// The `values`, as many as `places`, each at its place.
fn scattered(values: impl Iterator<Item = u64>, places: &[u32]) -> Vec<u64> {
    let mut out = vec![0_u64; places.len()];
    for (&place, value) in places.iter().zip(values) {
        if let Some(at) = out.get_mut(index(place)) {
            *at = value;
        }
    }
    out
}

impl GroupTable {
    /// \[DESIGN\] (RULES.md D1313) A table of the rows kept of `held`, a
    /// morsel's batches, each a group of its own, of the key of its
    /// columns at `at`, computing `aggs`, fed nothing yet: the `n`th row
    /// of hash `hashes[n]`, its first row numbered `first + n`. The
    /// groups lie by partition among `parts`, a row's partition of its
    /// hash, or of `by`'s, the hashes of its key's `lanes` alone, if
    /// given (RULES.md D1113). No group is looked for and none has a
    /// slot, as of `unplaced`.
    pub(in crate::exec) fn placed(
        (held, at): (&[(Batch, Option<Sel>)], &[usize]),
        (aggs, seed): (&[AggCall], &Seed),
        (hashes, by, first): (&[u64], Option<&[u64]>, u64),
        (parts, lanes): (usize, &[usize]),
    ) -> GroupTable {
        let (starts, places) = placed_by(by.unwrap_or(hashes), parts);
        let mut table = GroupTable::new(at.len(), aggs, seed, usize::MAX);
        table.keys = Keys::placed(held, at, &places, &table.keys);
        table.hashes = scattered(hashes.iter().copied(), &places);
        table.firsts = scattered((0_u64..).map(|n| first.saturating_add(n)), &places);
        table.states.iter_mut().for_each(|states| states.grow_by(places.len()));
        (table.apart, table.placed) = (true, Some(Placed { places, starts, lanes: lanes.to_vec() }));
        table
    }

    /// Takes the `count` rows kept of a batch, `rows` or all, the rows
    /// from the `from`th of those the table was placed of, each into the
    /// group at its place, to be fed to its states (`feed`).
    pub(in crate::exec) fn take_placed(&mut self, from: usize, (rows, count): (Option<&[u32]>, usize)) {
        self.taken = self.taken.saturating_add(count);
        let Self { placed, pending, aggs, .. } = self;
        let places = placed.as_ref().and_then(|placed| placed.places.get(from..from.saturating_add(count)));
        let Some(places) = places.filter(|_| !aggs.is_empty()) else { return };
        pending.given.extend((0..count).map(narrow));
        match rows {
            Some(rows) => pending.read.extend_from_slice(rows),
            None => pending.read.extend((0..count).map(narrow)),
        }
        pending.groups.extend_from_slice(places);
    }

    /// Where each partition's groups begin, and the last's end, if its
    /// groups were written by partition among `parts` by the lanes
    /// `lanes` (`placed`): what `made` hands over then, with no group
    /// moved. Forgets how its groups lie either way.
    pub(super) fn starts_placed(&mut self, parts: usize, lanes: &[usize]) -> Option<Vec<u32>> {
        let placed = self.placed.take()?;
        (placed.starts.len() == parts.saturating_add(1) && placed.lanes == lanes).then_some(placed.starts)
    }

    /// Its groups, made, as they lie, partition by partition from
    /// `starts`.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if an accumulator writes no state.
    pub(super) fn made_placed(mut self, starts: Vec<u32>) -> Result<Made> {
        // Each partition finds its groups by the hashes they came with.
        self.keyed = None;
        let groups = self.hashes.len();
        let mut made = self.frozen()?;
        (made.places, made.starts) = ((0..groups).map(narrow).collect(), starts);
        Ok(made)
    }
}

// Unit tests: rows written by partition are the rows appended and then
// gathered (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests;

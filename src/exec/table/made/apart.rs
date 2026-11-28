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
//! the states, a set function at a time. A column of states that one row
//! sets is made whole, each group's written once
//! (`states/typed/apart.rs`). Any other is grown once and fed each
//! batch's rows with their places as their groups, as a table's states
//! are fed. What is handed over is what the rows appended and gathered
//! give, a partition's rows one after another in the order they came.

use super::{Made, part_of};
use crate::error::Result;
use crate::exec::Runner;
use crate::exec::batch::{Fed, Taken, eliminated};
use crate::exec::table::{GroupTable, Keys, Pending, Seed, narrow};
use crate::logical::AggCall;
use crate::storage::mask::index;
use crate::storage::sel::nth;
use crate::storage::{Batch, Row, Sel};

mod runs;
pub(in crate::exec) use runs::Runs;

/// Where the rows of a table written by partition lie: the place of each
/// row as it came, the row at each place, its batch and its row in it,
/// where each partition's rows begin and the last's end, and the lanes
/// of the keys the partitions are of, none of whole keys.
#[derive(Debug)]
pub(in crate::exec::table) struct Placed {
    places: Vec<u32>,
    sources: Vec<(u32, u32)>,
    starts: Vec<u32>,
    lanes: Vec<usize>,
}

/// Where each row kept of `held` is read, the batch and the row in it,
/// in the order of the places `places` gives the rows as they come.
fn sources(held: &[(Batch, Option<Sel>)], places: &[u32]) -> Vec<(u32, u32)> {
    let mut order = vec![(0_u32, 0_u32); places.len()];
    let mut place = places.iter();
    for (b, (batch, rows)) in (0_u32..).zip(held) {
        let rows = rows.as_ref().map(Sel::as_slice);
        for n in 0..rows.map_or(batch.rows, <[u32]>::len) {
            let at = place.next().and_then(|&p| order.get_mut(index(p)));
            if let Some(at) = at {
                *at = (b, u32::try_from(nth(rows, n)).unwrap_or(u32::MAX));
            }
        }
    }
    order
}

/// Of each batch of `held`, in order, which of the rows kept of them all
/// are its own: from which to which, as they come.
fn spans(held: &[(Batch, Option<Sel>)]) -> Vec<(usize, usize)> {
    let mut from = 0_usize;
    let span = |(batch, rows): &(Batch, Option<Sel>)| {
        let to = from.saturating_add(rows.as_ref().map_or(batch.rows, Sel::len));
        let own = (from, to);
        from = to;
        own
    };
    held.iter().map(span).collect()
}

impl Placed {
    /// Notes in `pending` the rows kept of a batch, `rows` or all, those
    /// from `from` to `to` of the rows it was placed of, each to be fed
    /// to the group at its place, and nothing else.
    fn take(&self, pending: &mut Pending, (from, to): (usize, usize), rows: Option<&[u32]>) {
        let count = to.saturating_sub(from);
        pending.given.clear();
        pending.read.clear();
        pending.groups.clear();
        pending.given.extend((0..count).map(narrow));
        match rows {
            Some(rows) => pending.read.extend_from_slice(rows),
            None => pending.read.extend((0..count).map(narrow)),
        }
        pending.groups.extend_from_slice(self.places.get(from..to).unwrap_or(&[]));
    }
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
    /// of hash `hashes[n]`, its first row numbered the `n`th of
    /// `numbers`. The groups lie by partition among `parts`, a row's
    /// partition of its hash, or of `by`'s, the hashes of its key's
    /// `lanes` alone, if given (RULES.md D1113). No group is looked for
    /// and none has a slot, as of `unplaced`.
    pub(in crate::exec) fn placed(
        (held, at): (&[(Batch, Option<Sel>)], &[usize]),
        (aggs, seed): (&[AggCall], &Seed),
        (hashes, by, numbers): (&[u64], Option<&[u64]>, impl Iterator<Item = u64>),
        (parts, lanes): (usize, &[usize]),
    ) -> GroupTable {
        let (starts, places) = placed_by(by.unwrap_or(hashes), parts);
        let mut table = GroupTable::new(at.len(), aggs, seed, usize::MAX);
        let rows_at = sources(held, &places);
        table.keys = Keys::placed(held, at, (&places, &rows_at), &table.keys);
        table.hashes = scattered(hashes.iter().copied(), &places);
        table.firsts = scattered(numbers, &places);
        (table.apart, table.taken) = (true, places.len());
        table.placed = Some(Placed { places, sources: rows_at, starts, lanes: lanes.to_vec() });
        table
    }

    /// \[DESIGN\] (RULES.md D1313) Gives each group its row's states, of
    /// the batches `held` the table was placed of, a set function at a
    /// time, so one function's states alone are being written. A column
    /// of states that one row sets is made whole, each group's written
    /// once in the order they lie (`States::set_apart`). Any other is
    /// grown to the groups' count and fed each batch's rows with their
    /// places as their groups, as `taken` says, of `lanes`, a batch's
    /// each, where evaluated by vectors; an expression's row made in
    /// `scratch`, warnings heard by `runner`. Of batches held an entry a
    /// run, as `runs` says, each entry is given its run's states
    /// (`runs.rs`, RULES.md D1325).
    ///
    /// # Errors
    ///
    /// As a set function, or an expression it takes, fails.
    pub(in crate::exec) fn feed_placed(
        &mut self,
        (held, runs): (&[(Batch, Option<Sel>)], &Runs),
        (taken, lanes): (&[Taken], &[Vec<Option<Fed<'_>>>]),
        (scratch, runner): (&mut Row, &Runner),
    ) -> Result<()> {
        if runs.any() {
            return self.feed_runs((held, runs), (taken, lanes), (scratch, runner));
        }
        let Self { placed, states, aggs, pending, .. } = self;
        let Some(placed) = placed.as_ref() else { return Ok(()) };
        let spans = spans(held);
        for (s, ((column, agg), take)) in states.iter_mut().zip(aggs.iter()).zip(taken).enumerate() {
            if let Some(nulls) = column.set_apart(take, held, &placed.sources) {
                eliminated(nulls, runner);
                continue;
            }
            column.grow_by(placed.places.len());
            for (((batch, rows), of_batch), &span) in held.iter().zip(lanes).zip(&spans) {
                placed.take(pending, span, rows.as_ref().map(Sel::as_slice));
                if let Some(lane) = of_batch.get(s) {
                    column.feed((&*pending, agg), (batch, take, lane), (&mut *scratch, runner))?;
                }
            }
        }
        placed.take(pending, (0, 0), None);
        Ok(())
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

//! \[DESIGN\] (RULES.md D293, D356) A morsel's groups as a grouping run in
//! parallel hands them to its merge: each group's key, hash, first row's
//! number and states, the states of accumulators written, as they cannot
//! go to another thread, and the groups of each partition by their places.
//! A group's partition is of the high bits of its hash, the same of a key
//! in every morsel (`hash.rs`). Each partition is merged by a thread of
//! its own into a table made anew, its morsels in order, each group found
//! by the hash it came with, no key hashed again, and its states merged
//! slot by slot where both are columns of one kind (`states.rs`).

use crate::error::Result;
use crate::logical::AggCall;

use super::slots::MAX_PROBE;
use super::states::{Sent, States};
use super::{GroupTable, Key, Keys, Seed, narrow};

mod written;
pub(in crate::exec) use written::PIECE;

/// \[DESIGN\] The bits of a hash below those a partition is found by, as
/// a multiply-shift maps the high half onto any count of partitions.
const HALF: u32 = 32;

/// The partition of a group of hash `hash` among `parts`: of its high
/// half, so of other bits than its slot's.
fn part_of(hash: u64, parts: usize) -> usize {
    let parts = u64::try_from(parts).unwrap_or(u64::MAX);
    usize::try_from(((hash >> HALF).wrapping_mul(parts)) >> HALF).unwrap_or(0)
}

/// A morsel's groups, made, for the threads merging its partitions.
pub(in crate::exec) struct Made {
    keys: Keys,
    hashes: Vec<u64>,
    firsts: Vec<u64>,
    states: Vec<Sent>,
    /// The places of its groups, partition by partition, in one vector:
    /// each its own number, as the groups are held in that order.
    places: Vec<u32>,
    /// Where each partition's places begin in `places`, and the last's
    /// end; empty of groups frozen whole.
    starts: Vec<u32>,
    /// The keyed hashes its groups are placed by, if its table took them
    /// (RULES.md D357).
    keyed: Option<Vec<u64>>,
}

impl Made {
    /// The bytes it holds.
    pub(in crate::exec) fn bytes(&self) -> usize {
        let words = self.hashes.capacity().saturating_add(self.firsts.capacity()).saturating_mul(size_of::<u64>());
        let places = self.places.capacity().saturating_add(self.starts.capacity()).saturating_mul(size_of::<u32>());
        let states = self.states.iter().map(Sent::bytes).sum::<usize>();
        let keyed = self.keyed.as_ref().map_or(0, |k| k.capacity().saturating_mul(size_of::<u64>()));
        words.saturating_add(keyed).saturating_add(places).saturating_add(states).saturating_add(self.keys.bytes())
    }

    /// How many groups of partition `part` it holds.
    pub(in crate::exec) fn groups_of(&self, part: usize) -> usize {
        self.places_of(part).len()
    }

    /// The places of partition `part`'s groups, in their order.
    fn places_of(&self, part: usize) -> &[u32] {
        let bound = |at: usize| self.starts.get(at).map(|&start| crate::storage::mask::index(start));
        let range = bound(part).zip(part.checked_add(1).and_then(bound));
        range.and_then(|(from, to)| self.places.get(from..to)).unwrap_or(&[])
    }
}

impl GroupTable {
    /// Its groups, made, split among `parts` partitions by their hashes;
    /// or, of `lanes` some, by the hashes of their keys of those lanes
    /// alone, so every group of a value of them goes to one partition
    /// (RULES.md D1113).
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if an accumulator writes no state.
    pub(in crate::exec) fn made(mut self, parts: usize, lanes: &[usize]) -> Result<Made> {
        let by: Vec<u64> = if lanes.is_empty() {
            std::mem::take(&mut self.hashes)
        } else {
            (0..self.hashes.len()).map(|g| self.keys.hash_of(g, lanes, &self.seed)).collect()
        };
        // Counted, then placed: each partition's places one after another
        // in one vector made once (RULES.md D1110).
        let mut starts = vec![0_u32; parts.saturating_add(1)];
        for &hash in &by {
            if let Some(count) = starts.get_mut(part_of(hash, parts).saturating_add(1)) {
                *count = count.saturating_add(1);
            }
        }
        let mut sum = 0_u32;
        for start in &mut starts {
            sum = sum.saturating_add(*start);
            *start = sum;
        }
        let (mut places, mut next) = (vec![0_u32; by.len()], starts.clone());
        for (g, &hash) in by.iter().enumerate() {
            let Some(at) = next.get_mut(part_of(hash, parts)) else { continue };
            if let Some(place) = places.get_mut(crate::storage::mask::index(*at)) {
                *place = narrow(g);
            }
            *at = at.saturating_add(1);
        }
        if lanes.is_empty() {
            self.hashes = by;
        }
        // Each partition finds its groups by the hashes they came with.
        self.keyed = None;
        let mut made = self.frozen()?;
        // Its groups put in that order, so a partition's lie one after
        // another and the thread that merges them reads each column in
        // one run, not a group from here and one from there.
        let fresh = made.keys.like();
        let keys = std::mem::replace(&mut made.keys, fresh);
        made.keys.extend(&keys, &places);
        let picked = |words: &[u64]| places.iter().map(|&g| words.get(crate::storage::mask::index(g)).copied().unwrap_or(0)).collect();
        (made.hashes, made.firsts) = (picked(&made.hashes), picked(&made.firsts));
        made.states = made.states.iter().map(|sent| sent.gathered(&places)).collect();
        (made.places, made.starts) = ((0..places.len()).map(narrow).collect(), starts);
        Ok(made)
    }

    /// Its groups whole, in no partition, as they go to another thread to
    /// be taken up again there (`thawed`), none of their room spare.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if an accumulator writes no state.
    pub(in crate::exec) fn frozen(mut self) -> Result<Made> {
        let len = self.hashes.len();
        self.keys.truncate(len);
        self.hashes.shrink_to_fit();
        self.firsts.shrink_to_fit();
        self.states.iter_mut().for_each(|states| states.truncate(len));
        let states = self.states.into_iter().map(States::sent).collect::<Result<Vec<_>>>()?;
        Ok(Made {
            keys: self.keys,
            hashes: self.hashes,
            firsts: self.firsts,
            states,
            places: Vec::new(),
            starts: Vec::new(),
            keyed: self.keyed,
        })
    }

    /// The table of the groups of `made`, frozen, of the set functions
    /// `aggs`, its keys hashed by `seed`: its slots placed anew by the
    /// hashes it holds, with room for `more` groups it may yet take, so
    /// they are placed once however many come (RULES.md D1261); its
    /// columns of states as they were, each accumulator merged from its
    /// state written.
    ///
    /// # Errors
    ///
    /// As `Accumulator::merge`.
    pub(in crate::exec) fn thawed(made: Made, (aggs, seed): (&[AggCall], &Seed), more: usize) -> Result<GroupTable> {
        let len = made.hashes.len();
        let mut table = GroupTable::new(made.keys.width(), aggs, seed, usize::MAX);
        table.states = made.states.into_iter().zip(aggs).map(|(sent, agg)| States::thawed(sent, agg, len)).collect::<Result<_>>()?;
        table.slots.rebuilt_for(made.keyed.as_deref().unwrap_or(&made.hashes), more);
        (table.keys, table.hashes, table.firsts, table.keyed) = (made.keys.of_seed(seed), made.hashes, made.firsts, made.keyed);
        Ok(table)
    }

    /// Merges the groups of partition `part` of `made` into its own: each
    /// found by its key and hash, or made, its first row the earlier, its
    /// states merged after those it holds.
    ///
    /// # Errors
    ///
    /// As `Accumulator::merge`; a sum past 128 bits.
    pub(in crate::exec) fn absorb(&mut self, made: &Made, part: usize) -> Result<()> {
        self.absorb_places(made, made.places_of(part))
    }

    /// Merges every group of `made`, frozen whole, into its own, in their
    /// order, as `absorb` merges a partition's.
    ///
    /// # Errors
    ///
    /// As `absorb`.
    pub(in crate::exec) fn absorb_whole(&mut self, made: &Made) -> Result<()> {
        let places: Vec<u32> = (0..made.hashes.len()).map(narrow).collect();
        self.absorb_places(made, &places)
    }

    /// Room for `more` groups more, their keys in lanes as `like`'s, so
    /// merging a wave's morsels grows no vector a group at a time, and
    /// room in its slots for them too, so no group is placed again as
    /// they come (RULES.md D1261).
    pub(in crate::exec) fn reserve(&mut self, like: &Made, more: usize) {
        self.keys.reserve(&like.keys, more);
        self.hashes.reserve(more);
        self.firsts.reserve(more);
        self.states.iter_mut().for_each(|states| states.reserve(more));
        let placed = self.keyed.as_deref().unwrap_or(&self.hashes);
        self.slots.reserve(placed.len().saturating_add(more), placed);
    }

    /// How many slots it holds.
    #[cfg(test)]
    pub(in crate::exec) fn slots(&self) -> usize {
        self.slots.len()
    }

    /// \[DESIGN\] (RULES.md D1110) Merges the groups at `places` of
    /// `made` into its own, a column at a time: each looked up by its key
    /// and hash; those new, no two of one key as they are one table's,
    /// appended lane by lane and placed; then each set function's states
    /// merged in one loop.
    fn absorb_places(&mut self, made: &Made, places: &[u32]) -> Result<()> {
        let base = self.hashes.len();
        let (mut groups, mut fresh) = std::mem::take(&mut self.merging);
        groups.clear();
        fresh.clear();
        for &i in places {
            let at = crate::storage::mask::index(i);
            let found = self.find(made.hashes.get(at).copied().unwrap_or(0), Key::Group(&made.keys, at));
            groups.push(found.unwrap_or_else(|| narrow(base.saturating_add(fresh.len()))));
            if found.is_none() {
                fresh.push(i);
            }
        }
        self.make_all(made, &fresh);
        for (&g, &i) in groups.iter().zip(places) {
            let first = made.firsts.get(crate::storage::mask::index(i)).copied().unwrap_or(u64::MAX);
            if let Some(held) = self.firsts.get_mut(crate::storage::mask::index(g)) {
                *held = (*held).min(first);
            }
        }
        for ((states, agg), sent) in self.states.iter_mut().zip(&self.aggs).zip(&made.states) {
            states.merge_all(agg, &groups, (sent, places))?;
        }
        self.merging = (groups, fresh);
        Ok(())
    }

    /// Makes the groups at `fresh` of `made`, none of them its own yet,
    /// fed nothing, in that order: their keys, hashes and first rows
    /// appended, then each placed. A table placed by keyed hashes makes
    /// them one by one (RULES.md D357).
    fn make_all(&mut self, made: &Made, fresh: &[u32]) {
        let of = |i: u32| {
            let at = crate::storage::mask::index(i);
            (at, made.hashes.get(at).copied().unwrap_or(0), made.firsts.get(at).copied().unwrap_or(u64::MAX))
        };
        if self.keyed.is_some() {
            for (at, hash, first) in fresh.iter().map(|&i| of(i)) {
                self.make(hash, Key::Group(&made.keys, at), first);
            }
            return;
        }
        let from = self.hashes.len();
        self.keys.extend(&made.keys, fresh);
        self.hashes.extend(fresh.iter().map(|&i| of(i).1));
        self.firsts.extend(fresh.iter().map(|&i| of(i).2));
        self.states.iter_mut().for_each(|states| states.grow_by(fresh.len()));
        for g in from..self.hashes.len() {
            let hash = self.hashes.get(g).copied().unwrap_or(0);
            if self.slots.place(hash, narrow(g), &self.hashes) > MAX_PROBE {
                // Every group, those not yet placed too, placed by its
                // keyed hash.
                self.rekeyed();
                break;
            }
        }
    }
}

// Unit tests: partitions merged a column at a time hold what merging
// group by group holds (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;

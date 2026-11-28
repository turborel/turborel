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

mod apart;
mod wide;
mod written;
pub(super) use apart::Placed;
pub(in crate::exec) use wide::{Adopted, Lying, Run, Wide};
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
    /// \[DESIGN\] (RULES.md D1260) Whether two of its groups may hold one
    /// key, as rows handed on ungrouped do. Such groups are grouped
    /// before they go to the spill as they are (`waves.rs`).
    repeats: bool,
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

    /// Whether two of its groups may hold one key (RULES.md D1260).
    pub(in crate::exec) fn repeats(&self) -> bool {
        self.repeats
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

/// \[DESIGN\] (RULES.md D1275) Whether the group of `made` at `place`,
/// one given its slot and not yet appended, holds `key`. Kept out of the
/// lookup's loop, which then compares keys in one place alone, those the
/// table holds: a key seldom meets the slot of a group not yet appended,
/// and never of a morsel's groups no two of which share a key.
#[inline(never)]
fn unappended_equal(made: &Made, place: Option<u32>, key: Key<'_>, scratch: &mut Vec<u8>) -> bool {
    place.is_some_and(|j| made.keys.equal(crate::storage::mask::index(j), key, scratch))
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
        // Groups written by these partitions lie as they are handed over
        // already (RULES.md D1313).
        if let Some(starts) = self.starts_placed(parts, lanes) {
            return self.made_placed(starts);
        }
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
            repeats: self.apart,
        })
    }

    /// The table of the groups of `made`, frozen, of the set functions
    /// `aggs`, its keys hashed by `seed`: its slots placed anew by the
    /// hashes it holds, its columns of states as they were, each
    /// accumulator merged from its state written.
    ///
    /// # Errors
    ///
    /// As `Accumulator::merge`.
    pub(in crate::exec) fn thawed(made: Made, aggs: &[AggCall], seed: &Seed) -> Result<GroupTable> {
        let mut table = GroupTable::thawed_unplaced(made, aggs, seed)?;
        table.slots.rebuilt(table.keyed.as_deref().unwrap_or(&table.hashes));
        Ok(table)
    }

    /// As `thawed`, none of the groups given a slot: of a table no key is
    /// looked up in (`made/wide.rs`).
    ///
    /// # Errors
    ///
    /// As `Accumulator::merge`.
    fn thawed_unplaced(made: Made, aggs: &[AggCall], seed: &Seed) -> Result<GroupTable> {
        let len = made.hashes.len();
        let mut table = GroupTable::new(made.keys.width(), aggs, seed, usize::MAX);
        table.states = made.states.into_iter().zip(aggs).map(|(sent, agg)| States::thawed(sent, agg, len)).collect::<Result<_>>()?;
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
    /// merging a wave's morsels grows no vector a group at a time.
    pub(in crate::exec) fn reserve(&mut self, like: &Made, more: usize) {
        self.keys.reserve(&like.keys, more);
        self.hashes.reserve(more);
        self.firsts.reserve(more);
        self.states.iter_mut().for_each(|states| states.reserve(more));
    }

    /// \[DESIGN\] (RULES.md D1110, D1275) Merges the groups at `places` of
    /// `made` into its own, a column at a time and in one pass over them:
    /// each looked up by its key and hash, and one that is new given its
    /// slot at once (`looked_up`); then the new groups' keys, first rows
    /// and states appended lane by lane; then each set function's states
    /// merged in one loop. The groups, their numbers and their slots are
    /// those that finding or making them one at a time gives, of a table's
    /// groups, no two of a key, and of rows handed on ungrouped, which may
    /// share one (`Made::repeats`).
    fn absorb_places(&mut self, made: &Made, places: &[u32]) -> Result<()> {
        let (mut groups, mut fresh) = std::mem::take(&mut self.merging);
        groups.clear();
        fresh.clear();
        let (looked, long) = self.looked_up(made, places, (&mut groups, &mut fresh));
        self.keys.extend(&made.keys, &fresh);
        self.firsts.extend(fresh.iter().map(|&i| made.firsts.get(crate::storage::mask::index(i)).copied().unwrap_or(u64::MAX)));
        self.states.iter_mut().for_each(|states| states.grow_by(fresh.len()));
        if long {
            self.rekeyed();
        }
        // A table placed by keyed hashes finds or makes the rest one at a
        // time, each by its key's keyed hash (RULES.md D357).
        for &i in places.get(looked..).unwrap_or(&[]) {
            let at = crate::storage::mask::index(i);
            let (hash, first) = (made.hashes.get(at).copied().unwrap_or(0), made.firsts.get(at).copied().unwrap_or(u64::MAX));
            groups.push(self.found_or_made(hash, Key::Group(&made.keys, at), first));
        }
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

    /// \[DESIGN\] (RULES.md D1275) Looks up the groups at `places` of
    /// `made`, in their order, each group's number appended to `groups`.
    /// A key that has no group is given the next number, its hash and its
    /// slot at once, and its place in `made` is appended to `fresh`. Its
    /// key, first row and states are not appended here. So a key looked
    /// up after it that meets its slot is compared with the key `made`
    /// holds at that place, and a key `made` holds twice finds the group
    /// its first made.
    ///
    /// Gives how many of the places it looked up, and whether a probe
    /// grew past `MAX_PROBE`. It stops after the place whose probe did,
    /// and looks none up of a table placed by keyed hashes, as those are
    /// found by their keys' bytes (RULES.md D357).
    fn looked_up(&mut self, made: &Made, places: &[u32], (groups, fresh): (&mut Vec<u32>, &mut Vec<u32>)) -> (usize, bool) {
        if self.keyed.is_some() {
            return (0, false);
        }
        let base = self.hashes.len();
        for (n, &i) in places.iter().enumerate() {
            let at = crate::storage::mask::index(i);
            let (hash, key) = (made.hashes.get(at).copied().unwrap_or(0), Key::Group(&made.keys, at));
            let Self { slots, keys, scratch, .. } = self;
            let probe = slots.find(hash, |g| {
                let held = crate::storage::mask::index(g);
                match held.checked_sub(base) {
                    None => keys.equal(held, key, scratch),
                    Some(new) => unappended_equal(made, fresh.get(new).copied(), key, scratch),
                }
            });
            let mut length = probe.length;
            groups.push(probe.found.unwrap_or_else(|| {
                let g = narrow(self.hashes.len());
                self.hashes.push(hash);
                fresh.push(i);
                length = length.max(self.slots.place(hash, g, &self.hashes));
                g
            }));
            if length > MAX_PROBE {
                return (n.saturating_add(1), true);
            }
        }
        (places.len(), false)
    }
}

// Unit tests: partitions merged a column at a time hold what merging
// group by group holds (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;

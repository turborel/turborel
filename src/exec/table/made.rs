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

use super::states::{Sent, States};
use super::{GroupTable, Key, Keys, Seed, narrow};

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
    /// The places of each partition's groups.
    parts: Vec<Vec<u32>>,
    /// The keyed hashes its groups are placed by, if its table took them
    /// (RULES.md D357).
    keyed: Option<Vec<u64>>,
}

impl Made {
    /// The bytes it holds.
    pub(in crate::exec) fn bytes(&self) -> usize {
        let words = self.hashes.capacity().saturating_add(self.firsts.capacity()).saturating_mul(size_of::<u64>());
        let places = self.parts.iter().map(|p| p.capacity().saturating_mul(size_of::<u32>())).sum::<usize>();
        let states = self.states.iter().map(Sent::bytes).sum::<usize>();
        let keyed = self.keyed.as_ref().map_or(0, |k| k.capacity().saturating_mul(size_of::<u64>()));
        words.saturating_add(keyed).saturating_add(places).saturating_add(states).saturating_add(self.keys.bytes())
    }

    /// How many groups of partition `part` it holds.
    pub(in crate::exec) fn groups_of(&self, part: usize) -> usize {
        self.parts.get(part).map_or(0, Vec::len)
    }
}

impl GroupTable {
    /// Its groups, made, split among `parts` partitions by their hashes.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if an accumulator writes no state.
    pub(in crate::exec) fn made(mut self, parts: usize) -> Result<Made> {
        let mut places: Vec<Vec<u32>> = (0..parts).map(|_| Vec::new()).collect();
        for (g, &hash) in self.hashes.iter().enumerate() {
            if let Some(part) = places.get_mut(part_of(hash, parts)) {
                part.push(narrow(g));
            }
        }
        // Held until its partitions merge it, so none of its room is spare.
        places.iter_mut().for_each(Vec::shrink_to_fit);
        // Each partition finds its groups by the hashes they came with.
        self.keyed = None;
        let mut made = self.frozen()?;
        made.parts = places;
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
        Ok(Made { keys: self.keys, hashes: self.hashes, firsts: self.firsts, states, parts: Vec::new(), keyed: self.keyed })
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
        let len = made.hashes.len();
        let mut table = GroupTable::new(made.keys.width(), aggs, seed, usize::MAX);
        table.states = made.states.into_iter().zip(aggs).map(|(sent, agg)| States::thawed(sent, agg, len)).collect::<Result<_>>()?;
        table.slots.rebuilt(made.keyed.as_deref().unwrap_or(&made.hashes));
        (table.keys, table.hashes, table.firsts, table.keyed) = (made.keys, made.hashes, made.firsts, made.keyed);
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
        for &i in made.parts.get(part).map_or(&[][..], Vec::as_slice) {
            self.absorb_one(made, crate::storage::mask::index(i))?;
        }
        Ok(())
    }

    /// Merges every group of `made`, frozen whole, into its own, in their
    /// order, as `absorb` merges a partition's.
    ///
    /// # Errors
    ///
    /// As `absorb`.
    pub(in crate::exec) fn absorb_whole(&mut self, made: &Made) -> Result<()> {
        (0..made.hashes.len()).try_for_each(|i| self.absorb_one(made, i))
    }

    /// Merges group `i` of `made` into its own.
    fn absorb_one(&mut self, made: &Made, i: usize) -> Result<()> {
        let (Some(&hash), Some(&first)) = (made.hashes.get(i), made.firsts.get(i)) else { return Ok(()) };
        let key = Key::Group(&made.keys, i);
        let g = match self.find(hash, key) {
            Some(g) => g,
            None => self.make(hash, key, first),
        };
        let g = crate::storage::mask::index(g);
        if let Some(held) = self.firsts.get_mut(g) {
            *held = (*held).min(first);
        }
        for ((states, agg), sent) in self.states.iter_mut().zip(&self.aggs).zip(&made.states) {
            states.merge((g, agg), (sent, i))?;
        }
        Ok(())
    }
}

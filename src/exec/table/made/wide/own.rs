//! \[DESIGN\] (RULES.md D1300) The tables a wide partition makes of its
//! groups. A group that took a second row is taken up alone into a table
//! of its sub-part's own, which no key is ever looked up in, as the
//! sub-part's slots find a group wherever it lies, so its groups are
//! appended and given no slot. What reads a table has the whole
//! partition gathered into one, a run of a Made's groups at a time.

use super::super::super::slots::{MAX_PROBE, Slots};
use super::super::super::{GroupTable, Key, Seed, narrow};
use super::super::Made;
use crate::error::Result;
use crate::logical::AggCall;
use crate::storage::mask::index;

impl GroupTable {
    /// \[DESIGN\] (RULES.md D1300) Appends the groups at `places` of `made`
    /// as groups of its own, in that order, none of them looked up nor
    /// placed, as what calls it knows their keys are none it holds: their
    /// keys, hashes and first rows appended a column at a time, and their
    /// states merged into states fed nothing, as a merge makes a new
    /// group's.
    ///
    /// # Errors
    ///
    /// As `Accumulator::merge`; a sum past 128 bits.
    pub(super) fn take_up(&mut self, made: &Made, places: &[u32]) -> Result<()> {
        let from = self.hashes.len();
        let word = |words: &[u64], i: u32, missing: u64| words.get(index(i)).copied().unwrap_or(missing);
        self.keys.extend(&made.keys, places);
        self.hashes.extend(places.iter().map(|&i| word(&made.hashes, i, 0)));
        self.firsts.extend(places.iter().map(|&i| word(&made.firsts, i, u64::MAX)));
        self.states.iter_mut().for_each(|states| states.grow_by(places.len()));
        let groups: Vec<u32> = (from..self.hashes.len()).map(narrow).collect();
        for ((states, agg), sent) in self.states.iter_mut().zip(&self.aggs).zip(&made.states) {
            states.merge_all(agg, &groups, (sent, places))?;
        }
        Ok(())
    }

    /// \[DESIGN\] (RULES.md D1300) A table of a wide partition's own, of
    /// keys of `width` columns, each group computing `aggs`: no group
    /// yet, and none ever given a slot, as none is looked up in it. No
    /// two of its groups hold one key, as the partition's slots found
    /// each before it was taken up.
    pub(super) fn of_own(width: usize, aggs: &[AggCall], seed: &Seed) -> GroupTable {
        let mut table = GroupTable::unplaced(width, aggs, seed, 0);
        table.apart = false;
        table
    }

    /// \[DESIGN\] (RULES.md D1300) Appends the group at `place` of `made`
    /// as a group of its own, not looked up and given no slot: its key,
    /// hash and first row, and each set function's state merged into one
    /// fed nothing, as a merge makes a new group's. Gives its number.
    ///
    /// # Errors
    ///
    /// As `Accumulator::merge`; a sum past 128 bits.
    pub(super) fn taken_up(&mut self, made: &Made, place: u32) -> Result<u32> {
        let at = index(place);
        let (hash, first) = (made.hashes.get(at).copied().unwrap_or(0), made.firsts.get(at).copied().unwrap_or(u64::MAX));
        let g = self.appended(hash, Key::Group(&made.keys, at), first);
        for ((states, agg), sent) in self.states.iter_mut().zip(&self.aggs).zip(&made.states) {
            states.merge((index(g), agg), (sent, at))?;
        }
        Ok(g)
    }

    /// \[DESIGN\] (RULES.md D1301) Merges the group at `place` of `made`
    /// into its group `g`: its first row the earlier, each set function's
    /// state after those the group holds.
    ///
    /// # Errors
    ///
    /// As `Accumulator::merge`; a sum past 128 bits.
    pub(super) fn merged_one(&mut self, g: u32, made: &Made, place: u32) -> Result<()> {
        let at = index(place);
        let first = made.firsts.get(at).copied().unwrap_or(u64::MAX);
        if let Some(held) = self.firsts.get_mut(index(g)) {
            *held = (*held).min(first);
        }
        for ((column, agg), sent) in self.states.iter_mut().zip(&self.aggs).zip(&made.states) {
            column.merge((index(g), agg), (sent, at))?;
        }
        Ok(())
    }

    /// Places every group, appended and not yet placed, in slots made for
    /// them. A table placed by keyed hashes places each by its key's
    /// (RULES.md D357), and one whose probe grows long turns so, every
    /// group placed again.
    pub(super) fn placed_all(&mut self) {
        self.slots = Slots::with_room(self.hashes.len());
        for g in 0..self.hashes.len() {
            let hash = self.hashes.get(g).copied().unwrap_or(0);
            if self.slots.place(hash, narrow(g), &self.hashes) > MAX_PROBE {
                self.rekeyed();
                return;
            }
        }
    }
}

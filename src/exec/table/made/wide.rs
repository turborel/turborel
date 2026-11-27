//! \[DESIGN\] (RULES.md D1300) A partition's groups adopted where they
//! lie. A partition of a grouping run in parallel merges its morsels'
//! groups into a table of its own (`made.rs`), each new group's key, first
//! row's number and states copied out of the morsel's groups handed over.
//! Past the groups a cache holds, that copy and the slots it is found by
//! are most of what a group costs to merge. A wide partition copies
//! nothing of a new group. It keeps the Mades it was handed, as they are
//! and shared as they were, and of each group its hash and where the
//! group lies, the Made and the place of its first row. The key and the
//! first row's number are read there.
//!
//! A group that takes a second row cannot merge it where the group lies,
//! as a Made is never written once its morsel's thread has made it, which
//! is what lets any thread read it. The first time a row meets a group
//! held already the group is taken up into a table of the partition's own,
//! key, first row and states, and that row and every later one merge
//! there. A group of one row has no state but the Made's.
//!
//! A wave's Mades come in the morsels' order and a Made's rows of a
//! partition in the morsel's, so the first row of a group to be looked up
//! is its first row, and the groups listed are in the order of their
//! first rows. A group taken up is given the states of its first row and
//! then its later rows' in the order they come, the order a table merges
//! them in, so each set function's state is the one a table holds, of a
//! function whose state depends on that order too.

use std::sync::Arc;

use super::super::slots::{MAX_PROBE, Slots};
use super::super::{GroupTable, Key, Seed, narrow};
use super::Made;
use crate::error::Result;
use crate::exec::broken;
use crate::logical::AggCall;
use crate::storage::mask::index;

/// The segment of a group that lies in the table of the partition's own.
const OWN: u32 = u32::MAX;

/// A Made a partition adopted: the Made, let go once none of the
/// partition's groups lies in it any more; the bytes of it the
/// partition's rows hold; and how many of its groups lie in it.
struct Segment {
    made: Option<Arc<Made>>,
    bytes: usize,
    lying: usize,
}

/// A partition's groups, adopted where they lie.
pub(in crate::exec) struct Wide {
    /// The Mades its groups of one row lie in, in the order handed.
    segments: Vec<Segment>,
    /// Each group's hash, the groups in the order of their first rows.
    hashes: Vec<u64>,
    /// Where each group lies: its segment and its place there, or `OWN`
    /// and its number in `own`.
    lies: Vec<(u32, u32)>,
    /// The groups that took more than one row, whole.
    own: GroupTable,
    slots: Slots,
    /// Each group's hash by SipHash keyed by the seed, once a probe grew
    /// long (RULES.md D357).
    keyed: Option<Vec<u64>>,
    seed: Seed,
    scratch: Vec<u8>,
    /// The rows of the Made being merged that met a group held already,
    /// and each one's group in `own`, kept between Mades for their room.
    held: (Vec<u32>, Vec<u32>),
}

/// A wide partition between waves, as it goes to another thread: its
/// table of its own frozen, and no slots.
pub(in crate::exec) struct Adopted {
    segments: Vec<Segment>,
    hashes: Vec<u64>,
    lies: Vec<(u32, u32)>,
    own: Made,
    keyed: Option<Vec<u64>>,
}

impl Adopted {
    /// The bytes it holds, its share of the Mades it adopted among them.
    pub(in crate::exec) fn bytes(&self) -> usize {
        let segments = self.segments.iter().map(|segment| segment.bytes).sum::<usize>();
        let keyed = self.keyed.as_ref().map_or(0, Vec::capacity);
        let words = self.hashes.capacity().saturating_add(self.lies.capacity()).saturating_add(keyed);
        segments.saturating_add(words.saturating_mul(size_of::<u64>())).saturating_add(self.own.bytes())
    }
}

impl Wide {
    /// The groups of `table`, a partition's that grew past what a cache
    /// holds, as a wide partition's: the table frozen is its first
    /// segment, every group where the table held it, found by the slots
    /// the table found it by.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if an accumulator writes no state.
    pub(in crate::exec) fn of(mut table: GroupTable) -> Result<Wide> {
        let (aggs, seed, width) = (table.aggs.clone(), table.seed.clone(), table.keys.width());
        let slots = std::mem::take(&mut table.slots);
        let mut made = table.frozen()?;
        let keyed = made.keyed.take();
        let (hashes, bytes) = (made.hashes.clone(), made.bytes());
        Ok(Wide {
            lies: (0..hashes.len()).map(|g| (0, narrow(g))).collect(),
            segments: vec![Segment { made: Some(Arc::new(made)), bytes, lying: hashes.len() }],
            hashes,
            own: GroupTable::new(width, &aggs, &seed, usize::MAX),
            slots,
            keyed,
            seed,
            scratch: Vec::new(),
            held: (Vec::new(), Vec::new()),
        })
    }

    /// How many groups.
    #[cfg(test)]
    pub(in crate::exec) fn len(&self) -> usize {
        self.hashes.len()
    }

    /// Whether it places its groups by keyed hashes (RULES.md D357).
    #[cfg(test)]
    pub(in crate::exec) fn is_keyed(&self) -> bool {
        self.keyed.is_some()
    }

    /// The Made of segment `segment`, if any group still lies in it.
    fn made_of(segments: &[Segment], segment: u32) -> Option<&Made> {
        segments.get(index(segment)).and_then(|held| held.made.as_deref())
    }

    /// The hash its slots place `key`, of hash `hash`, by: that, or its
    /// keyed hash once a probe grew long.
    fn placed_by(&mut self, hash: u64, key: Key<'_>) -> u64 {
        if self.keyed.is_none() {
            return hash;
        }
        self.scratch.clear();
        key.put(self.own.keys.width(), &mut self.scratch);
        self.seed.keyed(&self.scratch)
    }

    /// The group of `key`, of hash `hash`, if it lists one: compared with
    /// the key where the group lies.
    fn find(&mut self, hash: u64, key: Key<'_>) -> Option<u32> {
        let placed = self.placed_by(hash, key);
        let Self { slots, lies, segments, own, scratch, .. } = self;
        let probe = slots.find(placed, |g| {
            let Some(&(segment, place)) = lies.get(index(g)) else { return false };
            if segment == OWN {
                return own.keys.equal(index(place), key, scratch);
            }
            Wide::made_of(segments, segment).is_some_and(|made| made.keys.equal(index(place), key, scratch))
        });
        if probe.length > MAX_PROBE {
            self.rekeyed();
        }
        probe.found
    }

    /// Lists the group of `key`, of hash `hash`, that lies at `lies`, and
    /// places it.
    fn list(&mut self, (hash, key): (u64, Key<'_>), lies: (u32, u32)) {
        let g = narrow(self.hashes.len());
        let placed = self.placed_by(hash, key);
        self.hashes.push(hash);
        self.lies.push(lies);
        if let Some(segment) = self.segments.get_mut(index(lies.0)) {
            segment.lying = segment.lying.saturating_add(1);
        }
        let length = match &mut self.keyed {
            Some(keyed) => {
                keyed.push(placed);
                self.slots.place(placed, g, keyed)
            }
            None => self.slots.place(hash, g, &self.hashes),
        };
        if length > MAX_PROBE {
            self.rekeyed();
        }
    }

    /// \[DESIGN\] (RULES.md D357) Places its groups by their keys' SipHash,
    /// keyed by the seed, from now on, if they are not yet, each key read
    /// where its group lies.
    fn rekeyed(&mut self) {
        if self.keyed.is_some() {
            return;
        }
        let width = self.own.keys.width();
        let mut keyed = Vec::with_capacity(self.hashes.len());
        for &(segment, place) in &self.lies {
            self.scratch.clear();
            if segment == OWN {
                Key::Group(&self.own.keys, index(place)).put(width, &mut self.scratch);
            } else if let Some(made) = Wide::made_of(&self.segments, segment) {
                Key::Group(&made.keys, index(place)).put(width, &mut self.scratch);
            }
            keyed.push(self.seed.keyed(&self.scratch));
        }
        self.slots.rebuilt(&keyed);
        self.keyed = Some(keyed);
    }

    /// The number in the table of its own of group `g`, which a row has
    /// met: taken up there from where it lay, if it lay in a Made, which
    /// then holds one group fewer.
    fn owned(&mut self, g: u32) -> Result<u32> {
        let Some(lies) = self.lies.get_mut(index(g)) else { return Err(broken("a group a wide partition does not list")) };
        let (segment, place) = *lies;
        if segment == OWN {
            return Ok(place);
        }
        let number = narrow(self.own.len());
        *lies = (OWN, number);
        let held = self.segments.get_mut(index(segment)).ok_or_else(|| broken("a segment a wide partition does not hold"))?;
        held.lying = held.lying.saturating_sub(1);
        let made = held.made.as_deref().ok_or_else(|| broken("a segment a wide partition let go holds a group"))?;
        self.own.take_up(made, &[place])?;
        Ok(number)
    }

    /// Lets go of each Made from segment `from` on that none of its groups
    /// lies in any more, and of its bytes.
    fn let_go(&mut self, from: usize) {
        for segment in self.segments.iter_mut().skip(from).filter(|segment| segment.lying == 0) {
            (segment.made, segment.bytes) = (None, 0);
        }
    }

    /// Merges the groups at `places` of `made`, a partition's run of a
    /// Made or a Made whole, in their order. A new group is listed where
    /// it lies and nothing of it is copied. A row of a group held already
    /// has the group taken up into the table of its own, and the rows of
    /// this Made that met one are merged there after, a set function at
    /// a time. A Made none of whose groups lies in it any more is let go.
    /// Gives how many of the groups were new.
    ///
    /// # Errors
    ///
    /// As `Accumulator::merge`; a sum past 128 bits.
    pub(in crate::exec) fn absorb(&mut self, made: &Arc<Made>, places: &[u32]) -> Result<usize> {
        let (segment, before) = (narrow(self.segments.len()), self.hashes.len());
        let bytes = made.bytes().saturating_mul(places.len()).checked_div(made.len()).unwrap_or(0);
        self.segments.push(Segment { made: Some(Arc::clone(made)), bytes, lying: 0 });
        let (mut rows, mut groups) = std::mem::take(&mut self.held);
        rows.clear();
        groups.clear();
        for &place in places {
            let at = index(place);
            let (hash, key) = (made.hashes.get(at).copied().unwrap_or(0), Key::Group(&made.keys, at));
            match self.find(hash, key) {
                Some(g) => {
                    groups.push(self.owned(g)?);
                    rows.push(place);
                }
                None => self.list((hash, key), (segment, place)),
            }
        }
        self.own.merge_into(&groups, made, &rows)?;
        self.held = (rows, groups);
        self.let_go(0);
        Ok(self.hashes.len().saturating_sub(before))
    }

    /// As it goes to another thread between waves: its table of its own
    /// frozen, its slots let go.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if an accumulator writes no state.
    pub(in crate::exec) fn frozen(self) -> Result<Adopted> {
        Ok(Adopted { segments: self.segments, hashes: self.hashes, lies: self.lies, own: self.own.frozen()?, keyed: self.keyed })
    }

    /// The wide partition `adopted` was, taken up again: its groups placed
    /// anew by the hashes kept, its table of its own thawed, of the set
    /// functions `aggs`, its keys hashed by `seed`.
    ///
    /// # Errors
    ///
    /// As `Accumulator::merge`.
    pub(in crate::exec) fn thawed(adopted: Adopted, aggs: &[AggCall], seed: &Seed) -> Result<Wide> {
        Ok(Wide {
            slots: Slots::of(adopted.keyed.as_deref().unwrap_or(&adopted.hashes)),
            segments: adopted.segments,
            hashes: adopted.hashes,
            lies: adopted.lies,
            own: GroupTable::thawed(adopted.own, aggs, seed)?,
            keyed: adopted.keyed,
            seed: seed.clone(),
            scratch: Vec::new(),
            held: (Vec::new(), Vec::new()),
        })
    }

    /// Its groups gathered into one table, in the order of their first
    /// rows, for what reads a table: each group of one row taken up from
    /// where it lies, a Made's a set function at a time, and each group of
    /// several rows from the table of its own.
    ///
    /// # Errors
    ///
    /// As `Accumulator::merge`; SQLCODE -910 if an accumulator writes no
    /// state.
    pub(in crate::exec) fn into_table(self) -> Result<GroupTable> {
        let (aggs, width) = (self.own.aggs.clone(), self.own.keys.width());
        let mut table = GroupTable::with_room(width, &aggs, &self.seed, self.hashes.len());
        let own = self.own.frozen()?;
        let (mut run, mut of): (Vec<u32>, Option<u32>) = (Vec::new(), None);
        let taken = |into: &mut GroupTable, from: Option<u32>, places: &[u32]| {
            let Some(from) = from else { return Ok(()) };
            let made = if from == OWN { Some(&own) } else { Wide::made_of(&self.segments, from) };
            into.take_up(made.ok_or_else(|| broken("a segment a wide partition let go holds a group"))?, places)
        };
        for &(segment, place) in &self.lies {
            if of != Some(segment) {
                taken(&mut table, of, &run)?;
                run.clear();
                of = Some(segment);
            }
            run.push(place);
        }
        taken(&mut table, of, &run)?;
        Ok(table)
    }
}

impl GroupTable {
    /// \[DESIGN\] (RULES.md D1300) Appends the groups at `places` of `made`
    /// as groups of its own, in that order, none of them looked up, as
    /// what calls it knows their keys are none it holds: their keys,
    /// hashes and first rows appended a column at a time, each placed,
    /// and their states merged into states fed nothing, as a merge makes a
    /// new group's.
    ///
    /// # Errors
    ///
    /// As `Accumulator::merge`; a sum past 128 bits.
    pub(in crate::exec) fn take_up(&mut self, made: &Made, places: &[u32]) -> Result<()> {
        let from = self.hashes.len();
        let word = |words: &[u64], i: u32, missing: u64| words.get(index(i)).copied().unwrap_or(missing);
        self.keys.extend(&made.keys, places);
        self.hashes.extend(places.iter().map(|&i| word(&made.hashes, i, 0)));
        self.firsts.extend(places.iter().map(|&i| word(&made.firsts, i, u64::MAX)));
        self.states.iter_mut().for_each(|states| states.grow_by(places.len()));
        self.placed_from(from);
        let groups: Vec<u32> = (from..self.hashes.len()).map(narrow).collect();
        for ((states, agg), sent) in self.states.iter_mut().zip(&self.aggs).zip(&made.states) {
            states.merge_all(agg, &groups, (sent, places))?;
        }
        Ok(())
    }

    /// Places its groups from the `from`th on, appended and not yet
    /// placed. A table placed by keyed hashes places each by its key's
    /// (RULES.md D357), and one whose probe grows long turns so, every
    /// group placed again.
    fn placed_from(&mut self, from: usize) {
        for g in from..self.hashes.len() {
            if self.keyed.is_none() {
                let hash = self.hashes.get(g).copied().unwrap_or(0);
                if self.slots.place(hash, narrow(g), &self.hashes) > MAX_PROBE {
                    self.rekeyed();
                    return;
                }
                continue;
            }
            self.scratch.clear();
            Key::Group(&self.keys, g).put(self.keys.width(), &mut self.scratch);
            let placed = self.seed.keyed(&self.scratch);
            if let Some(keyed) = &mut self.keyed {
                keyed.push(placed);
                self.slots.place(placed, narrow(g), keyed);
            }
        }
    }

    /// \[DESIGN\] (RULES.md D1300) Merges the group at each of `places` of
    /// `made` into its group of `groups` beside it, in that order: its
    /// first row the earlier, each set function's states in one loop.
    ///
    /// # Errors
    ///
    /// As `Accumulator::merge`; a sum past 128 bits.
    pub(in crate::exec) fn merge_into(&mut self, groups: &[u32], made: &Made, places: &[u32]) -> Result<()> {
        for (&g, &i) in groups.iter().zip(places) {
            let first = made.firsts.get(index(i)).copied().unwrap_or(u64::MAX);
            if let Some(held) = self.firsts.get_mut(index(g)) {
                *held = (*held).min(first);
            }
        }
        for ((states, agg), sent) in self.states.iter_mut().zip(&self.aggs).zip(&made.states) {
            states.merge_all(agg, groups, (sent, places))?;
        }
        Ok(())
    }
}

// Unit tests: a wide partition holds the groups a table holds, in its
// order (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;

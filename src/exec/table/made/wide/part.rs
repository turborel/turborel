//! \[DESIGN\] (RULES.md D1301) A sub-part of a wide partition. A wide
//! partition's groups are split among sub-parts by bits of their hashes
//! below those the partition was found by, so many that a sub-part's
//! groups and the rows a wave hands it are about what a second-level
//! cache holds slots for. A wave's rows are looked up a sub-part at a
//! time, in slots made for that sub-part alone and let go after it, so a
//! lookup reads slots the cache holds whatever the partition's size, and
//! no slots are kept between waves.
//!
//! A sub-part lists its groups in the order of their first rows, each
//! one's hash and where it lies, and holds a table of its own for those
//! that took a second row (RULES.md D1300). A row is looked up by its
//! hash and compared with the key where a group lies. The rows of a
//! sub-part come in the order of the wave's Mades and a Made's rows, so
//! a group's rows are merged in the order a table merges them.

use super::super::super::slots::{MAX_PROBE, Slots};
use super::super::super::{GroupTable, Key, Seed, narrow};
use super::{OWN, Segment, made_of};
use crate::error::Result;
use crate::exec::broken;
use crate::storage::mask::index;

/// The bits of a hash above those a sub-part is named by: those the
/// first level of a grouping's partitions takes (`made/written.rs`).
const ABOVE: u32 = 6;

/// The bits of a hash, 64.
const WORD: u32 = u64::BITS;

/// The most bits sub-parts are named by: 65,536 of them a partition.
pub(super) const MOST_BITS: u32 = 16;

/// A row a wave hands a wide partition: its hash, the segment of its
/// Made and its place there.
pub(super) type Handed = (u64, u32, u32);

/// The sub-part, among those `bits` bits name, of a group of hash
/// `hash`: the bits below those its partition was found by. Whatever
/// else splits a partition's groups by further bits of their hashes
/// names its parts by this, so that the parts are these sub-parts.
pub(in crate::exec::table::made) fn sub_of(hash: u64, bits: u32) -> usize {
    let below = hash.wrapping_shl(ABOVE);
    usize::try_from(below.checked_shr(WORD.saturating_sub(bits)).unwrap_or(0)).unwrap_or(0)
}

/// The fewest bits that name sub-parts enough for `groups` groups, each
/// sub-part of `sub` at most were they spread evenly.
pub(super) fn bits_for(groups: usize, sub: usize) -> u32 {
    let fits = |bits: &u32| groups <= sub.max(1).checked_shl(*bits).unwrap_or(usize::MAX);
    (0..MOST_BITS).find(fits).unwrap_or(MOST_BITS)
}

/// A sub-part's groups, its table of its own held as `T`: a table while
/// a wave is merged, frozen between waves.
pub(super) struct Part<T> {
    /// Each group's hash, the groups in the order of their first rows.
    pub(super) hashes: Vec<u64>,
    /// Where each group lies: its segment and its place there, or `OWN`
    /// and its number in `own`.
    pub(super) lies: Vec<(u32, u32)>,
    /// Each group's hash by SipHash keyed by the seed, once the partition
    /// places by those (RULES.md D357); none before.
    pub(super) keyed: Vec<u64>,
    /// The groups that took more than one row, whole.
    pub(super) own: T,
}

/// What a sub-part's lookups read and count of its partition: the Mades
/// adopted, how many groups lie in each, whether the partition places by
/// keyed hashes, the seed and the bytes a key is made in.
pub(super) struct Pass<'a> {
    pub(super) segments: &'a [Segment],
    pub(super) lying: &'a mut [usize],
    pub(super) keyed: &'a mut bool,
    pub(super) seed: &'a Seed,
    pub(super) scratch: &'a mut Vec<u8>,
}

impl Pass<'_> {
    /// The hash a sub-part's slots place `key`, of hash `hash`, by: that,
    /// or its keyed hash once the partition places by those.
    fn placed_by(&mut self, hash: u64, key: Key<'_>, width: usize) -> u64 {
        if !*self.keyed {
            return hash;
        }
        self.scratch.clear();
        key.put(width, self.scratch);
        self.seed.keyed(self.scratch)
    }

    /// Counts one group more, or one fewer, lying in segment `segment`.
    fn lies(&mut self, segment: u32, more: bool) {
        if let Some(lying) = self.lying.get_mut(index(segment)) {
            *lying = if more { lying.saturating_add(1) } else { lying.saturating_sub(1) };
        }
    }
}

impl Part<GroupTable> {
    /// Whether group `g` holds `key`, read where the group lies.
    fn holds(&self, g: u32, key: Key<'_>, segments: &[Segment], scratch: &mut Vec<u8>) -> bool {
        let Some(&(segment, place)) = self.lies.get(index(g)) else { return false };
        if segment == OWN {
            return self.own.keys.equal(index(place), key, scratch);
        }
        made_of(segments, segment).is_some_and(|made| made.keys.equal(index(place), key, scratch))
    }

    /// \[DESIGN\] (RULES.md D357) Each of its groups' hashes by SipHash
    /// keyed by the seed, each key read where its group lies.
    fn keyed_hashes(&self, pass: &mut Pass<'_>) -> Vec<u64> {
        let width = self.own.keys.width();
        let mut keyed = Vec::with_capacity(self.hashes.len());
        for &(segment, place) in &self.lies {
            pass.scratch.clear();
            if segment == OWN {
                Key::Group(&self.own.keys, index(place)).put(width, pass.scratch);
            } else if let Some(made) = made_of(pass.segments, segment) {
                Key::Group(&made.keys, index(place)).put(width, pass.scratch);
            }
            keyed.push(pass.seed.keyed(pass.scratch));
        }
        keyed
    }

    /// \[DESIGN\] (RULES.md D357) Places its groups by keyed hashes from
    /// now on, as the whole partition does, in `slots` made anew.
    fn rekeyed(&mut self, slots: &mut Slots, pass: &mut Pass<'_>) {
        *pass.keyed = true;
        self.keyed = self.keyed_hashes(pass);
        slots.rebuilt(&self.keyed);
    }

    /// Slots for its groups and `more` more, so they never grow while
    /// the rows a wave hands it are looked up, each group it lists
    /// placed and none compared, as no two of them hold one key.
    fn placed(&mut self, more: usize, pass: &mut Pass<'_>) -> Slots {
        if *pass.keyed && self.keyed.len() != self.hashes.len() {
            self.keyed = self.keyed_hashes(pass);
        }
        let by = if *pass.keyed { &self.keyed } else { &self.hashes };
        let mut slots = Slots::with_room(by.len().saturating_add(more));
        let mut longest = 0_usize;
        for (g, &hash) in (0_u32..).zip(by) {
            longest = longest.max(slots.place(hash, g, by));
        }
        if longest > MAX_PROBE && !*pass.keyed {
            self.rekeyed(&mut slots, pass);
        }
        slots
    }

    /// Lists the group of `key`, of hash `hash`, that lies at `lies`,
    /// and places it in `slots`.
    fn list(&mut self, slots: &mut Slots, (hash, key): (u64, Key<'_>), lies: (u32, u32), pass: &mut Pass<'_>) {
        let g = narrow(self.hashes.len());
        let placed = pass.placed_by(hash, key, self.own.keys.width());
        self.hashes.push(hash);
        self.lies.push(lies);
        pass.lies(lies.0, true);
        let length = if *pass.keyed {
            self.keyed.push(placed);
            slots.place(placed, g, &self.keyed)
        } else {
            slots.place(hash, g, &self.hashes)
        };
        if length > MAX_PROBE && !*pass.keyed {
            self.rekeyed(slots, pass);
        }
    }

    /// The number in the table of its own of group `g`, which a row has
    /// met: taken up there from where it lay, if it lay in a Made, which
    /// then holds one group fewer.
    fn owned(&mut self, g: u32, pass: &mut Pass<'_>) -> Result<u32> {
        let Some(lies) = self.lies.get_mut(index(g)) else { return Err(broken("a group a wide partition does not list")) };
        let (segment, place) = *lies;
        if segment == OWN {
            return Ok(place);
        }
        let number = narrow(self.own.len());
        *lies = (OWN, number);
        pass.lies(segment, false);
        let made = made_of(pass.segments, segment).ok_or_else(|| broken("a segment a wide partition let go holds a group"))?;
        self.own.taken_up(made, place)
    }

    /// \[DESIGN\] (RULES.md D1301) Looks up `rows`, those a wave hands
    /// this sub-part, in the order they come, in slots made for its
    /// groups and theirs. A row of a new key is listed where it lies and
    /// nothing of it is copied. A row of a key held already has its group
    /// taken up into the table of its own, if it is not there yet, and is
    /// merged into it.
    ///
    /// # Errors
    ///
    /// As `Accumulator::merge`; a sum past 128 bits.
    pub(super) fn looked_up(&mut self, rows: &[Handed], pass: &mut Pass<'_>) -> Result<()> {
        let mut slots = self.placed(rows.len(), pass);
        let (width, segments) = (self.own.keys.width(), pass.segments);
        for &(hash, segment, place) in rows {
            let made = made_of(segments, segment).ok_or_else(|| broken("a wave's Made a wide partition does not hold"))?;
            let key = Key::Group(&made.keys, index(place));
            let placed = pass.placed_by(hash, key, width);
            let probe = slots.find(placed, |g| self.holds(g, key, segments, pass.scratch));
            if probe.length > MAX_PROBE && !*pass.keyed {
                self.rekeyed(&mut slots, pass);
            }
            match probe.found {
                Some(g) => {
                    let number = self.owned(g, pass)?;
                    self.own.merged_one(number, made, place)?;
                }
                None => self.list(&mut slots, (hash, key), (segment, place), pass),
            }
        }
        Ok(())
    }

    /// \[DESIGN\] (RULES.md D1301) Itself split among the sub-parts that
    /// `more` bits more name, `bits` in all: each group to the sub-part
    /// its hash names, in the order it was listed, and each group of the
    /// table of its own taken up into that sub-part's.
    ///
    /// # Errors
    ///
    /// As `Accumulator::merge`; SQLCODE -910 if an accumulator writes no
    /// state.
    pub(super) fn split(self, (bits, more): (u32, u32), seed: &Seed) -> Result<Vec<Part<GroupTable>>> {
        let (aggs, width) = (self.own.aggs.clone(), self.own.keys.width());
        let count = 1_usize.checked_shl(more).unwrap_or(1);
        let child = |hash: u64| sub_of(hash, bits) & count.saturating_sub(1);
        let mut parts: Vec<Part<GroupTable>> = (0..count).map(|_| Part::new(GroupTable::of_own(width, &aggs, seed))).collect();
        // The table of its own first, so each group's new number is known.
        let mut places: Vec<Vec<u32>> = vec![Vec::new(); count];
        let mut numbers = Vec::with_capacity(self.own.len());
        for (g, &hash) in (0_u32..).zip(&self.own.hashes) {
            let Some(taken) = places.get_mut(child(hash)) else { continue };
            numbers.push(narrow(taken.len()));
            taken.push(g);
        }
        let own = self.own.frozen()?;
        for (part, taken) in parts.iter_mut().zip(&places) {
            part.own.take_up(&own, taken)?;
        }
        for (g, (&hash, &(segment, place))) in self.hashes.iter().zip(&self.lies).enumerate() {
            let Some(part) = parts.get_mut(child(hash)) else { continue };
            let number = if segment == OWN { numbers.get(index(place)).copied().unwrap_or(place) } else { place };
            part.hashes.push(hash);
            part.lies.push((segment, number));
            part.keyed.extend(self.keyed.get(g));
        }
        Ok(parts)
    }
}

impl<T> Part<T> {
    /// A sub-part of no group, its table of its own `own`.
    pub(super) fn new(own: T) -> Part<T> {
        Part { hashes: Vec::new(), lies: Vec::new(), keyed: Vec::new(), own }
    }

    /// The bytes its lists hold, its table of its own apart.
    pub(super) fn listed_bytes(&self) -> usize {
        let words = self.hashes.capacity().saturating_add(self.lies.capacity()).saturating_add(self.keyed.capacity());
        words.saturating_mul(size_of::<u64>())
    }

    /// Itself, its table of its own made `own` of what it was.
    pub(super) fn with<U>(self, own: impl FnOnce(T) -> Result<U>) -> Result<Part<U>> {
        Ok(Part { hashes: self.hashes, lies: self.lies, keyed: self.keyed, own: own(self.own)? })
    }
}

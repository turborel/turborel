//! \[DESIGN\] (RULES.md D356) Where a table's groups are found by their
//! hashes: open addressing with linear probing, as Richter, Alvarez and
//! Dittrich, "A Seven-Dimensional Analysis of Hashing Methods" (2015),
//! find the most robust for keys a database groups by. Each slot is a word,
//! the high half of its group's hash above the group's number plus one, 0
//! of an empty slot, so a probe reads one word a slot and compares a key
//! only of a slot whose half matches: a portable form of the tags a Swiss
//! table keeps a byte each and matches sixteen at once. The slots are a
//! power of two, at most half of them full, so a probe ends within a few
//! slots; on growth they double and each group is placed again by the hash
//! its table keeps, no key hashed again. Each probe's length is told, so a
//! table whose probe grows past `MAX_PROBE` hashes its keys anew (RULES.md
//! D357).
//!
//! \[DESIGN\] (RULES.md D1261) A table told how many groups it may yet
//! take makes its slots for them at once (`rebuilt_for`, `reserve`), so
//! its groups are placed one time, where doubling as they come would
//! place each again at every doubling.

/// \[DESIGN\] (RULES.md D357) The longest probe a table takes before it
/// hashes its keys anew by SipHash: 128 slots, eight times sixteen, far
/// past what half-full slots of keys spread by their hash ever reach, but
/// short of what keys made to collide would cost each.
pub(super) const MAX_PROBE: usize = 128;

/// The fewest slots: a table of few groups costs a cache line or two.
const LEAST_SLOTS: usize = 16;

/// The bits of a slot's group number plus one, below its hash's half.
const GROUP_BITS: u32 = 32;

/// A slot holding no group.
const EMPTY: u64 = 0;

/// The slots of a table's groups.
#[derive(Debug, Default)]
pub(super) struct Slots {
    slots: Vec<u64>,
    /// One less than the slots, a power of two; 0 of none.
    mask: u64,
}

/// What a probe found: the group whose slot's half matched and whose key
/// was equal, if any, and the slots it read.
pub(super) struct Probe {
    pub(super) found: Option<u32>,
    pub(super) length: usize,
}

/// The slots `groups` groups take: twice as many and one more, a power
/// of two, so at most half of them are full.
fn wanted(groups: usize) -> usize {
    groups.saturating_add(1).saturating_mul(2).max(LEAST_SLOTS).next_power_of_two()
}

/// The high half of `hash`, as a slot keeps it.
fn half(hash: u64) -> u64 {
    hash >> GROUP_BITS
}

impl Slots {
    /// Slots for the groups placed by `hashes`, each group's.
    pub(super) fn of(hashes: &[u64]) -> Slots {
        let mut slots = Slots::default();
        slots.rebuilt(hashes);
        slots
    }

    /// Empty slots for `groups` groups, none of them placed again as
    /// they come.
    pub(super) fn with_room(groups: usize) -> Slots {
        let wanted = wanted(groups);
        Slots { slots: vec![EMPTY; wanted], mask: u64::try_from(wanted).unwrap_or(u64::MAX).saturating_sub(1) }
    }

    /// How many slots.
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.slots.len()
    }

    /// The bytes it holds.
    pub(super) fn bytes(&self) -> usize {
        self.slots.capacity().saturating_mul(size_of::<u64>())
    }

    /// The place of the first slot of `hash`'s probe.
    fn start(&self, hash: u64) -> u64 {
        hash & self.mask
    }

    /// Finds the group of `hash` whose key `equal` says is equal, reading
    /// the slots from the hash's on till one is empty.
    pub(super) fn find(&self, hash: u64, mut equal: impl FnMut(u32) -> bool) -> Probe {
        let (mut at, mut length) = (self.start(hash), 0_usize);
        while let Some(&slot) = usize::try_from(at).ok().and_then(|at| self.slots.get(at)) {
            length = length.saturating_add(1);
            if slot == EMPTY {
                break;
            }
            let group = u32::try_from(slot & u64::from(u32::MAX)).unwrap_or(0).wrapping_sub(1);
            if half(slot) == half(hash) && equal(group) {
                return Probe { found: Some(group), length };
            }
            at = at.wrapping_add(1) & self.mask;
        }
        Probe { found: None, length }
    }

    /// Places group `group` of `hash`, the slots grown first if more than
    /// half of them would be full, each group placed again by `hashes`:
    /// the probe's length.
    pub(super) fn place(&mut self, hash: u64, group: u32, hashes: &[u64]) -> usize {
        let groups = usize::try_from(group).unwrap_or(usize::MAX).saturating_add(1);
        if groups.saturating_mul(2) > self.slots.len() {
            self.rebuilt(hashes.get(..groups.saturating_sub(1)).unwrap_or(hashes));
        }
        self.put(hash, group)
    }

    /// Puts group `group` of `hash` in the first empty slot of its probe:
    /// the probe's length.
    fn put(&mut self, hash: u64, group: u32) -> usize {
        let (mut at, mut length) = (self.start(hash), 0_usize);
        let word = (half(hash) << GROUP_BITS) | u64::from(group.wrapping_add(1));
        while let Some(slot) = usize::try_from(at).ok().and_then(|at| self.slots.get_mut(at)) {
            length = length.saturating_add(1);
            if *slot == EMPTY {
                *slot = word;
                return length;
            }
            at = at.wrapping_add(1) & self.mask;
        }
        length
    }

    /// Makes the slots anew for the groups of `hashes`, room for twice as
    /// many and one more, each placed in turn.
    pub(super) fn rebuilt(&mut self, hashes: &[u64]) {
        self.rebuilt_for(hashes, 0);
    }

    /// \[DESIGN\] (RULES.md D1261) Room for `groups` groups in all, those
    /// of `hashes` placed again if the slots must grow for it, and not
    /// touched if they hold that many already.
    pub(super) fn reserve(&mut self, groups: usize, hashes: &[u64]) {
        if wanted(groups) > self.slots.len() {
            self.rebuilt_for(hashes, groups.saturating_sub(hashes.len()));
        }
    }

    /// \[DESIGN\] (RULES.md D1261) As `rebuilt`, with room for `more`
    /// groups more than those of `hashes`, so none is placed again as
    /// that many come.
    pub(super) fn rebuilt_for(&mut self, hashes: &[u64], more: usize) {
        let wanted = wanted(hashes.len().saturating_add(more));
        self.slots.clear();
        self.slots.resize(wanted, EMPTY);
        self.slots.shrink_to_fit();
        self.mask = u64::try_from(wanted).unwrap_or(u64::MAX).saturating_sub(1);
        for (group, &hash) in (0_u32..).zip(hashes) {
            self.put(hash, group);
        }
    }
}

// Unit tests: growth past half full, probes wrapping past the last slot,
// groups placed again by their hashes (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] (RULES.md D356) slots grow past half full and
    /// every group is found by its hash after, placed again by the hashes
    /// kept, none hashed again.
    #[test]
    fn slots_grow_and_keep_every_group() {
        let hashes: Vec<u64> = (0_u64..1000).map(|n| n.wrapping_mul(0x9E37_79B9_7F4A_7C15)).collect();
        let mut slots = Slots::default();
        for (group, &hash) in (0_u32..).zip(&hashes) {
            slots.place(hash, group, &hashes);
        }
        assert!(slots.slots.len() >= 2000);
        for (group, &hash) in (0_u32..).zip(&hashes) {
            assert_eq!(slots.find(hash, |g| g == group).found, Some(group));
        }
        assert_eq!(slots.find(7, |_| true).found, None);
    }

    /// Verifies: [DESIGN] (RULES.md D1261) slots made with room for groups
    /// yet to come take them with no growth, every group found after, by
    /// the room asked when they are made anew and by a reserve after; a
    /// reserve of no more than they hold leaves them as they are, and
    /// slots made with no room grow as the groups come.
    #[test]
    fn slots_made_for_groups_to_come_do_not_grow() {
        let hashes: Vec<u64> = (0_u64..3000).map(|n| n.wrapping_mul(0x9E37_79B9_7F4A_7C15)).collect();
        let (held, coming) = hashes.split_at(1000);
        let filled = |slots: &mut Slots| {
            for (group, &hash) in (1000_u32..).zip(coming) {
                slots.place(hash, group, &hashes);
            }
            (0_u32..).zip(&hashes).all(|(group, &hash)| slots.find(hash, |g| g == group).found == Some(group))
        };
        let mut roomy = Slots::default();
        roomy.rebuilt_for(held, coming.len());
        let made = roomy.slots.len();
        assert_eq!(made, 8192, "twice 3001, a power of two");
        assert!(filled(&mut roomy) && roomy.slots.len() == made, "no growth");
        let mut reserved = Slots::of(held);
        assert_eq!(reserved.slots.len(), 2048);
        reserved.reserve(1000, held);
        assert_eq!(reserved.slots.len(), 2048, "room it has is not made again");
        reserved.reserve(3000, held);
        assert_eq!(reserved.slots.len(), made);
        assert!(filled(&mut reserved) && reserved.slots.len() == made);
        let mut tight = Slots::of(held);
        assert!(filled(&mut tight) && tight.slots.len() == made, "grown by doubling to the same size");
    }

    /// Verifies: [DESIGN] (RULES.md D356) a probe that starts at the last
    /// slot wraps to the first, and one whose half matches but whose key
    /// differs goes on.
    #[test]
    fn probes_wrap_and_compare_keys() {
        let last = 15_u64 | (5_u64 << 32_u32);
        let hashes = [last, last, last];
        let mut slots = Slots::default();
        for (group, &hash) in (0_u32..).zip(&hashes) {
            slots.place(hash, group, &hashes);
        }
        let probe = slots.find(last, |g| g == 2);
        assert_eq!(probe.found, Some(2));
        assert_eq!(probe.length, 3);
        assert_eq!(slots.find(last, |_| false).length, 4);
    }
}

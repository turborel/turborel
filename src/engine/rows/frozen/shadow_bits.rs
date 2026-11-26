//! \[DESIGN\] A group's shadow bits (RULES.md D510): of each row id in its
//! range, or each block of ids if the range is wider than `MOST_BITS`,
//! whether the tree may have an entry for it. Kept in memory alone, a
//! superset of the entries there are. A scan reads the tree over the
//! group's range only at the ids whose bits are set, and not at all if
//! none is, so a group no write touched costs no tree read
//! (`batches.rs`).
//!
//! A writer sets the bit of a row it writes after its entry is written,
//! as it finds the group in the directory (`RowTable::mark_shadow`); a
//! group published marks the entries the tree holds in its range after it
//! is in the directory, but those of the rows it froze, taken out later
//! (`freeze.rs`). Either the writer finds the group, or the group's mark
//! finds the writer's entry. Bits are never cleared: a rewrite makes a new
//! group, whose bits are marked as it is published. A table opened marks
//! its groups' bits from one scan of the tree over their ranges.
//!
//! A group is in the directory before its mark has read the tree, so a
//! reader may take it with bits yet to be set. Such a bit is that of a row
//! written just before the group was published, or of a row of its range
//! it did not freeze, whose commit was not yet stamped on its entry as the
//! freeze read it. Its bits are settled only once the mark is done
//! (RULES.md D1100). Until then a reader takes every bit as set, and reads
//! the tree over the group's range whole, as it did before there were
//! bits. It reads that the bits are settled before it reads any of them,
//! so the bits it trusts hold every mark made by then.

use std::ops::Bound;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::Entry;
use crate::engine::rows::{RowTable, key, row_id};
use crate::engine::tree::Direction;
use crate::error::Result;
use crate::storage::RowId;

/// \[DESIGN\] The most bits a group keeps: one a row of a group of
/// `DEFAULT_GROUP_ROWS`, 8 KiB, allocated only once one is set.
const MOST_BITS: u64 = 1 << 16;
/// \[DESIGN\] The most ranges of set bits a scan reads the tree over one
/// by one; past it, over the group's range whole.
pub(super) const MOST_RANGES: usize = 64;
/// Bits of a word.
const WORD: u64 = 64;

/// \[DESIGN\] A group's shadow bits.
pub(in crate::engine::rows) struct ShadowBits {
    first: RowId,
    last: RowId,
    /// The ids a bit stands for, as a power of two.
    shift: u32,
    /// The bits, none until one is set.
    words: OnceLock<Box<[AtomicU64]>>,
    /// Whether the entries the tree held as the group was put in the
    /// directory are marked. Until they are, every bit is taken as set.
    settled: AtomicBool,
}

impl ShadowBits {
    /// The bits of a group of row ids `first` to `last`, none set, and
    /// not settled.
    pub(super) fn new(first: RowId, last: RowId) -> ShadowBits {
        let span = last.saturating_sub(first).saturating_add(1);
        let mut shift = 0;
        while span.checked_shr(shift).unwrap_or(0) > MOST_BITS {
            shift = shift.saturating_add(1);
        }
        ShadowBits { first, last, shift, words: OnceLock::new(), settled: AtomicBool::new(false) }
    }

    /// The bit of row `id`, its word and its place in it.
    fn place(&self, id: RowId) -> Option<(usize, u64)> {
        if id < self.first || id > self.last {
            return None;
        }
        let bit = id.saturating_sub(self.first).checked_shr(self.shift).unwrap_or(0);
        Some((usize::try_from(bit.checked_div(WORD)?).ok()?, 1_u64 << bit.checked_rem(WORD)?))
    }

    /// Sets the bit of row `id`, if the group's range holds it.
    pub(in crate::engine::rows) fn mark(&self, id: RowId) {
        let Some((word, bit)) = self.place(id) else { return };
        let span = self.last.saturating_sub(self.first).checked_shr(self.shift).unwrap_or(0);
        let words = usize::try_from(span.checked_div(WORD).unwrap_or(0)).unwrap_or(0).saturating_add(1);
        let all = self.words.get_or_init(|| (0..words).map(|_| AtomicU64::new(0)).collect());
        if let Some(w) = all.get(word) {
            w.fetch_or(bit, Ordering::SeqCst);
        }
    }

    /// Notes that the entries the tree held as the group was put in the
    /// directory are marked, so its bits are trusted from now on.
    pub(super) fn settle(&self) {
        self.settled.store(true, Ordering::SeqCst);
    }

    /// Whether its bits are trusted. Read before any bit is, so the bits
    /// read after hold every mark made before it was settled.
    fn settled(&self) -> bool {
        self.settled.load(Ordering::SeqCst)
    }

    /// Whether the tree may have an entry for row `id`, as it may for any
    /// row of its range until its bits are settled.
    pub(in crate::engine::rows) fn may_hold(&self, id: RowId) -> bool {
        let Some((word, bit)) = self.place(id) else { return false };
        if !self.settled() {
            return true;
        }
        self.words.get().is_some_and(|words| words.get(word).is_some_and(|w| w.load(Ordering::SeqCst) & bit != 0))
    }

    /// Whether no bit is set, and its bits are settled.
    pub(in crate::engine::rows) fn none(&self) -> bool {
        self.settled() && self.words.get().is_none_or(|words| words.iter().all(|w| w.load(Ordering::SeqCst) == 0))
    }

    /// The ranges of row ids, first and last, of the set bits, in order;
    /// the group's range whole if they are more than `MOST_RANGES`, or if
    /// its bits are not settled.
    pub(in crate::engine::rows) fn ranges(&self) -> Vec<(RowId, RowId)> {
        if !self.settled() {
            return vec![(self.first, self.last)];
        }
        let Some(words) = self.words.get() else { return Vec::new() };
        let mut ranges: Vec<(RowId, RowId)> = Vec::new();
        for (n, word) in words.iter().enumerate() {
            let mut bits = word.load(Ordering::SeqCst);
            while bits != 0 {
                let bit = u64::try_from(n).unwrap_or(u64::MAX).saturating_mul(WORD).saturating_add(u64::from(bits.trailing_zeros()));
                bits &= bits.wrapping_sub(1);
                let from = self.first.saturating_add(bit.checked_shl(self.shift).unwrap_or(u64::MAX));
                let to = from.saturating_add((1_u64.checked_shl(self.shift).unwrap_or(u64::MAX)).saturating_sub(1)).min(self.last);
                if let Some(last) = ranges.last_mut().filter(|l| l.1.saturating_add(1) == from) {
                    last.1 = to;
                } else {
                    ranges.push((from, to));
                }
            }
            if ranges.len() > MOST_RANGES {
                return vec![(self.first, self.last)];
            }
        }
        ranges
    }
}

impl RowTable {
    /// Sets the shadow bit of row `id`, its entry in the tree just
    /// written, if a group's range holds it; a row past every group's
    /// costs a load alone.
    pub(in crate::engine::rows) fn mark_shadow(&self, id: RowId) {
        if id >= self.frozen.groups_end.load(Ordering::SeqCst) {
            return;
        }
        if let Ok(directory) = self.frozen.directory()
            && let Some(entry) = directory.holding(id)
        {
            entry.shadow.mark(id);
        }
    }

    /// Whether the tree may have an entry for row `id` of `entry`'s group.
    pub(super) fn may_shadow(&self, entry: &Entry, id: RowId) -> bool {
        !self.frozen.trust_bits.load(Ordering::Acquire) || entry.shadow.may_hold(id)
    }

    /// The ranges of row ids, first and last, of `entry`'s group over which
    /// the tree may have entries, in order.
    pub(in crate::engine::rows) fn shadow_ranges(&self, entry: &Entry) -> Vec<(RowId, RowId)> {
        if self.frozen.trust_bits.load(Ordering::Acquire) { entry.shadow.ranges() } else { vec![(entry.summary.first, entry.summary.last)] }
    }

    /// Whether scans trust the shadow bits: off, they read the tree over
    /// every group's range whole, as a test compares.
    #[cfg(test)]
    pub(in crate::engine::rows) fn trust_shadow_bits(&self, on: bool) {
        self.frozen.trust_bits.store(on, Ordering::Release);
    }

    /// Marks the bits of `entry`, a group just put in the directory, of
    /// every entry the tree holds in its range but those of `frozen`, the
    /// rows it froze as they were, in order, to be taken out; its bits are
    /// then settled.
    pub(super) fn mark_published(&self, entry: &Entry, frozen: &[(RowId, Vec<u8>)]) -> Result<()> {
        let (low, high) = (key(entry.summary.first), key(entry.summary.last));
        for found in self.tree.scan(Bound::Included(&low[..]), Bound::Included(&high[..]), Direction::Forward) {
            let (k, bytes) = found?;
            let id = row_id(&k)?;
            let at = frozen.binary_search_by_key(&id, |(i, _)| *i).ok();
            if at.and_then(|at| frozen.get(at)).is_none_or(|(_, b)| *b != bytes) {
                entry.shadow.mark(id);
            }
        }
        entry.shadow.settle();
        Ok(())
    }

    /// Marks the bits of every group of the directory just loaded, of each
    /// entry the tree holds in a group's range, in one scan over them;
    /// their bits are then settled.
    pub(super) fn mark_opened(&self) -> Result<()> {
        let directory = self.frozen.directory()?;
        let (Some(first), Some(last)) = (directory.groups.first(), directory.groups.last()) else { return Ok(()) };
        let (low, high) = (key(first.summary.first), key(last.summary.last));
        for found in self.tree.scan(Bound::Included(&low[..]), Bound::Included(&high[..]), Direction::Forward) {
            let id = row_id(&found?.0)?;
            if let Some(entry) = directory.holding(id) {
                entry.shadow.mark(id);
            }
        }
        directory.groups.iter().for_each(|entry| entry.shadow.settle());
        Ok(())
    }
}

// Unit tests: bits set, read and joined into ranges, over narrow and
// wide ranges of ids (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D510, shadow bits: none set at first;
    /// a mark in the range sets its row's bit alone, one outside it none;
    /// set bits next to each other join into ranges.
    #[test]
    fn marks_and_ranges() {
        let bits = ShadowBits::new(100, 199);
        bits.settle();
        assert!(bits.none(), "none set");
        assert_eq!(bits.ranges(), Vec::new());
        for id in [105, 106, 107, 150, 199, 99, 200] {
            bits.mark(id);
        }
        assert!(!bits.none());
        assert!(bits.may_hold(106) && bits.may_hold(199) && !bits.may_hold(108) && !bits.may_hold(99));
        assert_eq!(bits.ranges(), vec![(105, 107), (150, 150), (199, 199)]);
    }

    /// Verifies: [DESIGN] RULES.md D510, shadow bits: a range wider than
    /// the most bits gives a bit to a block of ids, its range whole.
    #[test]
    fn wide_ranges_take_blocks() {
        let bits = ShadowBits::new(0, (1 << 20) - 1);
        bits.settle();
        bits.mark(40);
        assert!(bits.may_hold(32) && bits.may_hold(47) && !bits.may_hold(48), "a bit of 16 ids");
        assert_eq!(bits.ranges(), vec![(32, 47)]);
    }

    /// Verifies: [DESIGN] RULES.md D510, shadow bits: past the most
    /// ranges, the group's range is read whole.
    #[test]
    fn many_ranges_read_whole() {
        let bits = ShadowBits::new(0, 9999);
        bits.settle();
        for id in (0..200).map(|n| n * 10) {
            bits.mark(id);
        }
        assert_eq!(bits.ranges(), vec![(0, 9999)]);
    }

    /// Verifies: [DESIGN] RULES.md D1100, shadow bits not settled. Until
    /// the entries the tree held as the group was published are marked,
    /// every row of its range is taken as one the tree may hold, its range
    /// is read whole, and its bits are not said to be none. Once settled,
    /// only the rows marked are, whether marked before it or after.
    #[test]
    fn bits_not_settled_are_all_taken_as_set() {
        let bits = ShadowBits::new(100, 199);
        bits.mark(120);
        assert!(bits.may_hold(100) && bits.may_hold(199) && !bits.may_hold(99) && !bits.may_hold(200), "its range, no more");
        assert!(!bits.none(), "not known to be none");
        assert_eq!(bits.ranges(), vec![(100, 199)]);
        bits.settle();
        bits.mark(150);
        assert!(bits.may_hold(120) && bits.may_hold(150) && !bits.may_hold(100));
        assert_eq!(bits.ranges(), vec![(120, 120), (150, 150)]);
        let untouched = ShadowBits::new(0, 9);
        assert!(!untouched.none() && untouched.may_hold(3), "before it is settled");
        untouched.settle();
        assert!(untouched.none() && !untouched.may_hold(3) && untouched.ranges().is_empty(), "once it is");
    }
}

//! \[DESIGN\] What the pager knows of each page's overflow (RULES.md D30),
//! from the reads and writes it made since it was opened: so a write, or a
//! free, need not read a page's old version to find the overflow pages it
//! frees. A page it knows nothing of is read first, as a torn one must be:
//! a failed write forgets its page.
//!
//! A page known to have no overflow, as most are, takes a bit; one known to
//! have some, its extents. The pages are spread over shards by number, so
//! threads writing different pages seldom wait for each other.

use std::collections::HashMap;
use std::sync::Mutex;

use super::Extent;
use crate::sync::lock_builtin;

/// \[DESIGN\] Shards of the knowledge: more than the threads that write at
/// once, so they seldom share one.
const SHARDS: u64 = 64;
/// Bits of a word of a shard's bitmap.
const WORD_BITS: u64 = 64;

/// The pages of one shard, numbered within it.
#[derive(Default)]
struct Shard {
    /// A bit for each page known to have no overflow.
    plain: Vec<u64>,
    /// The overflow and directory pages of each page known to have some.
    spilled: HashMap<u64, Vec<Extent>>,
}

/// The shard of page `id`, its place in it, and its bit's word and mask.
fn place(id: u64) -> (usize, u64, usize, u64) {
    let shard = usize::try_from(id.checked_rem(SHARDS).unwrap_or_default()).unwrap_or_default();
    let slot = id.checked_div(SHARDS).unwrap_or_default();
    let word = usize::try_from(slot.checked_div(WORD_BITS).unwrap_or_default()).unwrap_or(usize::MAX);
    (shard, slot, word, 1_u64 << slot.checked_rem(WORD_BITS).unwrap_or_default())
}

/// What the pager knows of its pages' overflow.
pub(super) struct Known {
    shards: Vec<Mutex<Shard>>,
}

impl Known {
    /// Knowing nothing.
    pub(super) fn new() -> Known {
        Known { shards: (0..SHARDS).map(|_| Mutex::default()).collect() }
    }

    /// The overflow and directory pages page `id` is known to have, none
    /// if it is known to have none; `None` if nothing is known of it.
    pub(super) fn of(&self, id: u64) -> Option<Vec<Extent>> {
        let (shard, slot, word, mask) = place(id);
        let shard = lock_builtin(self.shards.get(shard)?);
        if let Some(extents) = shard.spilled.get(&slot) {
            return Some(extents.clone());
        }
        shard.plain.get(word).is_some_and(|w| w & mask != 0).then(Vec::new)
    }

    /// Page `id` has `overflow`: its overflow and directory pages.
    pub(super) fn set(&self, id: u64, overflow: &[Extent]) {
        let (shard, slot, word, mask) = place(id);
        let Some(shard) = self.shards.get(shard) else { return };
        let mut shard = lock_builtin(shard);
        if overflow.is_empty() {
            shard.spilled.remove(&slot);
            if shard.plain.len() <= word {
                shard.plain.resize(word.saturating_add(1), 0);
            }
            if let Some(w) = shard.plain.get_mut(word) {
                *w |= mask;
            }
        } else {
            clear(&mut shard, word, mask);
            shard.spilled.insert(slot, overflow.to_vec());
        }
    }

    /// Nothing is known of page `id` any more.
    pub(super) fn forget(&self, id: u64) {
        let (shard, slot, word, mask) = place(id);
        let Some(shard) = self.shards.get(shard) else { return };
        let mut shard = lock_builtin(shard);
        shard.spilled.remove(&slot);
        clear(&mut shard, word, mask);
    }
}

/// Clears the bit `mask` of word `word` of `shard`.
fn clear(shard: &mut Shard, word: usize, mask: u64) {
    if let Some(w) = shard.plain.get_mut(word) {
        *w &= !mask;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disk::pager::PageId;

    /// Verifies: [DESIGN] RULES.md D30: a page is known as it was last
    /// set, with overflow or without, until forgotten; others are unknown.
    #[test]
    fn pages_are_known_as_set() {
        let known = Known::new();
        let extents = vec![Extent { start: PageId(9), count: 2 }];
        assert_eq!(known.of(5), None);
        known.set(5, &[]);
        known.set(5 + SHARDS * 200, &extents);
        assert_eq!(known.of(5), Some(Vec::new()));
        assert_eq!(known.of(5 + SHARDS * 200), Some(extents.clone()));
        assert_eq!(known.of(5 + SHARDS), None, "the same shard, another page");
        known.set(5, &extents);
        assert_eq!(known.of(5), Some(extents));
        known.set(5, &[]);
        assert_eq!(known.of(5), Some(Vec::new()));
        known.forget(5);
        known.forget(5 + SHARDS * 200);
        known.forget(u64::MAX);
        assert_eq!((known.of(5), known.of(5 + SHARDS * 200)), (None, None));
    }
}

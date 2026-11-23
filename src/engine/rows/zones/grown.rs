//! \[DESIGN\] Of the zones (see the module's documentation), what holds
//! them (RULES.md D860): a sequence that only grows, read with no lock.
//! Every write to a table finds its row's zone, every commit the zones of
//! the versions it superseded, and every entry an index-only scan reads
//! its zone's flag; behind a lock shared by readers each of those wrote
//! the lock's word, one cache line passed between every session using the
//! table. Here a zone is found by loads alone.
//!
//! The items are kept in chunks, each twice as long as the one before, so
//! none ever moves: a reference to one stays good as the sequence grows,
//! and a table of few rows keeps a few. A chunk is made whole, of items
//! not yet given; an item is given once it is filled and the length moved
//! past it. Growing is its callers' to do one at a time.

use std::array;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The items of the first chunk: a table of up to this many zones keeps
/// one small chunk of them.
const FIRST: usize = 8;
/// The chunks, each twice the one before: more items than a table has
/// blocks of row ids to give.
const CHUNKS: usize = 48;

/// \[DESIGN\] A sequence of `T` that only grows, read with no lock.
pub(super) struct Grown<T> {
    chunks: [OnceLock<Box<[T]>>; CHUNKS],
    /// The items given.
    len: AtomicUsize,
}

impl<T> Default for Grown<T> {
    fn default() -> Grown<T> {
        Grown { chunks: array::from_fn(|_| OnceLock::new()), len: AtomicUsize::new(0) }
    }
}

/// The chunk item `at` lies in, where in it, and how long the chunk is:
/// chunk `k` holds the `FIRST << k` items after those of the chunks
/// before it, `FIRST` times one less than two to the `k` of them.
fn place(at: usize) -> Option<(usize, usize, usize)> {
    let chunk = at.checked_div(FIRST)?.checked_add(1)?.ilog2();
    let size = FIRST.checked_shl(chunk)?;
    let before = size.checked_sub(FIRST)?;
    Some((usize::try_from(chunk).ok()?, at.checked_sub(before)?, size))
}

impl<T: Default> Grown<T> {
    /// How many items it holds.
    pub(super) fn len(&self) -> usize {
        self.len.load(Ordering::Acquire)
    }

    /// Item `at`, if it holds as many.
    pub(super) fn get(&self, at: usize) -> Option<&T> {
        if at >= self.len() {
            return None;
        }
        let (chunk, within, _) = place(at)?;
        self.chunks.get(chunk)?.get()?.get(within)
    }

    /// Its first `len` items, in order: of a length read once, so a pass
    /// over them and what it does past them agree on where they end.
    pub(super) fn first(&self, len: usize) -> impl Iterator<Item = &T> {
        self.chunks.iter().map_while(OnceLock::get).flat_map(|chunk| chunk.iter()).take(len.min(self.len()))
    }

    /// Adds an item, filled by `fill` before any reader finds it: whether
    /// it did, not past the last chunk. Called by one thread at a time.
    pub(super) fn push(&self, fill: impl FnOnce(&T)) -> bool {
        let at = self.len.load(Ordering::Acquire);
        let Some((chunk, within, size)) = place(at) else { return false };
        let Some(slot) = self.chunks.get(chunk) else { return false };
        let items = slot.get_or_init(|| (0..size).map(|_| T::default()).collect());
        let Some(item) = items.get(within) else { return false };
        fill(item);
        self.len.store(at.saturating_add(1), Ordering::Release);
        true
    }
}

// Unit tests: items are found where they were added, as the chunks grow
// (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use super::{FIRST, Grown, place};

    /// Verifies: [DESIGN] RULES.md D860: each item added is found at its
    /// place with what it was filled with, across the chunks' boundaries,
    /// none past the length; a reference taken before the sequence grew
    /// reads the same item after; and a pass over the first items stops at
    /// the length asked.
    #[test]
    fn items_are_found_where_they_were_added() {
        let grown: Grown<AtomicUsize> = Grown::default();
        assert!(grown.get(0).is_none());
        assert!(grown.push(|item| item.store(100, Ordering::Release)));
        let first = grown.get(0).unwrap();
        for n in 1..1000_usize {
            assert!(grown.push(|item| item.store(100 + n, Ordering::Release)));
        }
        assert_eq!(grown.len(), 1000);
        assert_eq!(first.load(Ordering::Acquire), 100);
        for n in 0..1000_usize {
            assert_eq!(grown.get(n).unwrap().load(Ordering::Acquire), 100 + n);
        }
        assert!(grown.get(1000).is_none(), "made with its chunk, not given");
        let passed: Vec<usize> = grown.first(10).map(|item| item.load(Ordering::Acquire)).collect();
        assert_eq!(passed, (100..110).collect::<Vec<_>>());
        assert_eq!(grown.first(usize::MAX).count(), 1000);
    }

    /// Verifies: [DESIGN] RULES.md D860: an item's chunk, its place in it
    /// and the chunk's length at the boundaries: the last of a chunk, and
    /// the first of the next, each chunk twice as long as the one before.
    #[test]
    fn a_chunk_is_twice_the_one_before() {
        assert_eq!(place(FIRST - 1), Some((0, FIRST - 1, FIRST)));
        assert_eq!(place(FIRST), Some((1, 0, 2 * FIRST)));
        assert_eq!(place(3 * FIRST - 1), Some((1, 2 * FIRST - 1, 2 * FIRST)));
        assert_eq!(place(3 * FIRST), Some((2, 0, 4 * FIRST)));
    }

    /// The length of `grown` as a reader finds it, each item below it
    /// checked filled with its place and one: the last by its place, and
    /// all of them by a pass.
    fn read_filled(grown: &Grown<AtomicUsize>) -> usize {
        let len = grown.len();
        if let Some(last) = len.checked_sub(1) {
            assert_eq!(grown.get(last).map(|item| item.load(Ordering::Acquire)), Some(len));
        }
        let passed = grown.first(len).enumerate().map(|(at, item)| assert_eq!(item.load(Ordering::Acquire), at.saturating_add(1)));
        assert_eq!(passed.count(), len);
        len
    }

    /// Reads `grown` as `read_filled` does, over and over, until it holds
    /// `items` or `done` is set.
    fn read_until(grown: &Grown<AtomicUsize>, done: &AtomicBool, items: usize) {
        while !done.load(Ordering::Acquire) && read_filled(grown) < items {
            std::hint::spin_loop();
        }
    }

    /// Verifies: [DESIGN] RULES.md D860: readers and a thread adding items
    /// at once: every item a reader finds below the length it read is
    /// filled, the one at its place, and a pass over the first items gives
    /// that many, each filled, while the sequence grows past its chunks.
    #[test]
    fn readers_find_only_items_filled_as_the_sequence_grows() {
        const ITEMS: usize = 20_000;
        let grown: Grown<AtomicUsize> = Grown::default();
        let done = AtomicBool::new(false);
        std::thread::scope(|scope| {
            for _ in 0..3_u8 {
                scope.spawn(|| read_until(&grown, &done, ITEMS));
            }
            scope.spawn(|| {
                // Counted, not asserted, so a failure ends the readers too.
                let pushed = (0..ITEMS).take_while(|&n| grown.push(|item| item.store(n.saturating_add(1), Ordering::Release))).count();
                done.store(true, Ordering::Release);
                assert_eq!(pushed, ITEMS);
            });
        });
        assert_eq!(read_filled(&grown), ITEMS);
    }
}

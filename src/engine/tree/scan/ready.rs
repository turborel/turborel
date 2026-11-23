//! \[DESIGN\] (RULES.md D809) The entries a scan took from its last leaf,
//! kept in one buffer of bytes the scan reuses from leaf to leaf. Each
//! entry copied into a key and a value of its own made two allocations,
//! and a key of a node with a shared prefix put back together, for every
//! entry of every range. A reader that borrows each entry
//! (`Scan::next_entry`) now allocates nothing once the buffer has grown
//! to a leaf's entries.
//!
//! A scan done gives its buffers back to its thread, and the thread's next
//! scan takes them (RULES.md D840), so a probe that reads one entry, a
//! scan of its own, allocates no buffer either.

use std::cell::RefCell;

use super::Direction;
use crate::engine::tree::node::KeyRef;

/// \[DESIGN\] The buffers a thread keeps for its next scans: one for each
/// scan a statement step may have open at once, an index's and a table's
/// and an outer one's.
const SPARES: usize = 4;
/// \[DESIGN\] The most bytes a buffer kept may hold: a few leaves' entries,
/// so a scan of long values leaves no large buffer behind.
const KEPT_BYTES: usize = 65_536;

std::thread_local! {
    static SPARE: RefCell<Vec<Ready>> = const { RefCell::new(Vec::new()) };
}

/// Entries taken from one leaf, in key order, given from the front going
/// forward and from the back going backward.
#[derive(Default)]
pub(super) struct Ready {
    /// Each entry's key and then its value.
    bytes: Vec<u8>,
    /// Where each entry's key starts, where its value starts, and where
    /// it ends, in `bytes`.
    spans: Vec<(usize, usize, usize)>,
    /// The entries before this one were given.
    front: usize,
    /// The entries from this one on were given.
    back: usize,
}

impl Ready {
    /// Buffers the thread kept from a scan done, or new ones.
    pub(super) fn reused() -> Ready {
        SPARE.with(|spare| spare.try_borrow_mut().ok().and_then(|mut spare| spare.pop())).unwrap_or_default()
    }

    /// Gives its buffers, cleared, to the thread's next scan, if they are
    /// not too large and the thread keeps fewer than `SPARES`.
    pub(super) fn give_back(mut self) {
        if self.bytes.capacity() > KEPT_BYTES {
            return;
        }
        self.clear();
        SPARE.with(|spare| {
            if let Ok(mut spare) = spare.try_borrow_mut()
                && spare.len() < SPARES
            {
                spare.push(self);
            }
        });
    }

    /// None taken, the buffers kept.
    pub(super) fn clear(&mut self) {
        self.bytes.clear();
        self.spans.clear();
        (self.front, self.back) = (0, 0);
    }

    /// Takes an entry, after those taken.
    pub(super) fn push(&mut self, key: KeyRef<'_>, value: &[u8]) {
        let start = self.bytes.len();
        // The whole entry reserved at once: put piece by piece, a buffer
        // still growing was reallocated for its key's prefix, its tail and
        // its value in turn, three times for a scan's one entry.
        self.bytes.reserve(key.len().saturating_add(value.len()));
        key.put(&mut self.bytes);
        let at = self.bytes.len();
        self.bytes.extend_from_slice(value);
        self.spans.push((start, at, self.bytes.len()));
        self.back = self.spans.len();
    }

    /// The entries taken, given or not, for tests.
    #[cfg(test)]
    pub(super) fn taken(&self) -> usize {
        self.spans.len()
    }

    /// Whether every entry taken was given.
    pub(super) fn is_empty(&self) -> bool {
        self.front >= self.back
    }

    /// The key of the first entry taken, the lowest.
    pub(super) fn first_key(&self) -> Option<&[u8]> {
        let &(start, at, _) = self.spans.first()?;
        self.bytes.get(start..at)
    }

    /// The key of the last entry taken, the highest.
    pub(super) fn last_key(&self) -> Option<&[u8]> {
        let &(start, at, _) = self.spans.last()?;
        self.bytes.get(start..at)
    }

    /// The next entry to give in `direction`, given.
    pub(super) fn give(&mut self, direction: Direction) -> Option<(&[u8], &[u8])> {
        if self.is_empty() {
            return None;
        }
        let place = match direction {
            Direction::Forward => {
                let place = self.front;
                self.front = place.checked_add(1)?;
                place
            }
            Direction::Backward => {
                self.back = self.back.checked_sub(1)?;
                self.back
            }
        };
        let &(start, at, end) = self.spans.get(place)?;
        Some((self.bytes.get(start..at)?, self.bytes.get(at..end)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D809: entries taken are given in key
    /// order forward and in reverse backward, each key put back together
    /// from its prefix; cleared, the buffer takes a new leaf's entries
    /// with no allocation.
    #[test]
    fn entries_are_given_in_order_from_one_buffer() {
        let mut ready = Ready::default();
        ready.push(KeyRef::new(b"ab", b"c"), b"1");
        ready.push(KeyRef::new(b"ab", b"d"), b"22");
        ready.push(KeyRef::whole(b"e"), b"");
        assert_eq!(ready.last_key(), Some(&b"e"[..]));
        let mut forward = Vec::new();
        while let Some((k, v)) = ready.give(Direction::Forward) {
            forward.push((k.to_vec(), v.to_vec()));
        }
        assert_eq!(forward, [(b"abc".to_vec(), b"1".to_vec()), (b"abd".to_vec(), b"22".to_vec()), (b"e".to_vec(), Vec::new())]);
        assert!(ready.is_empty());
        let allocation = ready.bytes.as_ptr();
        ready.clear();
        assert!(ready.is_empty() && ready.last_key().is_none());
        ready.push(KeyRef::whole(b"x"), b"9");
        ready.push(KeyRef::whole(b"y"), b"8");
        assert_eq!(ready.give(Direction::Backward), Some((&b"y"[..], &b"8"[..])));
        assert_eq!(ready.give(Direction::Backward), Some((&b"x"[..], &b"9"[..])));
        assert_eq!(ready.give(Direction::Backward), None);
        assert_eq!(ready.bytes.as_ptr(), allocation, "the buffer reused");
    }

    /// Verifies: [DESIGN] RULES.md D840: buffers given back are taken by
    /// the thread's next scan, cleared, with no allocation; at most
    /// `SPARES` are kept, and one past `KEPT_BYTES` is not.
    #[test]
    fn buffers_given_back_are_reused() {
        SPARE.with(|spare| spare.borrow_mut().clear());
        let mut ready = Ready::reused();
        ready.push(KeyRef::whole(b"k"), b"v");
        let allocation = ready.bytes.as_ptr();
        ready.give_back();
        let again = Ready::reused();
        assert!(again.is_empty() && again.last_key().is_none(), "cleared");
        assert_eq!(again.bytes.as_ptr(), allocation, "the buffer reused");
        for _ in 0..SPARES + 2 {
            let mut more = Ready::default();
            more.push(KeyRef::whole(b"k"), b"v");
            more.give_back();
        }
        assert_eq!(SPARE.with(|spare| spare.borrow().len()), SPARES, "at most SPARES kept");
        SPARE.with(|spare| spare.borrow_mut().clear());
        let mut large = Ready::default();
        large.bytes.reserve(KEPT_BYTES + 1);
        large.give_back();
        assert_eq!(SPARE.with(|spare| spare.borrow().len()), 0, "too large to keep");
    }
}

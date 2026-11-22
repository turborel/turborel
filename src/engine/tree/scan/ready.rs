//! \[DESIGN\] (RULES.md D809) The entries a scan took from its last leaf,
//! kept in one buffer of bytes the scan reuses from leaf to leaf. Each
//! entry copied into a key and a value of its own made two allocations,
//! and a key of a node with a shared prefix put back together, for every
//! entry of every range. A reader that borrows each entry
//! (`Scan::next_entry`) now allocates nothing once the buffer has grown
//! to a leaf's entries.

use super::Direction;
use crate::engine::tree::node::KeyRef;

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
    /// None taken, the buffers kept.
    pub(super) fn clear(&mut self) {
        self.bytes.clear();
        self.spans.clear();
        (self.front, self.back) = (0, 0);
    }

    /// Takes an entry, after those taken.
    pub(super) fn push(&mut self, key: KeyRef<'_>, value: &[u8]) {
        let start = self.bytes.len();
        key.put(&mut self.bytes);
        let at = self.bytes.len();
        self.bytes.extend_from_slice(value);
        self.spans.push((start, at, self.bytes.len()));
        self.back = self.spans.len();
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
}

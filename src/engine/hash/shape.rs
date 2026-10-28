//! \[DESIGN\] The shape of a linear hash table (`engine/hash.rs`): its
//! level and split pointer, packed in one word so readers take it in one
//! load. Split from hash.rs (RULES.md R-SIZE-2).

/// Bits of the packed shape that hold the split pointer.
const SPLIT_BITS: u32 = 56;

/// A level and split pointer: 2^level + split buckets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Shape {
    pub(super) level: u8,
    pub(super) split: u64,
}

impl Shape {
    pub(super) fn packed(self) -> u64 {
        (u64::from(self.level) << SPLIT_BITS) | (self.split & ((1_u64 << SPLIT_BITS).saturating_sub(1)))
    }

    pub(super) fn unpacked(word: u64) -> Shape {
        Shape { level: u8::try_from(word >> SPLIT_BITS).unwrap_or(u8::MAX), split: word & ((1_u64 << SPLIT_BITS).saturating_sub(1)) }
    }

    /// The buckets of this level, 2^L.
    pub(super) fn round(self) -> u64 {
        1_u64.checked_shl(u32::from(self.level)).unwrap_or(u64::MAX)
    }

    pub(super) fn buckets(self) -> u64 {
        self.round().saturating_add(self.split)
    }

    /// The bucket of a key whose hash is `h`.
    pub(super) fn bucket(self, h: u64) -> u64 {
        let low = h & self.round().saturating_sub(1);
        if low < self.split { h & self.round().saturating_mul(2).saturating_sub(1) } else { low }
    }

    /// The shape after one more split.
    pub(super) fn grown(self) -> Shape {
        let split = self.split.saturating_add(1);
        if split == self.round() { Shape { level: self.level.saturating_add(1), split: 0 } } else { Shape { level: self.level, split } }
    }

    /// The shape after one merge: its split pointer is the bucket the last
    /// merges into.
    pub(super) fn shrunk(self) -> Shape {
        if self.split == 0 {
            let level = self.level.saturating_sub(1);
            Shape { level, split: Shape { level, split: 0 }.round().saturating_sub(1) }
        } else {
            Shape { level: self.level, split: self.split.saturating_sub(1) }
        }
    }
}

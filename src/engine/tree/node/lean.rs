//! \[DESIGN\] (RULES.md D950, D955) A key looked for in a node's bytes
//! with no node made of them. A read by key wants one entry of its leaf,
//! and one child of each inner node above it, and `Node::parse` reads
//! every field of the header, checks the high key against the right
//! link, and builds a node a search then walks through fallible steps.
//! Here the bytes are read for what finding one key needs and no more:
//! what kind of node they are, the keys' prefix, the slots by their
//! heads between the header's hints, and the one entry whose head is the
//! key's.
//!
//! A key found in a leaf of its tree that is in the tree is the key's
//! entry, with no check of the leaf's range. Such a leaf holds only keys
//! of its range, as a split and a merge take a key from a leaf under the
//! leaf's latch before any other holds it for a reader to find changed
//! (`write.rs`, `merge.rs`). So a key found there is found where a
//! descent would find it. A key not found says nothing unless the reader
//! came down to the leaf for it: it may lie right of the leaf, or left
//! of one it was only guessed to be in.

use std::cmp::Ordering;

use super::key::head_of;
use super::{
    CHILD, COUNT_AT, DELETED, FIRST_AT, FLAGS_AT, HAS_HIGH, HEADER, HIGH_LEN_AT, LEN, LEVEL_AT, PREFIX_AT, SLOT, TREE_AT, field, hinted_in,
    split_slots, u16_of,
};

/// A node's bytes, opened for one key: what its header says of it, its
/// bytes before the slots, and the slots.
struct Lean<'a> {
    bytes: &'a [u8],
    header: &'a [u8; HEADER],
    /// Where its high key ends and its keys' prefix begins.
    prefix_at: usize,
    /// Where its prefix ends and its heap begins.
    heap: usize,
    front: &'a [u8],
    slotted: &'a [[u8; SLOT]],
}

/// What a leaf's bytes hold of a key (`held`).
#[derive(Debug, PartialEq, Eq)]
pub(in crate::engine::tree) enum Held<'a> {
    /// The key's entry: its slot and its value.
    At(usize, &'a [u8]),
    /// No entry of it, in a leaf of the tree, in it, whose range ends at
    /// or after the key: the key is not in the tree, if the leaf's range
    /// begins before it.
    Absent,
    /// Nothing to go by: no leaf of the tree in it, or one whose range
    /// ends before the key, or bytes malformed.
    Unknown,
}

impl<'a> Lean<'a> {
    /// `bytes` opened, if they are long enough for the node their header
    /// tells of, taken out of its tree or not.
    #[inline]
    fn open(bytes: &'a [u8]) -> Option<Lean<'a>> {
        let header = bytes.first_chunk::<HEADER>()?;
        let prefix_at = HEADER.checked_add(u16_of(header, HIGH_LEN_AT))?;
        let heap = prefix_at.checked_add(u16_of(header, PREFIX_AT))?;
        let slots = u16_of(header, COUNT_AT).checked_mul(SLOT).and_then(|s| bytes.len().checked_sub(s)).filter(|&s| s >= heap)?;
        let (front, slotted) = split_slots(bytes, slots)?;
        Some(Lean { bytes, header, prefix_at, heap, front, slotted })
    }

    /// Its level, and whether it was taken out of its tree.
    #[inline]
    fn kind(&self) -> (u8, bool) {
        let ([level], [flags]) = (field(self.header, LEVEL_AT), field::<1>(self.header, FLAGS_AT));
        (level, flags & DELETED != 0)
    }

    /// Whether `key` is in its range: not above its high key, if it has one.
    #[inline]
    fn covers(&self, key: &[u8]) -> Option<bool> {
        let [flags] = field(self.header, FLAGS_AT);
        Some(flags & HAS_HIGH == 0 || key <= self.bytes.get(HEADER..self.prefix_at)?)
    }

    /// The tail of the entry at `at` of its heap, and where its value's
    /// length is.
    #[inline]
    fn tail_from(&self, at: usize) -> Option<(&'a [u8], usize)> {
        let at = self.heap.checked_add(at)?;
        let start = at.checked_add(LEN)?;
        let len = usize::from(u16::from_le_bytes(*self.front.get(at..)?.first_chunk::<LEN>()?));
        let end = start.checked_add(len)?;
        Some((self.front.get(start..end)?, end))
    }

    /// The value whose length is at `at`.
    #[inline]
    fn value_from(&self, at: usize) -> Option<&'a [u8]> {
        let start = at.checked_add(LEN)?;
        let len = usize::from(u16::from_le_bytes(*self.front.get(at..)?.first_chunk::<LEN>()?));
        self.front.get(start..start.checked_add(len)?)
    }

    /// How the key of slot `i` compares with the key whose tail is `tail`
    /// and head `head`, and where its value's length is, read only where
    /// the heads are equal.
    #[inline]
    fn entry(&self, i: usize, (tail, head): (&[u8], u16)) -> Option<(Ordering, usize)> {
        let &[o0, o1, h0, h1] = self.slotted.get(i)?;
        match u16::from_be_bytes([h0, h1]).cmp(&head) {
            Ordering::Equal => {
                let (found, value_at) = self.tail_from(usize::from(u16::from_le_bytes([o0, o1])))?;
                Some((found.cmp(tail), value_at))
            }
            unequal @ (Ordering::Less | Ordering::Greater) => Some((unequal, 0)),
        }
    }

    /// Where the key of tail `tail` is among its slots, as `Node::search`
    /// places it, with where its value's length is if it is there.
    #[inline]
    fn find(&self, tail: &[u8]) -> Option<Result<(usize, usize), usize>> {
        let head = head_of(tail);
        let (mut low, mut high) = hinted_in(self.bytes, self.slotted.len(), head);
        while low < high {
            let mid = low.midpoint(high);
            match self.entry(mid, (tail, head))? {
                (Ordering::Less, _) => low = mid.saturating_add(1),
                (Ordering::Greater, _) => high = mid,
                (Ordering::Equal, value_at) => return Some(Ok((mid, value_at))),
            }
        }
        Some(Err(low))
    }
}

/// What the bytes of a leaf hold of `key`, for the tree whose meta page
/// is `tree`: its slot and value, if they are a leaf made in that tree,
/// not taken out of it, that holds `key`. Slot `first`, if given, is
/// tried before the others, with one comparison.
#[inline]
pub(in crate::engine::tree) fn held<'a>(bytes: &'a [u8], tree: u64, key: &[u8], first: Option<usize>) -> Held<'a> {
    sought(bytes, tree, key, first).unwrap_or(Held::Unknown)
}

/// `held`, none of bytes malformed.
#[inline]
fn sought<'a>(bytes: &'a [u8], tree: u64, key: &[u8], first: Option<usize>) -> Option<Held<'a>> {
    let lean = Lean::open(bytes)?;
    if lean.kind() != (0, false) || u64::from_le_bytes(field(lean.header, TREE_AT)) != tree {
        return Some(Held::Unknown);
    }
    let absent = if lean.covers(key)? { Held::Absent } else { Held::Unknown };
    let Some(tail) = key.strip_prefix(bytes.get(lean.prefix_at..lean.heap)?) else { return Some(absent) };
    if let Some((slot, (Ordering::Equal, value_at))) = first.and_then(|slot| Some((slot, lean.entry(slot, (tail, head_of(tail)))?))) {
        return Some(Held::At(slot, lean.value_from(value_at)?));
    }
    match lean.find(tail)? {
        Ok((slot, value_at)) => Some(Held::At(slot, lean.value_from(value_at)?)),
        Err(_) => Some(absent),
    }
}

/// The child of the inner node `bytes` hold whose range holds `key`, and
/// the node's level, as `Node::child` gives it: none if they are no inner
/// node in its tree whose range holds `key`, so a reader moves right or
/// starts again, or if they are malformed.
#[inline]
pub(in crate::engine::tree) fn child(bytes: &[u8], key: &[u8]) -> Option<(u64, u8)> {
    let lean = Lean::open(bytes)?;
    let (level, deleted) = lean.kind();
    if level == 0 || deleted || !lean.covers(key)? {
        return None;
    }
    let prefix = bytes.get(lean.prefix_at..lean.heap)?;
    // The entries below `key`: one equal to an entry's belongs to the
    // child on its left.
    let below = match key.strip_prefix(prefix) {
        Some(tail) => lean.find(tail)?.map_or_else(|at| at, |(at, _)| at),
        None if key < prefix => 0,
        None => lean.slotted.len(),
    };
    let Some(last) = below.checked_sub(1) else { return Some((u64::from_le_bytes(field(lean.header, FIRST_AT)), level)) };
    let &[o0, o1, _, _] = lean.slotted.get(last)?;
    let value = lean.value_from(lean.tail_from(usize::from(u16::from_le_bytes([o0, o1])))?.1)?;
    Some((u64::from_le_bytes(*value.first_chunk::<CHILD>().filter(|_| value.len() == CHILD)?), level))
}

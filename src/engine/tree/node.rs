//! \[DESIGN\] The bytes of a B-link tree node (RULES.md D31, D500; Lehman
//! and Yao, "Efficient Locking for Concurrent Operations on B-Trees", ACM
//! TODS 6(4), 1981, §3): one page's content, read in place, searched over a
//! slot array of key heads, and changed in place when an entry fits; a
//! value replaced by one of its length is written over, nothing moved.
//!
//! ```text
//! header (68 bytes, integers little-endian):
//!   level u8 (0: a leaf), flags u8 (1: it has a high key; 2: it is
//!   deleted, its right link leading to the node that took its range,
//!   `reclaim.rs`), entries u16,
//!   garbage u16 (bytes of removed entries, and of values written over
//!   shorter, not yet reclaimed),
//!   high key length u16, right neighbour u64 (none: all ones),
//!   first child u64 (inner nodes; 0 in a leaf),
//!   last insert u16 (the slot of the entry inserted last; none: all ones),
//!   prefix length u16 (the bytes every key of the node begins with),
//!   tree u64 (the meta page of the tree it belongs to),
//!   hints, 16 key heads (those of the slots at sixteen even steps
//!   through the slots, while there are at least 32)
//! high key
//! prefix
//! heap:   entries, each tail length u16, tail (the key past the prefix),
//!         value length u16, value, in the order they were added, or in
//!         key order once rebuilt
//! slots:  for each entry, in key order, its offset in the heap u16, and
//!         its head: the first 2 bytes of its tail, zeros past its end,
//!         big-endian
//! ```
//!
//! A node holds the keys above its left neighbour's high key, up to and
//! including its own; the rightmost node of a level has no high key, and
//! only it has no right neighbour. In an inner node the first child holds
//! the keys up to the first entry's key, and each entry's child (its value,
//! the page number as u64) those above its key up to the next one's: a key
//! there is only an upper bound for the child to its left (§3, §7).
//!
//! **Prefixes and heads** (Müller, Benson and Leis, "B-Trees Are Back",
//! 2025, §3-4). A node built keeps once the bytes its first key shares with
//! its last, which every key between them shares. A key inserted that does
//! not share them shortens the prefix, the node built again (`build.rs`),
//! as when keys appended to the rightmost node of a level pass a byte's
//! boundary. A search compares heads, 2-byte integers beside the slots,
//! and reads a tail only where the heads are equal. The hints narrow it to
//! one sixteenth of the slots before it starts. Heads of two bytes, after
//! the prefix, tell most keys of a node apart and cost a slot two bytes
//! less than the paper's four, so a node holds as many entries as one with
//! no prefix and no heads would.
//!
//! **Where a full node splits** (an extension: Lehman and Yao split a node
//! into halves, §3.1.3(b)). In the middle, by bytes, unless inserts are
//! running in key order through it: the entry added goes right after the
//! one inserted last, or past the end of the rightmost node of its level.
//! Then it splits where the entry goes: the entries before it stay, and
//! it starts the right half, where the run goes on. In an inner node the
//! entry just before it goes up instead. A run that reaches the end of a
//! node leaves it full. One thread inserting in key order (row ids, within
//! its block, `rows/ids.rs`) thus leaves full nodes behind it; inserts at
//! random rarely run, so split in the middle. And a run that begins past
//! the end of the rightmost leaf while another run goes on inside it (a
//! thread's new block of row ids) gets a leaf of its own at once
//! (`tree.rs`), so no entries of one run ride along with the other's as
//! it fills its leaves, and threads inserting into one table at once fill
//! their leaves nearly as one alone does.
//!
//! Where a node splits does not touch the paper's correctness: its proofs
//! rest on keys moving only right, the right half written before the left
//! is linked to it, never on how the entries are divided (§6.2). It gives
//! up the bound of k to 2k entries a node, which deletion already gives up
//! (§7).

mod build;
mod key;
mod parsed;
mod split;

pub(super) use build::{
    Shape, build, built, built_size, compact, deleted, grown, insert, lengthens, overwrite, remove, set_child, set_right, stored, value_mut,
};
pub(super) use key::KeyRef;
use key::head_of;
pub(crate) use parsed::Parsed;
pub(super) use split::{shortest_between, split};

use crate::disk::pager::PageId;
use crate::error::{Code, Result, SqlError};

/// Bytes of a node's header.
pub(super) const HEADER: usize = 68;
/// Bytes of an entry beyond its key and value: two lengths and its slot.
pub(super) const ENTRY_OVERHEAD: usize = 8;
/// Bytes of an inner entry's value, a page number.
pub(super) const CHILD: usize = 8;

const LEVEL_AT: usize = 0;
const FLAGS_AT: usize = 1;
const COUNT_AT: usize = 2;
const GARBAGE_AT: usize = 4;
const HIGH_LEN_AT: usize = 6;
const RIGHT_AT: usize = 8;
const FIRST_AT: usize = 16;
const LAST_AT: usize = 24;
const PREFIX_AT: usize = 26;
const TREE_AT: usize = 28;
const HINTS_AT: usize = 36;
/// \[DESIGN\] Key heads sampled into the header (Müller, Benson and Leis
/// 2025, §3, who keep 16).
const HINTS: usize = 16;
/// The slots below which a search uses no hints: fewer than two a step.
const HINTED: usize = 32;
/// Bytes of the hints.
const HINT_BYTES: usize = HINTS * key::HEAD;
/// No entry inserted last, as in a node just built.
const NO_LAST: usize = 0xFFFF;
const HAS_HIGH: u8 = 1;
const DELETED: u8 = 2;
/// Bytes of a length field, and of a slot's offset.
const LEN: usize = 2;
/// Bytes of a slot: its offset and its key's head.
const SLOT: usize = 4;
const NO_PAGE: u64 = u64::MAX;

fn corrupt(why: &str) -> SqlError {
    SqlError::new(Code::Corrupt, format!("[DESIGN] a B-link tree node is malformed: {why}"))
}

/// `len` bytes of `bytes` from `at`.
fn slice(bytes: &[u8], at: usize, len: usize) -> Result<&[u8]> {
    at.checked_add(len).and_then(|end| bytes.get(at..end)).ok_or_else(|| corrupt("a field runs past its end"))
}

fn u16_at(bytes: &[u8], at: usize) -> Result<usize> {
    let field = <[u8; 2]>::try_from(slice(bytes, at, 2)?).map_err(|_| corrupt("a field runs past its end"))?;
    Ok(usize::from(u16::from_le_bytes(field)))
}

/// `N` bytes of a header from `at`, a place every header has, so the read
/// cannot fail and builds no error.
#[inline]
fn field<const N: usize>(header: &[u8; HEADER], at: usize) -> [u8; N] {
    header.get(at..).and_then(<[u8]>::first_chunk::<N>).copied().unwrap_or([0; N])
}

/// A u16 field of a header.
#[inline]
fn u16_of(header: &[u8; HEADER], at: usize) -> usize {
    usize::from(u16::from_le_bytes(field(header, at)))
}

/// A node's bytes before its slots, which begin at `slots`, and the slots;
/// none if the slots do not run to the end in whole slots.
#[inline]
fn split_slots(bytes: &[u8], slots: usize) -> Option<(&[u8], &[[u8; SLOT]])> {
    let (front, back) = bytes.split_at_checked(slots)?;
    let (slotted, rest) = back.as_chunks::<SLOT>();
    rest.is_empty().then_some((front, slotted))
}

/// The page number of an inner entry's value.
pub(super) fn child_of(value: &[u8]) -> Result<PageId> {
    <[u8; 8]>::try_from(value).map(|v| PageId(u64::from_le_bytes(v))).map_err(|_| corrupt("a child is not a page number"))
}

/// Bytes an entry of `key` and `value` takes in a node, its slot included,
/// at most: less the node's prefix.
pub(super) fn entry_size(key: &[u8], value: &[u8]) -> usize {
    ENTRY_OVERHEAD.saturating_add(key.len()).saturating_add(value.len())
}

/// A node, read in place.
#[derive(Clone, Copy)]
pub(super) struct Node<'a> {
    bytes: &'a [u8],
    /// Its height above the leaves: 0 for a leaf.
    pub(super) level: u8,
    /// The highest key it may hold; none for the rightmost node of a level.
    pub(super) high: Option<&'a [u8]>,
    /// Its right neighbour, at the same level; of a node deleted, the one
    /// that took its range.
    pub(super) right: Option<PageId>,
    /// Whether it was taken out of its level, emptied or merged
    /// (`reclaim.rs`, `merge.rs`).
    pub(super) deleted: bool,
    /// The meta page of the tree it was made in.
    pub(super) tree: u64,
    /// The bytes every key of it begins with.
    prefix: &'a [u8],
    first: PageId,
    count: usize,
    garbage: usize,
    /// The slot of the entry inserted last, if known.
    last: Option<usize>,
    heap: usize,
    slots: usize,
    /// Its bytes before the slots: header, high key, prefix and heap.
    front: &'a [u8],
    /// Its slots, each its entry's offset in the heap and its key's head,
    /// found to lie within its bytes as it was parsed.
    slotted: &'a [[u8; SLOT]],
}

impl<'a> Node<'a> {
    /// The node `bytes` hold.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if they are not a node.
    pub(super) fn parse(bytes: &'a [u8]) -> Result<Node<'a>> {
        let header = bytes.first_chunk::<HEADER>().ok_or_else(|| corrupt("it is shorter than its header"))?;
        let flags = u8::from_le_bytes(field(header, FLAGS_AT));
        let (count, high_len) = (u16_of(header, COUNT_AT), u16_of(header, HIGH_LEN_AT));
        let right = Some(u64::from_le_bytes(field(header, RIGHT_AT))).filter(|&r| r != NO_PAGE).map(PageId);
        let high_key = slice(bytes, HEADER, high_len)?;
        let high = (flags & HAS_HIGH != 0).then_some(high_key);
        let deleted = flags & DELETED != 0;
        if high.is_some() != right.is_some() && !deleted {
            return Err(corrupt("it has a high key but no right neighbour, or one but not the other"));
        }
        let prefix_at = HEADER.saturating_add(high_len);
        let prefix = slice(bytes, prefix_at, u16_of(header, PREFIX_AT)).map_err(|_| corrupt("its prefix runs past its end"))?;
        let heap = prefix_at.saturating_add(prefix.len());
        let slots = count
            .checked_mul(SLOT)
            .and_then(|s| bytes.len().checked_sub(s))
            .filter(|&s| s >= heap)
            .ok_or_else(|| corrupt("its slots overlap its header"))?;
        let (front, slotted) = split_slots(bytes, slots).ok_or_else(|| corrupt("its slots overlap its header"))?;
        Ok(Node {
            bytes,
            level: u8::from_le_bytes(field(header, LEVEL_AT)),
            high,
            right,
            deleted,
            tree: u64::from_le_bytes(field(header, TREE_AT)),
            prefix,
            first: PageId(u64::from_le_bytes(field(header, FIRST_AT))),
            count,
            garbage: u16_of(header, GARBAGE_AT),
            last: Some(u16_of(header, LAST_AT)).filter(|&l| l != NO_LAST),
            heap,
            slots,
            front,
            slotted,
        })
    }

    /// Its entries.
    pub(super) fn len(&self) -> usize {
        self.count
    }

    /// The bytes of its entries removed, or replaced, not yet reclaimed.
    pub(super) fn garbage(&self) -> usize {
        self.garbage
    }

    /// Bytes it takes, less those of removed entries not yet reclaimed.
    pub(super) fn live(&self) -> usize {
        self.bytes.len().saturating_sub(self.garbage)
    }

    /// Where slot `i` is.
    fn slot(&self, i: usize) -> Result<usize> {
        i.checked_mul(SLOT).and_then(|s| self.slots.checked_add(s)).ok_or_else(|| corrupt("a slot is past its end"))
    }

    /// Slot `i`: where its entry begins in the node's bytes, and its
    /// key's head. None past the last slot.
    #[inline]
    fn slot_of(&self, i: usize) -> Option<(usize, u16)> {
        let &[o0, o1, h0, h1] = self.slotted.get(i)?;
        Some((self.heap.checked_add(usize::from(u16::from_le_bytes([o0, o1])))?, u16::from_be_bytes([h0, h1])))
    }

    /// The tail of the entry beginning at `at`, and where its value's
    /// length is; none if it runs past the heap.
    #[inline]
    fn tail_from(&self, at: usize) -> Option<(&'a [u8], usize)> {
        let start = at.checked_add(LEN)?;
        let len = usize::from(u16::from_le_bytes(*self.front.get(at..)?.first_chunk::<LEN>()?));
        let end = start.checked_add(len)?;
        Some((self.front.get(start..end)?, end))
    }

    /// The head of entry `i`'s key.
    fn head(&self, i: usize) -> Result<u16> {
        self.slot_of(i).map(|(_, head)| head).ok_or_else(|| corrupt("a slot is past its end"))
    }

    /// Entry `i`'s tail, the key past the prefix, and where its value's
    /// length is.
    fn tail_at(&self, i: usize) -> Result<(&'a [u8], usize)> {
        self.slot_of(i).and_then(|(at, _)| self.tail_from(at)).ok_or_else(|| corrupt("a field runs past its end"))
    }

    /// Entry `i`, in key order: its key and value.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if there is no entry `i`, or it is malformed.
    pub(super) fn entry(&self, i: usize) -> Result<(KeyRef<'a>, &'a [u8])> {
        let (tail, value_at) = self.tail_at(i)?;
        let value = slice(self.front, value_at.saturating_add(LEN), u16_at(self.front, value_at)?)?;
        Ok((KeyRef::new(self.prefix, tail), value))
    }

    /// Entry `i`'s value.
    ///
    /// # Errors
    ///
    /// As `entry`.
    pub(super) fn value(&self, i: usize) -> Result<&'a [u8]> {
        self.entry(i).map(|(_, value)| value)
    }

    /// Where entry `i`'s value begins in the node's bytes.
    ///
    /// # Errors
    ///
    /// As `entry`.
    fn value_at(&self, i: usize) -> Result<usize> {
        Ok(self.tail_at(i)?.1.saturating_add(LEN))
    }

    /// Entry `i`'s key.
    ///
    /// # Errors
    ///
    /// As `entry`.
    pub(super) fn key(&self, i: usize) -> Result<KeyRef<'a>> {
        Ok(KeyRef::new(self.prefix, self.tail_at(i)?.0))
    }

    /// Where `key` is: `Ok(i)` if entry `i` holds it, `Err(i)` if it would
    /// go before entry `i`. A key without the node's prefix is below or
    /// above every entry; else its tail is searched by heads, between the
    /// hints that bound its head.
    ///
    /// # Errors
    ///
    /// As `entry`.
    pub(super) fn search(&self, key: &[u8]) -> Result<std::result::Result<usize, usize>> {
        let Some(tail) = key.strip_prefix(self.prefix) else {
            return Ok(Err(if key < self.prefix { 0 } else { self.count }));
        };
        self.find(tail).ok_or_else(|| corrupt("an entry is past its end"))
    }

    /// `search`'s binary search for `tail` over the slots, by heads, a
    /// tail read only where the heads are equal; none if a slot leads
    /// past the heap. No step builds an error, so the search stays in
    /// registers (RULES.md D840).
    #[inline]
    fn find(&self, tail: &[u8]) -> Option<std::result::Result<usize, usize>> {
        let head = head_of(tail);
        let (mut low, mut high) = self.hinted(head);
        while low < high {
            let mid = low.midpoint(high);
            let (at, found) = self.slot_of(mid)?;
            let order = match found.cmp(&head) {
                std::cmp::Ordering::Equal => self.tail_from(at)?.0.cmp(tail),
                unequal @ (std::cmp::Ordering::Less | std::cmp::Ordering::Greater) => unequal,
            };
            match order {
                std::cmp::Ordering::Less => low = mid.saturating_add(1),
                std::cmp::Ordering::Greater => high = mid,
                std::cmp::Ordering::Equal => return Some(Ok(mid)),
            }
        }
        Some(Err(low))
    }

    /// The slots a key of head `head` may be among, as the hints bound it:
    /// past every hint below it, up to the first above it.
    #[inline]
    fn hinted(&self, head: u16) -> (usize, usize) {
        let (mut low, mut high) = (0, self.count);
        let Some(step) = hint_step(self.count) else { return (low, high) };
        let Some(hints) = self.bytes.get(HINTS_AT..).and_then(<[u8]>::first_chunk::<HINT_BYTES>) else { return (low, high) };
        let mut at = 0_usize;
        for &hint in hints.as_chunks::<{ key::HEAD }>().0 {
            at = at.saturating_add(step);
            let hint = u16::from_be_bytes(hint);
            if hint < head {
                low = at.saturating_add(1);
            } else if hint > head {
                high = at;
                break;
            }
        }
        (low.min(self.count), high.max(low).min(self.count))
    }

    /// Whether `key` is in its range: not above its high key. A key above
    /// it has moved right, past a split (§4).
    pub(super) fn covers(&self, key: &[u8]) -> bool {
        self.high.is_none_or(|high| key <= high)
    }

    /// The child whose range holds `key`, and the key below that range, if
    /// this node holds it.
    ///
    /// # Errors
    ///
    /// As `entry`.
    pub(super) fn child(&self, key: &[u8]) -> Result<(PageId, Option<KeyRef<'a>>)> {
        // The entries below `key`: a key equal to an entry's belongs to the
        // child on its left.
        let below = self.search(key)?.unwrap_or_else(|at| at);
        self.child_at(below)
    }

    /// Child `at`: the first child for 0, else entry `at - 1`'s, and the key
    /// below its range, if this node holds it.
    ///
    /// # Errors
    ///
    /// As `entry`.
    pub(super) fn child_at(&self, at: usize) -> Result<(PageId, Option<KeyRef<'a>>)> {
        let Some(i) = at.checked_sub(1) else { return Ok((self.first, None)) };
        let (key, value) = self.entry(i)?;
        Ok((child_of(value)?, Some(key)))
    }

    /// The entry inserted last, if it is still there: its index.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if the mark is past its entries.
    pub(super) fn last_insert(&self) -> Result<Option<usize>> {
        match self.last {
            Some(last) if last >= self.count => Err(corrupt("the entry inserted last is past its entries")),
            Some(_) | None => Ok(self.last),
        }
    }

    /// Its first child.
    pub(super) fn first_child(&self) -> PageId {
        self.first
    }

    /// Its entries, in key order.
    ///
    /// # Errors
    ///
    /// As `entry`.
    pub(super) fn entries(&self) -> Result<Vec<(KeyRef<'a>, &'a [u8])>> {
        (0..self.count).map(|i| self.entry(i)).collect()
    }

    /// Its shape, the fields besides its entries.
    pub(super) fn shape(&self) -> Shape<'a> {
        Shape { level: self.level, high: self.high, right: self.right, first: self.first, tree: self.tree }
    }
}

/// The slots between hints in a node of `count` entries, if it has enough
/// to use them.
fn hint_step(count: usize) -> Option<usize> {
    (count >= HINTED).then(|| count.checked_div(HINTS.saturating_add(1)).unwrap_or_default())
}

// Unit tests: nodes read back as built, searched in key order, changed in
// place, and split as `split` says (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;

//! \[DESIGN\] The bytes of a B-link tree node (RULES.md D31; Lehman and
//! Yao, "Efficient Locking for Concurrent Operations on B-Trees", ACM TODS
//! 6(4), 1981, §3): one page's content, read in place, searched by binary
//! search over a slot array, and changed in place when an entry fits.
//!
//! ```text
//! header (26 bytes, integers little-endian):
//!   level u8 (0: a leaf), flags u8 (1: it has a high key; 2: it is
//!   deleted, its right link leading to the node that took its range,
//!   `reclaim.rs`), entries u16,
//!   garbage u16 (bytes of removed entries not yet reclaimed),
//!   high key length u16, right neighbour u64 (none: all ones),
//!   first child u64 (inner nodes; 0 in a leaf),
//!   last insert u16 (the heap offset of the entry inserted last; none:
//!   all ones)
//! high key
//! heap:   entries, each key length u16, key, value length u16, value,
//!         in the order they were added
//! slots:  for each entry, in key order, its offset in the heap, u16
//! ```
//!
//! A node holds the keys above its left neighbour's high key, up to and
//! including its own; the rightmost node of a level has no high key, and
//! only it has no right neighbour. In an inner node the first child holds
//! the keys up to the first entry's key, and each entry's child (its value,
//! the page number as u64) those above its key up to the next one's: a key
//! there is only an upper bound for the child to its left (§3, §7).
//!
//! **Where a full node splits** (an extension: Lehman and Yao split a node
//! into halves, §3.1.3(b)). In the middle, by bytes, unless inserts are
//! running in key order through it: the entry added goes right after the
//! one inserted last, or past the end of the rightmost node of its level.
//! Then it splits where the entry goes: the entries before it stay, the
//! node full, and it starts the right half, where the run goes on. One
//! thread inserting in key order (row ids, within its block,
//! `rows/ids.rs`) thus leaves full nodes behind it; inserts at random
//! rarely run, so split in the middle. And a run that begins past the end
//! of the rightmost leaf while another run goes on inside it (a thread's
//! new block of row ids) gets a leaf of its own at once (`tree.rs`), so no
//! entries of one run ride along with the other's as it fills its leaves.
//! Measured with threads inserting into one table, eight at once fill
//! their leaves to 96% of what one alone does, against 55% with halves.
//!
//! Where a node splits does not touch the paper's correctness: its proofs
//! rest on keys moving only right, the right half written before the left
//! is linked to it, never on how the entries are divided (§6.2). It gives
//! up the bound of k to 2k entries a node, which deletion already gives up
//! (§7).

use crate::disk::pager::PageId;
use crate::error::{Code, Result, SqlError};

/// Bytes of a node's header.
pub(super) const HEADER: usize = 26;
/// Bytes of an entry beyond its key and value: two lengths and its slot.
pub(super) const ENTRY_OVERHEAD: usize = 6;
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
/// No entry inserted last, as in a node just built.
const NO_LAST: usize = 0xFFFF;
const HAS_HIGH: u8 = 1;
const DELETED: u8 = 2;
const SLOT: usize = 2;
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

fn u64_at(bytes: &[u8], at: usize) -> Result<u64> {
    let field = <[u8; 8]>::try_from(slice(bytes, at, 8)?).map_err(|_| corrupt("a field runs past its end"))?;
    Ok(u64::from_le_bytes(field))
}

/// `value` as a u16 field.
fn u16_of(value: usize) -> Result<[u8; 2]> {
    u16::try_from(value).map(u16::to_le_bytes).map_err(|_| corrupt("a length or offset is past 65,535"))
}

fn set_u16(bytes: &mut [u8], at: usize, value: usize) -> Result<()> {
    let field = u16_of(value)?;
    let end = at.checked_add(SLOT).ok_or_else(|| corrupt("a field runs past its end"))?;
    bytes.get_mut(at..end).ok_or_else(|| corrupt("a field runs past its end"))?.copy_from_slice(&field);
    Ok(())
}

/// The page number of an inner entry's value.
pub(super) fn child_of(value: &[u8]) -> Result<PageId> {
    <[u8; 8]>::try_from(value).map(|v| PageId(u64::from_le_bytes(v))).map_err(|_| corrupt("a child is not a page number"))
}

/// Bytes an entry of `key` and `value` takes in a node, its slot included.
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
    /// Whether it was taken out of its level, emptied (`reclaim.rs`).
    pub(super) deleted: bool,
    first: PageId,
    count: usize,
    garbage: usize,
    /// The heap offset of the entry inserted last, if known.
    last: Option<usize>,
    heap: usize,
    slots: usize,
}

impl<'a> Node<'a> {
    /// The node `bytes` hold.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if they are not a node.
    pub(super) fn parse(bytes: &'a [u8]) -> Result<Node<'a>> {
        let header = slice(bytes, 0, HEADER).map_err(|_| corrupt("it is shorter than its header"))?;
        let (level, flags) = (header.get(LEVEL_AT).copied().unwrap_or_default(), header.get(FLAGS_AT).copied().unwrap_or_default());
        let count = u16_at(bytes, COUNT_AT)?;
        let high_len = u16_at(bytes, HIGH_LEN_AT)?;
        let right = Some(u64_at(bytes, RIGHT_AT)?).filter(|&r| r != NO_PAGE).map(PageId);
        let high_key = slice(bytes, HEADER, high_len)?;
        let high = (flags & HAS_HIGH != 0).then_some(high_key);
        let deleted = flags & DELETED != 0;
        if high.is_some() != right.is_some() && !deleted {
            return Err(corrupt("it has a high key but no right neighbour, or one but not the other"));
        }
        let heap = HEADER.saturating_add(high_len);
        let slots = count
            .checked_mul(SLOT)
            .and_then(|s| bytes.len().checked_sub(s))
            .filter(|&s| s >= heap)
            .ok_or_else(|| corrupt("its slots overlap its header"))?;
        Ok(Node {
            bytes,
            level,
            high,
            right,
            deleted,
            first: PageId(u64_at(bytes, FIRST_AT)?),
            count,
            garbage: u16_at(bytes, GARBAGE_AT)?,
            last: Some(u16_at(bytes, LAST_AT)?).filter(|&l| l != NO_LAST),
            heap,
            slots,
        })
    }

    /// Its entries.
    pub(super) fn len(&self) -> usize {
        self.count
    }

    /// Bytes it takes, less those of removed entries not yet reclaimed.
    pub(super) fn live(&self) -> usize {
        self.bytes.len().saturating_sub(self.garbage)
    }

    /// Entry `i`, in key order: its key and value.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if there is no entry `i`, or it is malformed.
    pub(super) fn entry(&self, i: usize) -> Result<(&'a [u8], &'a [u8])> {
        let slot = i.checked_mul(SLOT).and_then(|s| self.slots.checked_add(s)).ok_or_else(|| corrupt("a slot is past its end"))?;
        let at = self.heap.checked_add(u16_at(self.bytes, slot)?).ok_or_else(|| corrupt("an entry is past its end"))?;
        let heap = self.bytes.get(..self.slots).ok_or_else(|| corrupt("its heap is past its end"))?;
        let key_len = u16_at(heap, at)?;
        let key = slice(heap, at.saturating_add(SLOT), key_len)?;
        let value_at = at.saturating_add(SLOT).saturating_add(key_len);
        let value = slice(heap, value_at.saturating_add(SLOT), u16_at(heap, value_at)?)?;
        Ok((key, value))
    }

    /// Where entry `i`'s value begins in the node's bytes.
    ///
    /// # Errors
    ///
    /// As `entry`.
    fn value_at(&self, i: usize) -> Result<usize> {
        let slot = i.checked_mul(SLOT).and_then(|s| self.slots.checked_add(s)).ok_or_else(|| corrupt("a slot is past its end"))?;
        let at = self.heap.checked_add(u16_at(self.bytes, slot)?).ok_or_else(|| corrupt("an entry is past its end"))?;
        let key_len = u16_at(self.bytes, at)?;
        Ok(at.saturating_add(SLOT).saturating_add(key_len).saturating_add(SLOT))
    }

    /// Entry `i`'s key.
    ///
    /// # Errors
    ///
    /// As `entry`.
    pub(super) fn key(&self, i: usize) -> Result<&'a [u8]> {
        Ok(self.entry(i)?.0)
    }

    /// Where `key` is: `Ok(i)` if entry `i` holds it, `Err(i)` if it would
    /// go before entry `i`.
    ///
    /// # Errors
    ///
    /// As `entry`.
    pub(super) fn search(&self, key: &[u8]) -> Result<std::result::Result<usize, usize>> {
        let (mut low, mut high) = (0_usize, self.count);
        while low < high {
            let mid = low.midpoint(high);
            match self.key(mid)?.cmp(key) {
                std::cmp::Ordering::Less => low = mid.saturating_add(1),
                std::cmp::Ordering::Greater => high = mid,
                std::cmp::Ordering::Equal => return Ok(Ok(mid)),
            }
        }
        Ok(Err(low))
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
    pub(super) fn child(&self, key: &[u8]) -> Result<(PageId, Option<&'a [u8]>)> {
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
    pub(super) fn child_at(&self, at: usize) -> Result<(PageId, Option<&'a [u8]>)> {
        let Some(i) = at.checked_sub(1) else { return Ok((self.first, None)) };
        let (key, value) = self.entry(i)?;
        Ok((child_of(value)?, Some(key)))
    }

    /// The entry inserted last, if it is still there: its index.
    ///
    /// # Errors
    ///
    /// As `entry`.
    pub(super) fn last_insert(&self) -> Result<Option<usize>> {
        let Some(last) = self.last else { return Ok(None) };
        for i in 0..self.count {
            let slot = i.checked_mul(SLOT).and_then(|s| self.slots.checked_add(s)).ok_or_else(|| corrupt("a slot is past its end"))?;
            if u16_at(self.bytes, slot)? == last {
                return Ok(Some(i));
            }
        }
        Ok(None)
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
    pub(super) fn entries(&self) -> Result<Vec<(&'a [u8], &'a [u8])>> {
        (0..self.count).map(|i| self.entry(i)).collect()
    }

    /// Its shape, the fields besides its entries.
    pub(super) fn shape(&self) -> Shape<'a> {
        Shape { level: self.level, high: self.high, right: self.right, first: self.first }
    }
}

/// A node's fields besides its entries.
#[derive(Clone, Copy)]
pub(super) struct Shape<'a> {
    /// Its height above the leaves.
    pub(super) level: u8,
    /// Its high key; none for the rightmost node of a level.
    pub(super) high: Option<&'a [u8]>,
    /// Its right neighbour.
    pub(super) right: Option<PageId>,
    /// Its first child; `PageId(0)` in a leaf.
    pub(super) first: PageId,
}

/// The content of a node of `shape` holding `entries`, in key order.
///
/// # Errors
///
/// SQLCODE -916 if a key, a value or the node is longer than its fields hold.
pub(super) fn build(shape: &Shape<'_>, entries: &[(&[u8], &[u8])]) -> Result<Vec<u8>> {
    build_marked(shape, entries, None)
}

/// A leaf deleted: no entries, its link `outlink`, its right neighbour,
/// which took its range (`reclaim.rs`).
///
/// # Errors
///
/// As `build`.
pub(super) fn deleted(outlink: PageId) -> Result<Vec<u8>> {
    let mut content = build(&Shape { level: 0, high: None, right: Some(outlink), first: PageId(0) }, &[])?;
    if let Some(flags) = content.get_mut(FLAGS_AT) {
        *flags |= DELETED;
    }
    Ok(content)
}

/// Makes `right` the right neighbour of the node `content` holds, in place:
/// a node that has one.
///
/// # Errors
///
/// SQLCODE -916 if it is not a node.
pub(super) fn set_right(content: &mut [u8], right: PageId) -> Result<()> {
    Node::parse(content)?;
    let end = RIGHT_AT.checked_add(8).ok_or_else(|| corrupt("a field runs past its end"))?;
    content.get_mut(RIGHT_AT..end).ok_or_else(|| corrupt("a field runs past its end"))?.copy_from_slice(&right.0.to_le_bytes());
    Ok(())
}

/// Makes `child` the child of entry `at` of the inner node `content`
/// holds, in place.
///
/// # Errors
///
/// SQLCODE -916 if it is not a node, or has no entry `at`.
pub(super) fn set_child(content: &mut [u8], at: usize, child: PageId) -> Result<()> {
    let start = {
        let node = Node::parse(content)?;
        let (_, value) = node.entry(at)?;
        if value.len() != CHILD {
            return Err(corrupt("an inner entry's value is not a page number"));
        }
        node.value_at(at)?
    };
    let end = start.checked_add(CHILD).ok_or_else(|| corrupt("a field runs past its end"))?;
    content.get_mut(start..end).ok_or_else(|| corrupt("a field runs past its end"))?.copy_from_slice(&child.0.to_le_bytes());
    Ok(())
}

/// As `build`, entry `last` marked as the one inserted last.
///
/// # Errors
///
/// As `build`.
fn build_marked(shape: &Shape<'_>, entries: &[(&[u8], &[u8])], last: Option<usize>) -> Result<Vec<u8>> {
    let high = shape.high.unwrap_or_default();
    let size = entries.iter().fold(HEADER.saturating_add(high.len()), |n, (k, v)| n.saturating_add(entry_size(k, v)));
    let mut out = Vec::with_capacity(size);
    out.push(shape.level);
    out.push(if shape.high.is_some() { HAS_HIGH } else { 0 });
    out.extend_from_slice(&u16_of(entries.len())?);
    out.extend_from_slice(&u16_of(0)?);
    out.extend_from_slice(&u16_of(high.len())?);
    out.extend_from_slice(&shape.right.map_or(NO_PAGE, |r| r.0).to_le_bytes());
    out.extend_from_slice(&shape.first.0.to_le_bytes());
    out.extend_from_slice(&u16_of(NO_LAST)?);
    out.extend_from_slice(high);
    let heap = out.len();
    let mut slots = Vec::with_capacity(entries.len().saturating_mul(SLOT));
    for (i, (key, value)) in entries.iter().enumerate() {
        let offset = out.len().saturating_sub(heap);
        if last == Some(i) {
            set_u16(&mut out, LAST_AT, offset)?;
        }
        slots.extend_from_slice(&u16_of(offset)?);
        put_entry(&mut out, key, value)?;
    }
    out.extend_from_slice(&slots);
    Ok(out)
}

fn put_entry(out: &mut Vec<u8>, key: &[u8], value: &[u8]) -> Result<()> {
    out.extend_from_slice(&u16_of(key.len())?);
    out.extend_from_slice(key);
    out.extend_from_slice(&u16_of(value.len())?);
    out.extend_from_slice(value);
    Ok(())
}

/// Inserts `(key, value)` as entry `at` of the node `content` holds, in
/// place: the entry appended to the heap, its slot put in key order, and
/// marked as the entry inserted last.
///
/// # Errors
///
/// SQLCODE -916 if `content` is not a node, `at` is past its entries, or
/// the node grows past the fields' 65,535 bytes.
pub(super) fn insert(content: &mut Vec<u8>, at: usize, key: &[u8], value: &[u8]) -> Result<()> {
    let (heap, slots, count) = {
        let node = Node::parse(content)?;
        (node.heap, node.slots, node.count)
    };
    if at > count {
        return Err(corrupt("an entry is inserted past its end"));
    }
    let mut entry = Vec::with_capacity(entry_size(key, value));
    put_entry(&mut entry, key, value)?;
    let offset = u16_of(slots.saturating_sub(heap))?;
    let slot_at = slots.saturating_add(entry.len()).saturating_add(at.saturating_mul(SLOT));
    content.splice(slots..slots, entry);
    content.splice(slot_at..slot_at, offset);
    set_u16(content, LAST_AT, slots.saturating_sub(heap))?;
    set_u16(content, COUNT_AT, count.saturating_add(1))
}

/// Removes entry `at` of the node `content` holds, in place: its slot
/// removed, its bytes left in the heap as garbage until the node is built
/// again.
///
/// # Errors
///
/// SQLCODE -916 if `content` is not a node or has no entry `at`.
pub(super) fn remove(content: &mut Vec<u8>, at: usize) -> Result<()> {
    let (slot, garbage, count) = {
        let node = Node::parse(content)?;
        let (key, value) = node.entry(at)?;
        let freed = entry_size(key, value).saturating_sub(SLOT);
        (node.slots.saturating_add(at.saturating_mul(SLOT)), node.garbage.saturating_add(freed), node.count)
    };
    content.drain(slot..slot.saturating_add(SLOT));
    set_u16(content, COUNT_AT, count.saturating_sub(1))?;
    set_u16(content, GARBAGE_AT, garbage)
}

/// The node `content` holds, built again without its garbage.
///
/// # Errors
///
/// As `build`.
pub(super) fn compact(content: &[u8]) -> Result<Vec<u8>> {
    let node = Node::parse(content)?;
    build_marked(&node.shape(), &node.entries()?, node.last_insert()?)
}

/// The halves of a split node (§4): the left half, rewritten in its page,
/// and the right half, in a new page to its right; and the separator, the
/// left half's high key, which goes to the parent with the right half's
/// page.
pub(super) struct Halves {
    pub(super) left: Vec<u8>,
    pub(super) right: Vec<u8>,
    pub(super) separator: Vec<u8>,
}

/// Splits a node of `shape` that would hold `entries`, in key order, too
/// many for its page; the right half goes to page `new`. Halves are even
/// by bytes, unless inserts run in key order through the node and `run`
/// is the index of the entry added: then the entries before it stay, and
/// it starts the right half, marked as inserted last there, so the run
/// goes on (see the module's documentation).
///
/// # Errors
///
/// SQLCODE -916 if there are too few entries to split, or as `build`.
pub(super) fn split(shape: &Shape<'_>, entries: &[(&[u8], &[u8])], run: Option<usize>, new: PageId) -> Result<Halves> {
    let leaf = shape.level == 0;
    // A leaf keeps the separator, its highest key; an inner node gives it
    // up, its child becoming the right half's first.
    let most = entries.len().checked_sub(if leaf { 2 } else { 1 }).ok_or_else(|| corrupt("a node too full has too few entries"))?;
    // A run starts the right half at the entry added: in a leaf, the one
    // before it is the separator; in an inner node it goes up.
    let at_run = run.and_then(|at| at.checked_sub(1)).filter(|&before| before <= most);
    let middle = at_run.unwrap_or_else(|| balance(entries).min(most));
    let (last, value) = *entries.get(middle).ok_or_else(|| corrupt("a split is past its entries"))?;
    // A leaf's high key need only part its last key from the right half's
    // first: the shortest that does, so the levels above hold short keys.
    let separator = match (leaf, entries.get(middle.saturating_add(1))) {
        (true, Some((first, _))) => shortest_between(last, first),
        (true, None) | (false, _) => last,
    };
    let (left_end, right_first) = if leaf { (middle.saturating_add(1), PageId(0)) } else { (middle, child_of(value)?) };
    let left_entries = entries.get(..left_end).unwrap_or_default();
    let right_entries = entries.get(middle.saturating_add(1)..).unwrap_or_default();
    let left = build(&Shape { high: Some(separator), right: Some(new), ..*shape }, left_entries)?;
    let right = build_marked(&Shape { first: right_first, ..*shape }, right_entries, at_run.map(|_| 0))?;
    Ok(Halves { left, right, separator: separator.to_vec() })
}

/// \[DESIGN\] Suffix truncation (Bayer and Unterauer, "Prefix B-Trees",
/// ACM TODS 2(1), 1977): the shortest key at or above `last` and below
/// `first`, which is above it: a proper prefix of `first` one byte past
/// their common prefix, if `first` is longer; `last` itself if it is a
/// prefix of `first`, or no proper prefix of `first` lies between them.
pub(super) fn shortest_between<'k>(last: &'k [u8], first: &'k [u8]) -> &'k [u8] {
    let common = last.iter().zip(first).take_while(|(a, b)| a == b).count();
    if common == last.len() {
        return last;
    }
    let prefix = first.get(..common.saturating_add(1)).unwrap_or(last);
    if prefix.len() < first.len() { prefix } else { last }
}

/// The entry at which `entries` are half their bytes.
fn balance(entries: &[(&[u8], &[u8])]) -> usize {
    let total = entries.iter().fold(0_usize, |n, (k, v)| n.saturating_add(entry_size(k, v)));
    let half = total.checked_div(2).unwrap_or_default();
    let mut sum = 0_usize;
    for (i, (key, value)) in entries.iter().enumerate() {
        sum = sum.saturating_add(entry_size(key, value));
        if sum >= half {
            return i;
        }
    }
    entries.len().saturating_sub(1)
}

// Unit tests: nodes read back as built, searched in key order, changed in
// place, and split as `split` says (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;

//! \[DESIGN\] Nodes built and changed in place (RULES.md D31, D500): the
//! header, high key, heap and slots of `node.rs`, the prefix kept once,
//! each slot's head beside it, and the hints sampled from the heads again
//! after every change, so a search never reads a hint of another count.

use super::key::{self, KeyRef, head_of};
use super::{
    COUNT_AT, DELETED, FLAGS_AT, GARBAGE_AT, HAS_HIGH, HEADER, HINTS, HINTS_AT, LAST_AT, LEN, NO_LAST, NO_PAGE, Node, RIGHT_AT, SLOT,
    corrupt, hint_step, slice,
};
use crate::disk::pager::PageId;
use crate::error::Result;

/// A node's fields besides its entries.
#[derive(Clone, Copy)]
pub(in crate::engine::tree) struct Shape<'a> {
    /// Its height above the leaves.
    pub(in crate::engine::tree) level: u8,
    /// Its high key; none for the rightmost node of a level.
    pub(in crate::engine::tree) high: Option<&'a [u8]>,
    /// Its right neighbour.
    pub(in crate::engine::tree) right: Option<PageId>,
    /// Its first child; `PageId(0)` in a leaf.
    pub(in crate::engine::tree) first: PageId,
    /// The meta page of its tree.
    pub(in crate::engine::tree) tree: u64,
}

/// `value` as a u16 field.
fn u16_of(value: usize) -> Result<[u8; 2]> {
    u16::try_from(value).map(u16::to_le_bytes).map_err(|_| corrupt("a length or offset is past 65,535"))
}

fn set_u16(bytes: &mut [u8], at: usize, value: usize) -> Result<()> {
    let field = u16_of(value)?;
    let end = at.checked_add(LEN).ok_or_else(|| corrupt("a field runs past its end"))?;
    bytes.get_mut(at..end).ok_or_else(|| corrupt("a field runs past its end"))?.copy_from_slice(&field);
    Ok(())
}

/// The content of a node of `shape` holding `entries`, whole keys in key
/// order.
///
/// # Errors
///
/// SQLCODE -916 if a key, a value or the node is longer than its fields hold.
pub(in crate::engine::tree) fn build(shape: &Shape<'_>, entries: &[(&[u8], &[u8])]) -> Result<Vec<u8>> {
    let keyed: Vec<(KeyRef<'_>, &[u8])> = entries.iter().map(|&(k, v)| (KeyRef::whole(k), v)).collect();
    built_in(0, shape, &keyed, None, None)
}

/// \[DESIGN\] The most room a node is built with: the largest page, past
/// which no node's fields reach.
const MOST_ROOM: usize = 65_536;

/// The prefix a node holding `entries` keeps: what its first key and its
/// last share, which every key between them shares.
fn prefix_of(entries: &[(KeyRef<'_>, &[u8])]) -> usize {
    let (Some((first, _)), Some((last, _))) = (entries.first(), entries.last()) else { return 0 };
    first.common_with(last)
}

/// The bytes a node of `entries` and a high key `high` bytes long takes,
/// built with its own prefix (`prefix_of`).
pub(in crate::engine::tree) fn built_size(high: usize, entries: &[(KeyRef<'_>, &[u8])]) -> usize {
    let prefix = prefix_of(entries);
    entries.iter().fold(HEADER.saturating_add(high).saturating_add(prefix), |n, (k, v)| {
        n.saturating_add(super::entry_size(&[], v)).saturating_add(k.len().saturating_sub(prefix))
    })
}

/// As `build`, of keys as they lie in other nodes, entry `last` marked as
/// the one inserted last, keeping the first `prefix` bytes its keys share
/// if given, else its own (`prefix_of`); in a buffer of at least `room`
/// bytes, its page's, up to the largest page (RULES.md D864). A node
/// built to its own size was grown by the first entry inserted into it,
/// its whole content moved to a new buffer; a node its tree writes is
/// built with its page's room, and takes entries where it is.
///
/// # Errors
///
/// As `build`.
pub(in crate::engine::tree) fn built_in(
    room: usize,
    shape: &Shape<'_>,
    entries: &[(KeyRef<'_>, &[u8])],
    last: Option<usize>,
    prefix: Option<usize>,
) -> Result<Vec<u8>> {
    let high = shape.high.unwrap_or_default();
    let shared = entries.first().map(|(k, _)| k.to_vec()).unwrap_or_default();
    let prefix = shared.get(..prefix.unwrap_or_else(|| prefix_of(entries)).min(shared.len())).unwrap_or_default();
    let size = entries.iter().fold(HEADER.saturating_add(high.len()).saturating_add(prefix.len()), |n, (k, v)| {
        n.saturating_add(super::entry_size(&[], v)).saturating_add(k.len().saturating_sub(prefix.len()))
    });
    let mut out = Vec::with_capacity(size.max(room.min(MOST_ROOM)));
    out.push(shape.level);
    out.push(if shape.high.is_some() { HAS_HIGH } else { 0 });
    out.extend_from_slice(&u16_of(entries.len())?);
    out.extend_from_slice(&u16_of(0)?);
    out.extend_from_slice(&u16_of(high.len())?);
    out.extend_from_slice(&shape.right.map_or(NO_PAGE, |r| r.0).to_le_bytes());
    out.extend_from_slice(&shape.first.0.to_le_bytes());
    out.extend_from_slice(&u16_of(last.unwrap_or(NO_LAST))?);
    out.extend_from_slice(&u16_of(prefix.len())?);
    out.extend_from_slice(&shape.tree.to_le_bytes());
    out.resize(HEADER, 0);
    out.extend_from_slice(high);
    out.extend_from_slice(prefix);
    let heap = out.len();
    let mut slots = Vec::with_capacity(entries.len().saturating_mul(SLOT));
    for (key, value) in entries {
        let (a, b) = key.from(prefix.len());
        slots.extend_from_slice(&u16_of(out.len().saturating_sub(heap))?);
        slots.extend_from_slice(&head_of_pieces(a, b).to_be_bytes());
        out.extend_from_slice(&u16_of(a.len().saturating_add(b.len()))?);
        out.extend_from_slice(a);
        out.extend_from_slice(b);
        out.extend_from_slice(&u16_of(value.len())?);
        out.extend_from_slice(value);
    }
    out.extend_from_slice(&slots);
    hint(&mut out)?;
    Ok(out)
}

/// The head of the tail `a` then `b`.
fn head_of_pieces(a: &[u8], b: &[u8]) -> u16 {
    let mut first = [0_u8; key::HEAD];
    for (to, from) in first.iter_mut().zip(a.iter().chain(b)) {
        *to = *from;
    }
    u16::from_be_bytes(first)
}

/// Samples the hints again after entry `at` was inserted into, or removed
/// from, the node `content` holds, which had `before` entries and has
/// `after`, if a hint may have changed (RULES.md D864). The hints are the
/// heads of the slots at sixteen even steps, so they stay as they are
/// while the step is the same and the slot changed lies past the last
/// one sampled, as an entry appended to a node, a table's next row, does
/// sixteen times in seventeen. The bytes are those sampling again gives.
fn hint_after(content: &mut [u8], (before, after): (usize, usize), at: usize) -> Result<()> {
    let step = hint_step(after);
    if hint_step(before) == step && step.is_none_or(|step| at > step.saturating_mul(HINTS)) {
        return Ok(());
    }
    hint(content)
}

/// Samples the hints of the node `content` holds from its heads.
pub(super) fn hint(content: &mut [u8]) -> Result<()> {
    let mut hints = [0_u8; HINTS * key::HEAD];
    {
        let node = Node::parse(content)?;
        if let Some(step) = hint_step(node.len()) {
            for (i, to) in hints.as_chunks_mut::<{ key::HEAD }>().0.iter_mut().enumerate() {
                *to = node.head(step.saturating_mul(i.saturating_add(1)))?.to_be_bytes();
            }
        }
    }
    let end = HINTS_AT.saturating_add(hints.len());
    content.get_mut(HINTS_AT..end).ok_or_else(|| corrupt("its header is short"))?.copy_from_slice(&hints);
    Ok(())
}

/// A leaf of tree `tree` deleted: no entries, its link `outlink`, its right
/// neighbour, which took its range (`reclaim.rs`).
///
/// # Errors
///
/// As `build`.
pub(in crate::engine::tree) fn deleted(outlink: PageId, tree: u64) -> Result<Vec<u8>> {
    // Built with no link, as a node with a link and no high key is not
    // one until it is marked deleted.
    let mut content = build(&Shape { level: 0, high: None, right: None, first: PageId(0), tree }, &[])?;
    if let Some(flags) = content.get_mut(FLAGS_AT) {
        *flags |= DELETED;
    }
    set_right(&mut content, outlink)?;
    Ok(content)
}

/// Makes `right` the right neighbour of the node `content` holds, in place:
/// a node that has one.
///
/// # Errors
///
/// SQLCODE -916 if it is not a node.
pub(in crate::engine::tree) fn set_right(content: &mut [u8], right: PageId) -> Result<()> {
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
pub(in crate::engine::tree) fn set_child(content: &mut [u8], at: usize, child: PageId) -> Result<()> {
    let start = {
        let node = Node::parse(content)?;
        if node.value(at)?.len() != super::CHILD {
            return Err(corrupt("an inner entry's value is not a page number"));
        }
        node.value_at(at)?
    };
    let end = start.checked_add(super::CHILD).ok_or_else(|| corrupt("a field runs past its end"))?;
    content.get_mut(start..end).ok_or_else(|| corrupt("a field runs past its end"))?.copy_from_slice(&child.0.to_le_bytes());
    Ok(())
}

/// Writes `value` over the value of entry `at`, no longer than it, in
/// place: no byte moves, and the node keeps its last insert. The bytes a
/// shorter value leaves past its end are garbage, counted as a removed
/// entry's are, until the node is built again.
///
/// # Errors
///
/// SQLCODE -916 if it is not a node, has no entry `at`, or its value is
/// shorter than `value`.
pub(in crate::engine::tree) fn overwrite(content: &mut [u8], at: usize, value: &[u8]) -> Result<()> {
    let (start, garbage) = {
        let node = Node::parse(content)?;
        let left = node.value(at)?.len().checked_sub(value.len()).ok_or_else(|| corrupt("a value written over is longer than it"))?;
        (node.value_at(at)?, node.garbage.saturating_add(left))
    };
    let end = start.checked_add(value.len()).ok_or_else(|| corrupt("a field runs past its end"))?;
    content.get_mut(start..end).ok_or_else(|| corrupt("a field runs past its end"))?.copy_from_slice(value);
    set_u16(content, start.checked_sub(LEN).ok_or_else(|| corrupt("a field runs past its end"))?, value.len())?;
    set_u16(content, GARBAGE_AT, garbage)
}

/// The value of entry `at`, to change in place, keeping its length.
///
/// # Errors
///
/// SQLCODE -916 if it is not a node or has no entry `at`.
pub(in crate::engine::tree) fn value_mut(content: &mut [u8], at: usize) -> Result<&mut [u8]> {
    let (start, len) = {
        let node = Node::parse(content)?;
        (node.value_at(at)?, node.value(at)?.len())
    };
    let end = start.checked_add(len).ok_or_else(|| corrupt("a field runs past its end"))?;
    content.get_mut(start..end).ok_or_else(|| corrupt("a field runs past its end"))
}

/// The bytes the node `node` grows by, besides the entry, as `key` is
/// inserted: its keys' tails longer by what its prefix loses, if `key`
/// does not begin with it.
pub(in crate::engine::tree) fn grown(node: &Node<'_>, key: &[u8]) -> usize {
    let kept = key::common(node.prefix, key);
    node.len().saturating_mul(node.prefix.len().saturating_sub(kept))
}

/// The bytes an entry of `key` and `value` takes in the node `node`, its
/// slot included: its key less what it shares of the node's prefix, which
/// a key without all of it shortens (`grown`).
pub(in crate::engine::tree) fn stored(node: &Node<'_>, key: &[u8], value: &[u8]) -> usize {
    super::entry_size(key, value).saturating_sub(key::common(node.prefix, key))
}

/// Whether the node `node`, built again with `key` added, would keep a
/// longer prefix than it keeps now: the least of what its first key, its
/// last and `key` share two by two is what the lowest and highest share.
///
/// # Errors
///
/// SQLCODE -916 if the node's slots are malformed.
pub(in crate::engine::tree) fn lengthens(node: &Node<'_>, key: &[u8]) -> Result<bool> {
    let Some(end) = node.len().checked_sub(1) else { return Ok(false) };
    let (first, last, added) = (node.key(0)?, node.key(end)?, KeyRef::whole(key));
    let shared = first.common_with(&last).min(added.common_with(&first)).min(added.common_with(&last));
    Ok(shared > node.prefix.len())
}

/// Inserts `(key, value)` as entry `at` of the node `content` holds, in
/// place: the entry appended to the heap, its slot put in key order, and
/// marked as the entry inserted last. A key without the node's prefix
/// first makes the node be built again with the prefix they share.
///
/// [DESIGN] RULES.md D847: the node is parsed once unless built again,
/// and grown once, the slots moved over the room made, the entry and
/// its slot written into the gaps left, with nothing put together apart.
///
/// # Errors
///
/// SQLCODE -916 if `content` is not a node, `at` is past its entries, or
/// the node grows past the fields' 65,535 bytes.
pub(in crate::engine::tree) fn insert(content: &mut Vec<u8>, at: usize, key: &[u8], value: &[u8]) -> Result<()> {
    let fields = |node: &Node<'_>| (node.heap, node.slots, node.count, node.prefix.len());
    let parsed = {
        let node = Node::parse(content)?;
        let kept = key::common(node.prefix, key);
        if kept < node.prefix.len() { Err(rebuilt(&node, kept, content.capacity())?) } else { Ok(fields(&node)) }
    };
    let (heap, slots, count, prefix) = match parsed {
        Ok(found) => found,
        Err(rebuilt) => {
            *content = rebuilt;
            fields(&Node::parse(content)?)
        }
    };
    let tail = key.get(prefix..).unwrap_or_default();
    let (tail_len, value_len, offset) = (u16_of(tail.len())?, u16_of(value.len())?, u16_of(slots.saturating_sub(heap))?);
    let entry = LEN.saturating_mul(2).saturating_add(tail.len()).saturating_add(value.len());
    let end = content.len();
    let slot_from = at.checked_mul(SLOT).and_then(|s| slots.checked_add(s)).filter(|&s| at <= count && s <= end);
    let slot_from = slot_from.ok_or_else(|| corrupt("an entry is inserted past its end"))?;
    content.resize(end.saturating_add(entry).saturating_add(SLOT), 0);
    // The slots from `at` on go past the entry and the new slot, those
    // before it past the entry alone.
    content.copy_within(slot_from..end, slot_from.saturating_add(entry).saturating_add(SLOT));
    content.copy_within(slots..slot_from, slots.saturating_add(entry));
    let written = put(content, slots, &[&tail_len, tail, &value_len, value])?;
    put(content, written.saturating_add(slot_from.saturating_sub(slots)), &[&offset, &head_of(tail).to_be_bytes()])?;
    set_u16(content, LAST_AT, at)?;
    set_u16(content, COUNT_AT, count.saturating_add(1))?;
    hint_after(content, (count, count.saturating_add(1)), at)
}

/// `node` built again keeping the first `kept` bytes of its prefix, in a
/// buffer of at least `room` bytes.
fn rebuilt(node: &Node<'_>, kept: usize, room: usize) -> Result<Vec<u8>> {
    built_in(room, &node.shape(), &node.entries()?, node.last, Some(kept))
}

/// Writes `value`, longer than the value of entry `at` of the node
/// `content` holds, in its place (RULES.md D864): the entry written again
/// at the end of the heap with its key's tail, the slots moved past it
/// once, and its slot led to it. The old entry's bytes are garbage, and
/// the entry is marked as the one inserted last, as an entry removed and
/// inserted again leaves them. No slot is taken out or put in and no head
/// changes, so the hints stay.
///
/// # Errors
///
/// SQLCODE -916 if `content` is not a node, has no entry `at`, or grows
/// past the fields' 65,535 bytes.
pub(in crate::engine::tree) fn lengthen(content: &mut Vec<u8>, at: usize, value: &[u8]) -> Result<()> {
    let (heap, slots, tail, garbage, slot) = {
        let node = Node::parse(content)?;
        let (tail, value_at) = node.tail_at(at)?;
        let old = slice(node.front, value_at.saturating_add(LEN), super::u16_at(node.front, value_at)?)?;
        let tail_from = value_at.saturating_sub(tail.len());
        let freed = tail.len().saturating_add(old.len()).saturating_add(LEN.saturating_mul(2));
        (node.heap, node.slots, (tail_from, tail.len()), node.garbage.saturating_add(freed), node.slot(at)?)
    };
    let (tail_len, value_len, offset) = (u16_of(tail.1)?, u16_of(value.len())?, u16_of(slots.saturating_sub(heap))?);
    let entry = LEN.saturating_mul(2).saturating_add(tail.1).saturating_add(value.len());
    let end = content.len();
    content.resize(end.saturating_add(entry), 0);
    content.copy_within(slots..end, slots.saturating_add(entry));
    let tail_to = put(content, slots, &[&tail_len])?;
    content.copy_within(tail.0..tail.0.saturating_add(tail.1), tail_to);
    put(content, tail_to.saturating_add(tail.1), &[&value_len, value])?;
    put(content, slot.saturating_add(entry), &[&offset])?;
    set_u16(content, LAST_AT, at)?;
    set_u16(content, GARBAGE_AT, garbage)
}

/// Writes `parts` one after another into `bytes` from `at`: where they end.
fn put(bytes: &mut [u8], at: usize, parts: &[&[u8]]) -> Result<usize> {
    parts.iter().try_fold(at, |from, part| {
        let to = from.checked_add(part.len()).ok_or_else(|| corrupt("a field runs past its end"))?;
        bytes.get_mut(from..to).ok_or_else(|| corrupt("a field runs past its end"))?.copy_from_slice(part);
        Ok(to)
    })
}

/// Removes entry `at` of the node `content` holds, in place: its slot
/// removed, its bytes left in the heap as garbage until the node is built
/// again.
///
/// # Errors
///
/// SQLCODE -916 if `content` is not a node or has no entry `at`.
pub(in crate::engine::tree) fn remove(content: &mut Vec<u8>, at: usize) -> Result<()> {
    let (slot, garbage, count, last) = {
        let node = Node::parse(content)?;
        let (key, value) = node.entry(at)?;
        let tail = key.len().saturating_sub(node.prefix.len());
        let freed = tail.saturating_add(value.len()).saturating_add(LEN.saturating_mul(2));
        (node.slot(at)?, node.garbage.saturating_add(freed), node.count, node.last)
    };
    content.drain(slot..slot.saturating_add(SLOT));
    // The mark follows its entry down a slot, or goes with it.
    let last = last.filter(|&l| l != at).map_or(NO_LAST, |l| if l > at { l.saturating_sub(1) } else { l });
    set_u16(content, LAST_AT, last)?;
    set_u16(content, COUNT_AT, count.saturating_sub(1))?;
    set_u16(content, GARBAGE_AT, garbage)?;
    hint_after(content, (count, count.saturating_sub(1)), at)
}

/// The node `content` holds, built again without its garbage, in a buffer
/// of at least `room` bytes (`built_in`).
///
/// # Errors
///
/// As `build`.
pub(in crate::engine::tree) fn compact_in(content: &[u8], room: usize) -> Result<Vec<u8>> {
    let node = Node::parse(content)?;
    built_in(room, &node.shape(), &node.entries()?, node.last_insert()?, None)
}

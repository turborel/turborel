//! \[DESIGN\] Nodes built and changed in place (RULES.md D31, D500): the
//! header, high key, heap and slots of `node.rs`, the prefix kept once,
//! each slot's head beside it, and the hints sampled from the heads again
//! after every change, so a search never reads a hint of another count.

use super::key::{self, KeyRef, head_of};
use super::{
    COUNT_AT, DELETED, FLAGS_AT, GARBAGE_AT, HAS_HIGH, HEADER, HINTS, HINTS_AT, LAST_AT, LEN, NO_LAST, NO_PAGE, Node, RIGHT_AT, SLOT,
    corrupt, hint_step,
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
    built(shape, &keyed, None, None)
}

/// The prefix a node holding `entries` keeps: what its first key and its
/// last share, which every key between them shares.
fn prefix_of(entries: &[(KeyRef<'_>, &[u8])]) -> usize {
    let (Some((first, _)), Some((last, _))) = (entries.first(), entries.last()) else { return 0 };
    first.common_with(last)
}

/// As `build`, of keys as they lie in other nodes, entry `last` marked as
/// the one inserted last, keeping the first `prefix` bytes its keys share
/// if given, else its own (`prefix_of`).
///
/// # Errors
///
/// As `build`.
pub(super) fn built(shape: &Shape<'_>, entries: &[(KeyRef<'_>, &[u8])], last: Option<usize>, prefix: Option<usize>) -> Result<Vec<u8>> {
    let high = shape.high.unwrap_or_default();
    let shared = entries.first().map(|(k, _)| k.to_vec()).unwrap_or_default();
    let prefix = shared.get(..prefix.unwrap_or_else(|| prefix_of(entries)).min(shared.len())).unwrap_or_default();
    let size = entries.iter().fold(HEADER.saturating_add(high.len()).saturating_add(prefix.len()), |n, (k, v)| {
        n.saturating_add(super::entry_size(&[], v)).saturating_add(k.len().saturating_sub(prefix.len()))
    });
    let mut out = Vec::with_capacity(size);
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

/// Samples the hints of the node `content` holds from its heads.
fn hint(content: &mut [u8]) -> Result<()> {
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

/// Writes `value` over the value of entry `at`, of its length, in place:
/// no byte moves, and the node keeps its garbage and its last insert.
///
/// # Errors
///
/// SQLCODE -916 if it is not a node, has no entry `at`, or its value is
/// of another length.
pub(in crate::engine::tree) fn overwrite(content: &mut [u8], at: usize, value: &[u8]) -> Result<()> {
    let start = {
        let node = Node::parse(content)?;
        if node.value(at)?.len() != value.len() {
            return Err(corrupt("a value written over is of another length"));
        }
        node.value_at(at)?
    };
    let end = start.checked_add(value.len()).ok_or_else(|| corrupt("a field runs past its end"))?;
    content.get_mut(start..end).ok_or_else(|| corrupt("a field runs past its end"))?.copy_from_slice(value);
    Ok(())
}

/// The bytes the node `node` grows by, besides the entry, as `key` is
/// inserted: its keys' tails longer by what its prefix loses, if `key`
/// does not begin with it.
pub(in crate::engine::tree) fn grown(node: &Node<'_>, key: &[u8]) -> usize {
    let kept = key::common(node.prefix, key);
    node.len().saturating_mul(node.prefix.len().saturating_sub(kept))
}

/// Inserts `(key, value)` as entry `at` of the node `content` holds, in
/// place: the entry appended to the heap, its slot put in key order, and
/// marked as the entry inserted last. A key without the node's prefix
/// first makes the node be built again with the prefix they share.
///
/// # Errors
///
/// SQLCODE -916 if `content` is not a node, `at` is past its entries, or
/// the node grows past the fields' 65,535 bytes.
pub(in crate::engine::tree) fn insert(content: &mut Vec<u8>, at: usize, key: &[u8], value: &[u8]) -> Result<()> {
    let shorter = {
        let node = Node::parse(content)?;
        let kept = key::common(node.prefix, key);
        (kept < node.prefix.len()).then(|| node.entries().and_then(|e| built(&node.shape(), &e, node.last, Some(kept)))).transpose()?
    };
    if let Some(rebuilt) = shorter {
        *content = rebuilt;
    }
    let (heap, slots, count, prefix) = {
        let node = Node::parse(content)?;
        (node.heap, node.slots, node.count, node.prefix.len())
    };
    if at > count {
        return Err(corrupt("an entry is inserted past its end"));
    }
    let tail = key.get(prefix..).unwrap_or_default();
    let mut entry = Vec::with_capacity(tail.len().saturating_add(value.len()).saturating_add(LEN.saturating_mul(2)));
    entry.extend_from_slice(&u16_of(tail.len())?);
    entry.extend_from_slice(tail);
    entry.extend_from_slice(&u16_of(value.len())?);
    entry.extend_from_slice(value);
    let mut slot = u16_of(slots.saturating_sub(heap))?.to_vec();
    slot.extend_from_slice(&head_of(tail).to_be_bytes());
    let slot_at = slots.saturating_add(entry.len()).saturating_add(at.saturating_mul(SLOT));
    content.splice(slots..slots, entry);
    content.splice(slot_at..slot_at, slot);
    set_u16(content, LAST_AT, at)?;
    set_u16(content, COUNT_AT, count.saturating_add(1))?;
    hint(content)
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
    hint(content)
}

/// The node `content` holds, built again without its garbage.
///
/// # Errors
///
/// As `build`.
pub(in crate::engine::tree) fn compact(content: &[u8]) -> Result<Vec<u8>> {
    let node = Node::parse(content)?;
    built(&node.shape(), &node.entries()?, node.last_insert()?, None)
}

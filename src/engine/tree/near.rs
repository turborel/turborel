//! \[DESIGN\] Reads by key that start where the last one ended (RULES.md
//! D31). The rows of a range sit side by side, so a scan through an index
//! asks for key after key of one leaf, and a descent from the root for each
//! would latch the root and the inner nodes, which every thread shares,
//! for an answer the leaf already holds.
//!
//! The leaf is trusted only if it cannot have left the tree. A leaf leaves
//! by being retired (`reclaim.rs`), which moves the tree's stamp first. A
//! read enters its epoch, then loads the stamp: unchanged, nothing retired
//! before that load has been freed behind it, and whatever is retired after
//! it stays unfreed until the read leaves its epoch. A changed stamp costs
//! one descent.
//!
//! A leaf holds no keys below its lowest, so one in the tree whose range
//! covers a key at or above its first entry is the leaf for that key, split
//! or not, the key present or not. A key past the leaf's range is looked
//! for in its right neighbour, as a descent moves right (Lehman and Yao
//! 1981, §4): a link read from a leaf in the tree names no page freed, as
//! a leaf is unlinked before it is retired.
//!
//! A read of a table's row with no leaf to start from tries the leaf noted
//! for the row (`hints.rs`, RULES.md D500), the hint its index entry
//! carried. It is trusted as the remembered leaf is, but for the stamp,
//! as it may come from anywhere: only if a frame holds it now, so no
//! page is read that may be free, and it is a leaf of this tree, not
//! deleted, whose first key is at or below the key. A page reused by
//! another tree names that tree; one freed has left the pool, its frame
//! emptied (`BufferPool::free`); and a leaf of this tree not deleted holds
//! no key below its range. Past its high key, the key moved right with a
//! split, and the read follows the right links a few leaves, as a descent
//! would. Any other read descends.

use std::cmp::Ordering;

use super::hints;
use super::node::Node;
use super::{BLinkTree, Path, Target};
use crate::error::Result;
use crate::storage::Near;

/// \[DESIGN\] The misses in a row after which a scan stops trying its leaf:
/// its keys are not side by side, and each try costs a page read.
const MOST_MISSES: u8 = 8;
/// \[DESIGN\] The leaves a read tries before it descends: the one it knows
/// and its right neighbour, where a scan's next key goes when it leaves one.
const LEAVES_TRIED: usize = 2;
/// \[DESIGN\] The right links a read from a noted leaf follows before it
/// descends: splits since the note, each moving keys only right.
const HOPS: usize = 4;

impl BLinkTree {
    /// Whether `key` is in the tree; its value left in `near`, which is
    /// kept from read to read of one scan.
    ///
    /// # Errors
    ///
    /// As `get`.
    pub fn get_near(&self, key: &[u8], near: &mut Near) -> Result<bool> {
        let _guard = self.epochs.guard();
        let stamp = self.epochs.stamp();
        if let Some(found) = self.in_leaf(key, stamp, near)? {
            return Ok(found);
        }
        if let Some(found) = self.hinted(key, stamp, near)? {
            return Ok(found);
        }
        let (bytes, at) = (&mut near.bytes, &mut near.at);
        let mut path = Path { tallied: true, ..Path::default() };
        let (leaf, found) = self.descend(Target::Key(key), 0, &mut path, |leaf| copied(leaf, key, (bytes, at)))?;
        near.leaf = Some((leaf.0, stamp));
        Ok(found)
    }

    /// Whether `key` is in the tree, if the leaf `near` remembers answers,
    /// or the one right of it, for a key past its range. A leaf that
    /// answers twice in a row is copied into `near`, and later reads of it
    /// take the copy while its frame's version holds (`in_copy`).
    fn in_leaf(&self, key: &[u8], stamp: u64, near: &mut Near) -> Result<Option<bool>> {
        let Some((first, _)) = near.leaf.filter(|&(_, at)| at == stamp && near.misses < MOST_MISSES) else { return Ok(None) };
        if let Some(found) = self.in_copy(key, first, near)? {
            return Ok(Some(found));
        }
        let mut next = Some(first);
        for _ in 0..LEAVES_TRIED {
            let Some(leaf) = next else { break };
            let (page, version) = self.pool.read_stamped(self.file, leaf, Some(&self.tally))?;
            let node = Node::parse(&page)?;
            if answers(&node, key)? {
                self.tally.lookup();
                near.misses = 0;
                near.leaf = Some((leaf, stamp));
                if leaf == first {
                    near.page.clear();
                    near.page.extend_from_slice(&page);
                    near.kept = Some((leaf, version));
                    near.parsed = Some(node.parsed());
                }
                return copied(&node, key, (&mut near.bytes, &mut near.at)).map(Some);
            }
            next = node.right.filter(|_| node.level == 0 && !node.deleted && !node.covers(key)).map(|right| right.0);
        }
        near.misses = near.misses.saturating_add(1);
        Ok(None)
    }

    /// Whether `key` is in the tree, if `near` keeps a copy of leaf `leaf`
    /// that is still the leaf's content and answers for it. Such a read
    /// reads no page, takes no latch and writes no shared word (RULES.md
    /// D806). The copy is not parsed again, and a key at the slot after
    /// the last one found is taken with one comparison (D808).
    fn in_copy(&self, key: &[u8], leaf: u64, near: &mut Near) -> Result<Option<bool>> {
        let Some((_, version)) = near.kept.filter(|&(kept, _)| kept == leaf) else { return Ok(None) };
        if !self.pool.unchanged(self.file, leaf, version) {
            near.kept = None;
            return Ok(None);
        }
        let Near { page, bytes, at, misses, parsed, .. } = near;
        let node = match parsed.as_ref().and_then(|parsed| Node::parsed_again(page, parsed)) {
            Some(node) => node,
            None => Node::parse(page)?,
        };
        let next = at.and_then(|at| at.checked_add(1)).filter(|&next| next < node.len());
        let stepped = match next {
            Some(next) => node.key(next)?.is(key).then_some(next),
            None => None,
        };
        if stepped.is_none() && !answers(&node, key)? {
            return Ok(None);
        }
        self.tally.lookup();
        *misses = 0;
        let Some(slot) = stepped else { return copied(&node, key, (bytes, at)).map(Some) };
        bytes.clear();
        bytes.extend_from_slice(node.entry(slot)?.1);
        *at = Some(slot);
        Ok(Some(true))
    }
}

impl BLinkTree {
    /// Whether `key` is in the tree, if the leaf noted for its row answers,
    /// or one right of it within `HOPS` links.
    fn hinted(&self, key: &[u8], stamp: u64, near: &mut Near) -> Result<Option<bool>> {
        let Some(mut id) = self.row_of(key).and_then(hints::leaf_of) else { return Ok(None) };
        let Some(mut page) = self.pool.read_resident(self.file, id)? else { return Ok(None) };
        for hop in 0..=HOPS {
            let right = {
                // Of no use, rather than malformed: it may be any page.
                let Ok(node) = Node::parse(&page) else { return Ok(None) };
                if hop == 0 && !trusted(&node, self.meta.0, key)? {
                    return Ok(None);
                }
                if answers(&node, key)? {
                    self.tally.lookup();
                    self.counters.hinted.add();
                    near.misses = 0;
                    near.leaf = Some((id, stamp));
                    return copied(&node, key, (&mut near.bytes, &mut near.at)).map(Some);
                }
                node.right.filter(|_| node.level == 0 && (node.deleted || !node.covers(key)))
            };
            let Some(right) = right else { return Ok(None) };
            drop(page);
            super::count(&self.counters.right_moves);
            id = right.0;
            page = self.pool.read(self.file, id)?;
        }
        Ok(None)
    }
}

/// Whether `node`, a page noted, may be trusted as a leaf of the tree
/// whose meta page is `tree`, for `key`: one not deleted, its first key at
/// or below `key`.
pub(super) fn trusted(node: &Node<'_>, tree: u64, key: &[u8]) -> Result<bool> {
    if node.tree != tree || node.level != 0 || node.deleted || node.len() == 0 {
        return Ok(false);
    }
    Ok(node.key(0)?.cmp_bytes(key) != Ordering::Greater)
}

/// Whether leaf `node` is the one for `key`.
fn answers(node: &Node<'_>, key: &[u8]) -> Result<bool> {
    Ok(node.level == 0 && !node.deleted && node.len() > 0 && node.covers(key) && node.key(0)?.cmp_bytes(key) != Ordering::Greater)
}

/// Whether `leaf` holds `key`, its value copied to `bytes` and its place
/// left in `at`. The entry after the one at `at` is tried first, as a scan
/// through an index asks for the rows of a range in order; its key is
/// compared, so a place left by another leaf costs one comparison.
fn copied(leaf: &Node<'_>, key: &[u8], (bytes, at): (&mut Vec<u8>, &mut Option<usize>)) -> Result<bool> {
    bytes.clear();
    let next = at.and_then(|at| at.checked_add(1)).filter(|&next| next < leaf.len());
    let found = match next {
        Some(next) if leaf.key(next)?.is(key) => Ok(next),
        _ => leaf.search(key)?,
    };
    *at = found.ok();
    match found {
        Ok(found) => {
            bytes.extend_from_slice(leaf.entry(found)?.1);
            Ok(true)
        }
        Err(_) => Ok(false),
    }
}

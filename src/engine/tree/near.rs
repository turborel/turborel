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

use super::node::{self, Held, Node};
use super::{BLinkTree, Path, Target, Within, hints, leaves};
use crate::disk::buffer::Stamp;
use crate::error::Result;
use crate::storage::Near;

/// \[DESIGN\] The misses in a row after which a scan stops trying its leaf:
/// its keys are not side by side, and each try costs a page read. Only a
/// read the leaf remembered answers ends the row, so what keeps a `Near`
/// across reads of rows far apart stops trying for good.
const MOST_MISSES: u8 = 8;
/// \[DESIGN\] The leaves a read tries before it descends: the one it knows
/// and its right neighbour, where a scan's next key goes when it leaves one.
const LEAVES_TRIED: usize = 2;
/// \[DESIGN\] The right links a read from a noted leaf follows before it
/// descends: splits since the note, each moving keys only right.
const HOPS: usize = 4;

/// A reader shown a value once: what `get_near_with` was given, until one
/// of the places a read looks in has the key.
struct Reader<F>(Option<F>);

impl<F> Reader<F> {
    /// What the reader makes of `value`, shown where it lies.
    fn show<R>(&mut self, value: &[u8]) -> Result<Option<R>>
    where
        F: FnOnce(&[u8]) -> Result<R>,
    {
        self.0.take().map(|f| f(value)).transpose()
    }
}

/// What came of looking for a key in one of the places a read looks in.
enum Tried<R> {
    /// Nothing: the place does not answer for the key.
    Elsewhere,
    /// What the reader made of the key's value, none if the key is not in
    /// the tree.
    Answered(Option<R>),
}

impl BLinkTree {
    /// Whether `key` is in the tree; its value left in `near`, which is
    /// kept from read to read of one scan.
    ///
    /// # Errors
    ///
    /// As `get`.
    pub fn get_near(&self, key: &[u8], near: &mut Near) -> Result<bool> {
        let mut bytes = std::mem::take(&mut near.bytes);
        bytes.clear();
        let copy = |value: &[u8]| {
            bytes.extend_from_slice(value);
            Ok(())
        };
        let found = self.get_near_with(key, near, copy);
        near.bytes = bytes;
        Ok(found?.is_some())
    }

    /// \[DESIGN\] As `get_near`, the value of `key` shown to `f` where it
    /// lies, not copied (RULES.md D863): under its leaf's latch, or over a
    /// copy of the leaf, `near`'s or the thread's. What `f` makes of it,
    /// or none if `key` is not in the tree, `f` then not called. The leaf
    /// remembered, the leaf noted and the descent are tried in `get_near`'s
    /// order, and `near` is left for the next read as `get_near` leaves
    /// it, but for its bytes, which are not touched. `f` must not use the
    /// tree nor wait.
    ///
    /// # Errors
    ///
    /// `f`'s errors; as `get`.
    pub fn get_near_with<R>(&self, key: &[u8], near: &mut Near, f: impl FnOnce(&[u8]) -> Result<R>) -> Result<Option<R>> {
        let op = self.epochs.guard();
        // The stamp loaded in the epoch, with the proof of being in it.
        let (within, stamp) = (op.within(), self.epochs.stamp());
        let mut reader = Reader(Some(f));
        if let Tried::Answered(found) = self.in_leaf(key, (within, stamp), near, &mut reader)? {
            return Ok(found);
        }
        if let Tried::Answered(found) = self.hinted(key, (within, stamp), near, &mut reader)? {
            return Ok(found);
        }
        if let Tried::Answered(found) = self.down_copies(key, (within, stamp), near, &mut reader)? {
            return Ok(found);
        }
        let at = &mut near.at;
        let mut path = Path { tallied: true, ..Path::default() };
        let (leaf, found) = self.descend((within, Target::Key(key)), 0, &mut path, |leaf| located(leaf, key, at, &mut reader))?;
        near.leaf = Some((leaf.0, stamp));
        Ok(found)
    }

    /// \[DESIGN\] (RULES.md D955) What the reader makes of `key`'s value,
    /// none if it is not in the tree, if the thread's copies of the inner
    /// nodes lead to `key`'s leaf (`inner.rs`) and the leaf, read with no
    /// node made of it (`node/lean.rs`), is one of the tree, in it, whose
    /// range ends at or after `key`. The copies say its range begins
    /// before `key`, so a key it does not hold is not in the tree.
    /// Elsewhere if any of that fails; the caller then descends as before. `near`
    /// is left as a descent leaves it, and the lookup is tallied as one.
    fn down_copies<R, F>(&self, key: &[u8], (within, stamp): (Within<'_>, u64), near: &mut Near, reader: &mut Reader<F>) -> Result<Tried<R>>
    where
        F: FnOnce(&[u8]) -> Result<R>,
    {
        let Some(leaf) = self.leaf_by_copies(within, key) else { return Ok(Tried::Elsewhere) };
        let page = leaves::read(&self.pool, (self.file, leaf), Some(&self.tally))?;
        let found = match node::held(&page, self.meta.0, key, None) {
            Held::At(slot, value) => {
                near.at = Some(slot);
                reader.show(value)?
            }
            Held::Absent => {
                near.at = None;
                None
            }
            Held::Unknown => return Ok(Tried::Elsewhere),
        };
        self.tally.lookup();
        near.leaf = Some((leaf, stamp));
        Ok(Tried::Answered(found))
    }

    /// What the reader makes of `key`'s value, none if it is not in the
    /// tree, if the leaf `near` remembers answers, or the one right of it,
    /// for a key past its range. A leaf that answers twice in a row is
    /// copied into `near`, and later reads of it take the copy while its
    /// frame's version holds (`in_copy`). A key the leaf holds is found
    /// with no node made of it, at the slot after the last found if it is
    /// there (`node/lean.rs`, RULES.md D950).
    fn in_leaf<R, F>(&self, key: &[u8], (_within, stamp): (Within<'_>, u64), near: &mut Near, reader: &mut Reader<F>) -> Result<Tried<R>>
    where
        F: FnOnce(&[u8]) -> Result<R>,
    {
        let Some((first, _)) = near.leaf.filter(|&(_, at)| at == stamp && near.misses < MOST_MISSES) else { return Ok(Tried::Elsewhere) };
        if let Tried::Answered(found) = self.in_copy(key, first, near, reader)? {
            return Ok(Tried::Answered(found));
        }
        let mut next = Some(first);
        for _ in 0..LEAVES_TRIED {
            let Some(leaf) = next else { break };
            let (page, version) = self.pool.read_stamped(self.file, leaf, Some(&self.tally))?;
            let after = near.at.and_then(|at| at.checked_add(1));
            if let Held::At(slot, value) = node::held(&page, self.meta.0, key, after) {
                self.tally.lookup();
                (near.misses, near.at, near.leaf) = (0, Some(slot), Some((leaf, stamp)));
                keep(near, (leaf, first), (&page, version));
                return reader.show(value).map(Tried::Answered);
            }
            let node = Node::parse(&page)?;
            if answers(&node, key)? {
                self.tally.lookup();
                (near.misses, near.leaf) = (0, Some((leaf, stamp)));
                keep(near, (leaf, first), (&page, version));
                return located(&node, key, &mut near.at, reader).map(Tried::Answered);
            }
            next = node.right.filter(|_| node.level == 0 && !node.deleted && !node.covers(key)).map(|right| right.0);
        }
        near.misses = near.misses.saturating_add(1);
        Ok(Tried::Elsewhere)
    }

    /// As `in_leaf`, if `near` keeps a copy of leaf `leaf` that is still
    /// the leaf's content and holds `key`. Such a read reads no page,
    /// takes no latch and writes no shared word (RULES.md D806). The key
    /// is found in the copy's bytes with no node made of them, at the slot
    /// after the last one found with one comparison (D808, D950). One
    /// the copy does not hold is not in the tree if the copy answers for
    /// it, as a read of the leaf would find.
    fn in_copy<R, F>(&self, key: &[u8], leaf: u64, near: &mut Near, reader: &mut Reader<F>) -> Result<Tried<R>>
    where
        F: FnOnce(&[u8]) -> Result<R>,
    {
        let Some((_, version)) = near.kept.filter(|&(kept, _)| kept == leaf) else { return Ok(Tried::Elsewhere) };
        if !self.pool.unchanged(self.file, leaf, version) {
            near.kept = None;
            return Ok(Tried::Elsewhere);
        }
        let Near { page, at, misses, .. } = near;
        let after = at.and_then(|at| at.checked_add(1));
        if let Held::At(slot, value) = node::held(page, self.meta.0, key, after) {
            self.tally.lookup();
            (*misses, *at) = (0, Some(slot));
            return reader.show(value).map(Tried::Answered);
        }
        let node = Node::parse(page)?;
        if !answers(&node, key)? {
            return Ok(Tried::Elsewhere);
        }
        self.tally.lookup();
        *misses = 0;
        located(&node, key, at, reader).map(Tried::Answered)
    }
}

/// Copies leaf `leaf`'s page `page`, read with `version`, into `near`, if
/// it is the leaf `near` remembered, `first`, so it answered twice in a
/// row (`BLinkTree::in_copy`).
fn keep(near: &mut Near, (leaf, first): (u64, u64), (page, version): (&[u8], Stamp)) {
    if leaf == first {
        near.page.clear();
        near.page.extend_from_slice(page);
        near.kept = Some((leaf, version));
    }
}

impl BLinkTree {
    /// As `in_leaf`, if the leaf noted for `key`'s row answers, or one
    /// right of it within `HOPS` links.
    fn hinted<R, F>(&self, key: &[u8], (within, stamp): (Within<'_>, u64), near: &mut Near, reader: &mut Reader<F>) -> Result<Tried<R>>
    where
        F: FnOnce(&[u8]) -> Result<R>,
    {
        if let Some(found) = self.in_noted(key, (within, stamp), near, reader)? {
            return Ok(Tried::Answered(Some(found)));
        }
        let at = &mut near.at;
        let Some((leaf, found)) = self.at_noted(within, key, |node| located(node, key, at, reader))? else { return Ok(Tried::Elsewhere) };
        // The misses stay as they are: the leaf remembered did not answer,
        // so the reads are not side by side, and a hint that answers says
        // nothing else. Cleared here, reads of rows far apart kept trying
        // the leaf of the last and its neighbour before every hint.
        near.leaf = Some((leaf, stamp));
        Ok(Tried::Answered(found))
    }

    /// \[DESIGN\] (RULES.md D950) What the reader makes of `key`'s value
    /// if the leaf noted for `key`'s row holds `key`, read with no node
    /// made of it (`node/lean.rs`): the leaf need only be one of this
    /// tree, in it, as it then holds no key but of its range, so neither
    /// its first key nor its high key is read. None of a leaf without
    /// the key, and of no leaf noted or held by a frame; the caller then
    /// reads as `at_noted` does. `near` is left as `hinted` leaves it.
    fn in_noted<R, F>(&self, key: &[u8], (_within, stamp): (Within<'_>, u64), near: &mut Near, reader: &mut Reader<F>) -> Result<Option<R>>
    where
        F: FnOnce(&[u8]) -> Result<R>,
    {
        let Some(id) = self.row_of(key).and_then(hints::leaf_of) else { return Ok(None) };
        let Some(page) = leaves::read_resident(&self.pool, (self.file, id))? else { return Ok(None) };
        let Held::At(slot, value) = node::held(&page, self.meta.0, key, None) else { return Ok(None) };
        self.tally.lookup();
        self.counters.hinted.add();
        // The misses stay as they are, as `hinted` leaves them.
        (near.at, near.leaf) = (Some(slot), Some((id, stamp)));
        reader.show(value)
    }

    /// What `at` finds in the leaf for `key`, and that leaf, if the leaf
    /// noted for `key`'s row is it, or leads to it within `HOPS` links;
    /// none if the tree is no table's, no leaf is noted, or the one noted
    /// may not be trusted. The caller is in its epoch.
    pub(super) fn at_noted<R>(&self, _within: Within<'_>, key: &[u8], at: impl FnOnce(&Node<'_>) -> Result<R>) -> Result<Option<(u64, R)>> {
        let Some(mut id) = self.row_of(key).and_then(hints::leaf_of) else { return Ok(None) };
        // From the thread's copy of the leaf, if it keeps one (`leaves.rs`).
        let Some(mut page) = leaves::read_resident(&self.pool, (self.file, id))? else { return Ok(None) };
        for hop in 0..=HOPS {
            let right = {
                // Of no use, rather than malformed: it may be any page.
                let Ok(node) = page.node() else { return Ok(None) };
                if hop == 0 && !trusted(&node, self.meta.0, key)? {
                    return Ok(None);
                }
                if answers(&node, key)? {
                    self.tally.lookup();
                    self.counters.hinted.add();
                    return at(&node).map(|found| Some((id, found)));
                }
                node.right.filter(|_| node.level == 0 && (node.deleted || !node.covers(key)))
            };
            let Some(right) = right else { return Ok(None) };
            drop(page);
            super::count(&self.counters.right_moves);
            id = right.0;
            page = leaves::read(&self.pool, (self.file, id), None)?;
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

/// What the reader makes of `key`'s value in `leaf`, shown where it lies,
/// none if `leaf` does not hold `key`; its place left in `at`. The entry
/// after the one at `at` is tried first, as a scan through an index asks
/// for the rows of a range in order; its key is compared, so a place left
/// by another leaf costs one comparison.
fn located<R, F>(leaf: &Node<'_>, key: &[u8], at: &mut Option<usize>, reader: &mut Reader<F>) -> Result<Option<R>>
where
    F: FnOnce(&[u8]) -> Result<R>,
{
    let next = at.and_then(|at| at.checked_add(1)).filter(|&next| next < leaf.len());
    let found = match next {
        Some(next) if leaf.key(next)?.is(key) => Ok(next),
        _ => leaf.search(key)?,
    };
    *at = found.ok();
    match found {
        Ok(found) => reader.show(leaf.value(found)?),
        Err(_) => Ok(None),
    }
}

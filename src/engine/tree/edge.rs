//! \[DESIGN\] Writers on hot nodes (RULES.md D503). A writer owns a node
//! through its frame (`disk/buffer/owned.rs`), which counts the writers
//! that waited for it.
//!
//! Keys written in order, a table's row ids or an index on a growing
//! value, all go past the end of the rightmost leaf. The tree remembers
//! that leaf when an entry goes there, and the next entry is written
//! straight into it, with no descent, if it may be trusted as a leaf
//! read from a hint is (`near.rs`). A frame holds it now, it is a leaf
//! of this tree, not deleted, with no right neighbour, and its first key
//! is at or below the key. Such a leaf is the one for the key, and a
//! split of it finds its parent again from the root, as one past the top
//! of a remembered path does.
//!
//! A leaf writers keep waiting for is split before it is full, at its
//! middle, so the keys they write go to two leaves and two owners. Only
//! a leaf mostly full, for an entry not past the end of the rightmost
//! leaf: keys written in order all go to one leaf whatever is split, and
//! the leaves they leave behind stay full.

use std::sync::atomic::Ordering;

use super::node::Node;
use super::{BLinkTree, NodeLock, Slot, count, near};
use crate::disk::pager::PageId;
use crate::error::Result;

/// \[DESIGN\] The waits for a leaf after which it is split early.
pub(super) const CONTENDED: u32 = 32;

impl BLinkTree {
    /// Owns node `id` for writing.
    pub(super) fn lock(&self, id: PageId) -> Result<NodeLock<'_>> {
        self.pool.own(self.file, id)
    }

    /// The rightmost leaf to write `key` into with no descent, if the tree
    /// remembers one it may trust for it; one no longer the rightmost leaf
    /// is forgotten.
    pub(super) fn edge_for(&self, key: &[u8]) -> Result<Option<PageId>> {
        let id = self.edge.load(Ordering::Relaxed);
        if id == 0 {
            return Ok(None);
        }
        let Some(page) = self.pool.read_resident(self.file, id)? else { return Ok(self.forget(id)) };
        // Of no use, rather than malformed: it may be any page.
        let Ok(node) = Node::parse(&page) else { return Ok(self.forget(id)) };
        if node.right.is_some() || node.tree != self.meta.0 || node.level != 0 || node.deleted {
            return Ok(self.forget(id));
        }
        // Still the rightmost leaf, for a later key.
        if !near::trusted(&node, self.meta.0, key)? {
            return Ok(None);
        }
        self.counters.edge_writes.add();
        Ok(Some(PageId(id)))
    }

    /// Forgets the rightmost leaf `id`, if it is still the one remembered.
    fn forget(&self, id: u64) -> Option<PageId> {
        let _ = self.edge.compare_exchange(id, 0, Ordering::Relaxed, Ordering::Relaxed);
        None
    }

    /// Remembers `leaf` as the one the last entry written past the end of
    /// the rightmost leaf went to.
    pub(super) fn note_edge(&self, leaf: PageId) {
        if self.edge.load(Ordering::Relaxed) != leaf.0 {
            self.edge.store(leaf.0, Ordering::Relaxed);
        }
    }

    /// Whether leaf `node`, owned as `held`, is split early for `slot`'s
    /// change, writers having waited for it; their count cleared if so.
    pub(super) fn crowded(&self, held: &NodeLock<'_>, node: &Node<'_>, slot: Slot) -> bool {
        let Slot::Insert(at) = slot else { return false };
        let past_the_end = node.right.is_none() && at == node.len();
        let full = node.live().saturating_mul(4) >= self.capacity.saturating_mul(3);
        let crowded = node.level == 0 && !past_the_end && full && held.contended() >= CONTENDED;
        if crowded {
            held.settle();
            count(&self.counters.contention_splits);
        }
        crowded
    }
}

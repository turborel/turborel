//! \[DESIGN\] Leaves taken out of an R-link tree, and their pages freed
//! once no operation can reach them (RULES.md D139). A remove that leaves
//! a leaf empty, or under the R*-tree's minimum fill of two fifths
//! (Beckmann et al., section 4.1), takes it out of its parent. An empty
//! leaf just goes; one with entries is merged with the sibling whose
//! envelope grows least, if both fit a loaded node's fill.
//!
//! A merge copies, never moves. The two leaves' entries go to a new page;
//! the old leaves are marked retired, their entries, stamps and right
//! links kept as they were; then the parent's entry for the partner names
//! the new page and the leaf's entry goes, in one write. A scan that read
//! the parent before that write reaches the old leaves, frozen, and sees
//! each entry once; one that read it after sees the new page only. Nothing
//! moves into a node a scan already read, so nothing is missed or seen
//! twice. A writer that reaches a retired leaf starts again from the root.
//!
//! Locks are taken leaves first, by page number, then the parent, bottom
//! to top, as every writer of this tree locks. Only leaves are taken out,
//! and only when their parent keeps another child, so no branch empties.
//! A retired page is freed when the tree's epoch has moved on twice, as
//! the B-link tree's are (`engine/tree/reclaim.rs`). Every operation and
//! scan is pinned in the epoch it began in.

use std::sync::atomic::Ordering;

use super::node::{Entry, Node};
use super::scan::Walk;
use super::{RTree, corrupt, pack};
use crate::disk::pager::PageId;
use crate::error::Result;
use crate::spatial::{Bounds, partition};

/// \[DESIGN\] Minimum fill, the R*-tree's two fifths of a node's capacity.
const MIN_FILL: (usize, usize) = (2, 5);

/// A leaf and its parent's entry for it, read under their locks.
struct Held {
    id: PageId,
    node: Node,
    at: usize,
}

impl RTree {
    /// Entries under which a leaf is taken out or merged.
    pub(super) fn minimum_fill(&self) -> usize {
        self.inner.capacity.saturating_mul(MIN_FILL.0).checked_div(MIN_FILL.1).unwrap_or_default()
    }

    /// Takes out or merges `leaf`, from which an entry of `bounds` was just
    /// removed, if it is now under minimum fill. Nothing if a check fails.
    ///
    /// # Errors
    ///
    /// Storage errors or malformed links. A failure before the parent is
    /// written changes nothing a reader or writer can see.
    pub(super) fn reclaim(&self, leaf: PageId, bounds: Bounds) -> Result<()> {
        let count = self.read(leaf)?.entries.len();
        if count >= self.minimum_fill() {
            return Ok(());
        }
        let Some(parent) = self.parent_of(leaf, bounds)? else { return Ok(()) };
        let partner = if count == 0 { None } else { self.partner(parent, leaf)? };
        let mut ids: Vec<PageId> = std::iter::once(leaf).chain(partner).collect();
        ids.sort_unstable_by_key(|id| id.0);
        let _leaves = ids.iter().map(|&id| self.inner.locks.lock(id)).collect::<Result<Vec<_>>>()?;
        let _parent = self.inner.locks.lock(parent)?;
        let mut above = self.read(parent)?;
        let Some(held) = self.held(&above, leaf)? else { return Ok(()) };
        let mut retired = vec![held];
        if let Some(partner) = partner {
            let Some(other) = self.held(&above, partner)? else { return Ok(()) };
            retired.push(other);
        }
        if !self.worth(&retired, &above) {
            return Ok(());
        }
        self.take_out(parent, &mut above, retired)
    }

    /// Whether the leaf, `retired`'s first, is still under minimum fill,
    /// its parent keeps another child, and it is empty or fits in one
    /// loaded node with its partner.
    fn worth(&self, retired: &[Held], above: &Node) -> bool {
        let entries: usize = retired.iter().map(|h| h.node.entries.len()).sum();
        let under = retired.first().is_some_and(|h| h.node.entries.len() < self.minimum_fill());
        let fits = if retired.len() > 1 { entries <= pack::fill(self.inner.capacity) } else { entries == 0 };
        under && fits && above.entries.len() > 1
    }

    /// The level-one node with an entry for `leaf`. Its ancestors' entries
    /// cover every entry it held, so the search follows `bounds`.
    fn parent_of(&self, leaf: PageId, bounds: Bounds) -> Result<Option<PageId>> {
        let mut walk = Walk::new(self.clone(), Some(bounds));
        while let Some((id, node)) = walk.node()? {
            if node.level == 1 && node.entries.iter().any(|e| e.id == leaf.0) {
                return Ok(Some(id));
            }
        }
        Ok(None)
    }

    /// The sibling of `leaf` under `parent` whose envelope grows least
    /// taking in `leaf`'s, if `parent` has another child.
    fn partner(&self, parent: PageId, leaf: PageId) -> Result<Option<PageId>> {
        let above = self.read(parent)?;
        let Some(own) = above.entries.iter().find(|e| e.id == leaf.0) else { return Ok(None) };
        let others: Vec<&Entry> = above.entries.iter().filter(|e| e.id != leaf.0).collect();
        if others.is_empty() {
            return Ok(None);
        }
        let bounds: Vec<Bounds> = others.iter().map(|e| e.bounds).collect();
        let chosen = partition::choose(&bounds, own.bounds, false)?;
        Ok(others.get(chosen).map(|e| PageId(e.id)))
    }

    /// `id` read under its lock, if `above` posts it at its current stamp
    /// and it is a leaf neither split unposted nor retired.
    fn held(&self, above: &Node, id: PageId) -> Result<Option<Held>> {
        let node = self.read(id)?;
        let at = above.entries.iter().position(|e| e.id == id.0 && e.value == node.stamp);
        let usable = above.level == 1 && !above.pending && !node.pending && !node.retired;
        Ok(at.filter(|_| usable).map(|at| Held { id, node, at }))
    }

    /// Retires `retired`, their entries copied to one new leaf if any, and
    /// writes `above` without them, the new leaf in the last one's place.
    fn take_out(&self, parent: PageId, above: &mut Node, mut retired: Vec<Held>) -> Result<()> {
        let entries: Vec<Entry> = retired.iter().flat_map(|h| h.node.entries.iter().copied()).collect();
        let merged = if entries.is_empty() { None } else { Some(self.new_leaf(entries)?) };
        let mut places: Vec<usize> = retired.iter().map(|h| h.at).collect();
        if let Some(entry) = merged {
            let last = places.pop().ok_or_else(|| corrupt("a merge has no partner"))?;
            *above.entries.get_mut(last).ok_or_else(|| corrupt("a merged entry disappeared"))? = entry;
        }
        places.sort_unstable();
        for at in places.into_iter().rev() {
            above.entries.remove(at);
        }
        let marked = retired.iter_mut().try_for_each(|held| {
            held.node.retired = true;
            self.store(held.id, &held.node)
        });
        if let Err(error) = marked.and_then(|()| self.store(parent, above)) {
            self.restore(&mut retired, merged);
            return Err(error);
        }
        for held in &retired {
            self.inner.epochs.retire(held.id);
            self.inner.retired.fetch_add(1, Ordering::Relaxed);
        }
        self.inner.reclaimed.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// The parent was not written, so the leaves are made live again, and the
    /// new leaf, which nothing links to, is freed. A failure here leaves a
    /// retired leaf in the tree, which writers then wait on; the original
    /// error is the one reported.
    fn restore(&self, retired: &mut [Held], merged: Option<Entry>) {
        for held in retired {
            held.node.retired = false;
            let _ = self.store(held.id, &held.node);
        }
        if let Some(entry) = merged {
            let _ = self.inner.pool.free(self.inner.file, PageId(entry.id));
        }
    }

    fn new_leaf(&self, entries: Vec<Entry>) -> Result<Entry> {
        let stamp = self.fresh_stamp()?;
        let mut node = Node::empty(stamp, 0);
        node.entries = entries;
        let bounds = node.bounds().ok_or_else(|| corrupt("a merged leaf has no entries"))?;
        let (id, mut page) = self.inner.pool.new_page(self.inner.file)?;
        *page = node.bytes()?;
        Ok(Entry { bounds, id: id.0, value: stamp })
    }

    /// Frees the retired pages no operation can hold any more.
    ///
    /// # Errors
    ///
    /// The buffer pool's. Pages not freed by a failure stay allocated, which
    /// costs space only.
    pub(super) fn release(&self) -> Result<()> {
        if self.inner.retired.load(Ordering::Relaxed) == 0 {
            return Ok(());
        }
        for page in self.inner.epochs.freeable() {
            self.inner.retired.fetch_sub(1, Ordering::Relaxed);
            self.inner.pool.free_when_released(self.inner.file, page)?;
        }
        Ok(())
    }
}

//! \[DESIGN\] Failure-atomic logical changes. Splits and parent postings
//! happen before an insert gains a reference. Readers can traverse an
//! unfinished split, while writers help finish it before enlarging it.
//! Node locks are acquired bottom-up when posting; descent holds one.

use std::sync::atomic::Ordering;

use super::node::{Entry, MAX_LEVEL, Node};
use super::scan::Walk;
use super::{RTree, Visit, corrupt, meta_bytes};
use crate::disk::pager::PageId;
use crate::error::{Code, Result, SqlError};
use crate::spatial::{Bounds, partition};
use crate::storage::RowId;

impl RTree {
    pub(super) fn change_existing(&self, bounds: Bounds, row: RowId, increment: bool) -> Result<bool> {
        loop {
            let Some(id) = self.find(bounds, row)? else { return Ok(false) };
            let _held = self.inner.locks.lock(id)?;
            let mut node = self.read(id)?;
            let at = node.entries.iter().position(|e| e.id == row && e.bounds == bounds);
            let Some(at) = at else { continue };
            let entry = node.entries.get_mut(at).ok_or_else(|| corrupt("an entry just found is missing"))?;
            if increment {
                entry.value = entry
                    .value
                    .checked_add(1)
                    .ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] an R-tree entry has too many references"))?;
            } else if entry.value > 1 {
                entry.value = entry.value.saturating_sub(1);
            } else {
                node.entries.remove(at);
            }
            self.store(id, &node)?;
            return Ok(true);
        }
    }

    fn find(&self, bounds: Bounds, row: RowId) -> Result<Option<PageId>> {
        let mut walk = Walk::new(self.clone(), Some(bounds));
        while let Some((id, node)) = walk.leaf()? {
            if node.entries.iter().any(|e| e.id == row && e.bounds == bounds) {
                return Ok(Some(id));
            }
        }
        Ok(None)
    }

    pub(super) fn add(&self, bounds: Bounds, row: RowId) -> Result<()> {
        loop {
            let Some((visit, path)) = self.descend(bounds)? else { continue };
            let held = self.inner.locks.lock(visit.id)?;
            let mut node = self.read(visit.id)?;
            if node.stamp != visit.stamp || node.pending {
                continue;
            }
            if node.entries.len() >= self.inner.capacity {
                drop(held);
                self.finish_split(visit.id, path)?;
                continue;
            }
            node.entries.push(Entry { bounds, id: row, value: 1 });
            self.store(visit.id, &node)?;
            return Ok(());
        }
    }

    fn descend(&self, bounds: Bounds) -> Result<Option<(Visit, Vec<Visit>)>> {
        let mut visit = self.root()?;
        let mut path = Vec::new();
        loop {
            let held = self.inner.locks.lock(visit.id)?;
            let mut node = self.read(visit.id)?;
            if node.pending {
                drop(held);
                self.finish_split(visit.id, path)?;
                return Ok(None);
            }
            if node.stamp != visit.stamp {
                return Ok(None);
            }
            if node.level != visit.level {
                return Err(corrupt("a descent changed levels unexpectedly"));
            }
            if node.level == 0 {
                return Ok(Some((visit, path)));
            }
            let bounds_of: Vec<_> = node.entries.iter().map(|e| e.bounds).collect();
            let chosen = partition::choose(&bounds_of, bounds, node.level == 1)?;
            let child = node.entries.get_mut(chosen).ok_or_else(|| corrupt("the chosen child is missing"))?;
            let enlarged = child.bounds.union(bounds);
            let changed = enlarged != child.bounds;
            child.bounds = enlarged;
            let next = Visit { id: PageId(child.id), stamp: child.value, level: node.level.saturating_sub(1) };
            if changed {
                self.store(visit.id, &node)?;
            }
            path.push(visit);
            visit = next;
        }
    }

    /// Work moves upward when a parent must split first. The original
    /// child remains linked; the next descent finishes its posting. No
    /// recursion, and no chain of ancestor locks is held while helping.
    pub(super) fn finish_split(&self, mut id: PageId, mut path: Vec<Visit>) -> Result<()> {
        loop {
            let held = self.inner.locks.lock(id)?;
            let mut node = self.read(id)?;
            if !node.pending {
                if node.entries.len() < self.inner.capacity {
                    return Ok(());
                }
                node = self.split_node(id, node)?;
            }
            let root = self.root()?;
            if root.id == id {
                self.grow(id, &mut node)?;
                return Ok(());
            }
            let Some(parent) = path.pop() else {
                drop(held);
                path = self.ancestry(id, node.level)?;
                continue;
            };
            match self.post(id, &mut node, parent)? {
                Posted::Done => return Ok(()),
                Posted::Help(parent) => id = parent,
                Posted::Moved => {
                    drop(held);
                    path = self.ancestry(id, node.level)?;
                }
            }
        }
    }

    pub(super) fn split_node(&self, id: PageId, mut node: Node) -> Result<Node> {
        let bounds: Vec<_> = node.entries.iter().map(|e| e.bounds).collect();
        let split = partition::split(&bounds)?;
        let entries: Vec<_> = split
            .order
            .iter()
            .map(|&at| node.entries.get(at).copied().ok_or_else(|| corrupt("a split entry is missing")))
            .collect::<Result<_>>()?;
        let (left, right) = entries.split_at_checked(split.at).ok_or_else(|| corrupt("a split position is past its entries"))?;
        let (new, mut page) = self.inner.pool.new_page(self.inner.file)?;
        let right = Node { level: node.level, stamp: node.stamp, right: node.right, pending: false, entries: right.to_vec() };
        *page = right.bytes()?;
        drop(page);
        node.entries = left.to_vec();
        node.stamp = new.0;
        node.right = Some(new);
        node.pending = true;
        if let Err(error) = self.store(id, &node) {
            // No link was published: freeing can only fail by leaking an
            // unreachable page; preserve the original insertion error.
            let _ = self.inner.pool.free(self.inner.file, new);
            return Err(error);
        }
        self.inner.splits.fetch_add(1, Ordering::Relaxed);
        Ok(node)
    }

    fn grow(&self, id: PageId, node: &mut Node) -> Result<()> {
        let _meta = self.inner.locks.lock(self.inner.meta)?;
        if self.root()?.id != id {
            return Ok(());
        }
        let level = node.level.checked_add(1).filter(|&n| n <= MAX_LEVEL).ok_or_else(|| corrupt("its height limit is exceeded"))?;
        let right_id = node.right.ok_or_else(|| corrupt("a pending split has no sibling"))?;
        let right = self.read(right_id)?;
        let (new, mut page) = self.inner.pool.new_page(self.inner.file)?;
        let mut root = Node::empty(new, level);
        let empty = Bounds::point([0.0_f64; crate::spatial::DIMENSIONS])?;
        root.entries.push(Entry { bounds: node.bounds().unwrap_or(empty), id: id.0, value: node.stamp });
        root.entries.push(Entry { bounds: right.bounds().unwrap_or(empty), id: right_id.0, value: right.stamp });
        *page = root.bytes()?;
        drop(page);
        *self.inner.pool.write(self.inner.file, self.inner.meta)? = meta_bytes(new, root.stamp);
        node.pending = false;
        self.store(id, node)
    }

    fn post(&self, id: PageId, node: &mut Node, mut parent: Visit) -> Result<Posted> {
        let mut visited = 0_u64;
        loop {
            visited = visited.saturating_add(1);
            if visited > self.inner.pool.pages_in_use(self.inner.file)? {
                return Err(corrupt("its parent links contain a cycle"));
            }
            let _parent = self.inner.locks.lock(parent.id)?;
            let mut above = self.read(parent.id)?;
            if above.level != node.level.saturating_add(1) {
                return Err(corrupt("a split's parent is at the wrong level"));
            }
            let at = above.entries.iter().position(|e| e.id == id.0);
            if let Some(at) = at {
                let old = *above.entries.get(at).ok_or_else(|| corrupt("a parent's entry disappeared"))?;
                if old.value == node.stamp {
                    node.pending = false;
                    self.store(id, node)?;
                    return Ok(Posted::Done);
                }
                if above.pending || above.entries.len() >= self.inner.capacity {
                    return Ok(Posted::Help(parent.id));
                }
                let right_id = node.right.ok_or_else(|| corrupt("a split has no right sibling"))?;
                let right = self.read(right_id)?;
                *above.entries.get_mut(at).ok_or_else(|| corrupt("a parent's entry disappeared"))? =
                    Entry { bounds: node.bounds().unwrap_or(old.bounds), id: id.0, value: node.stamp };
                above.entries.push(Entry { bounds: right.bounds().unwrap_or(old.bounds), id: right_id.0, value: right.stamp });
                self.store(parent.id, &above)?;
                node.pending = false;
                self.store(id, node)?;
                return Ok(Posted::Done);
            }
            if above.stamp == parent.stamp {
                return Ok(Posted::Moved);
            }
            parent.id = above.right.ok_or_else(|| corrupt("a changed parent has no sibling"))?;
        }
    }

    /// A root may have grown after the writer remembered its path. Find
    /// its original child, not the geometrically cheapest parent: posting
    /// elsewhere would make a reader of the original entry miss the split.
    fn ancestry(&self, id: PageId, level: u8) -> Result<Vec<Visit>> {
        let mut stack = vec![(self.root()?, Vec::new())];
        let mut visited = 0_u64;
        while let Some((visit, path)) = stack.pop() {
            visited = visited.saturating_add(1);
            if visited > self.inner.pool.pages_in_use(self.inner.file)? {
                return Err(corrupt("its parent search contains a cycle"));
            }
            let node = self.read(visit.id)?;
            if node.stamp != visit.stamp {
                let right = node.right.ok_or_else(|| corrupt("a changed ancestor has no sibling"))?;
                stack.push((Visit { id: right, ..visit }, path.clone()));
            }
            let mut below = path;
            below.push(Visit { stamp: node.stamp, ..visit });
            if node.level == level.saturating_add(1) {
                if node.entries.iter().any(|e| e.id == id.0) {
                    return Ok(below);
                }
            } else if node.level > level {
                for entry in &node.entries {
                    stack.push((Visit { id: PageId(entry.id), stamp: entry.value, level: node.level.saturating_sub(1) }, below.clone()));
                }
            }
        }
        Err(corrupt("a split node has no parent"))
    }
}

enum Posted {
    Done,
    Help(PageId),
    Moved,
}

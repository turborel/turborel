//! \[DESIGN\] R-link traversal. A parent remembers the child's stamp. A
//! split gives the old node a new stamp and the new right node the old
//! stamp. Reading siblings stops at that remembered stamp, so a scan
//! neither misses moved entries nor also visits a sibling already queued
//! from the parent. A leaf is copied once under its buffer-pool latch.
//! A walk is pinned in the tree's epoch while it lives, so a node retired
//! by reclamation keeps its frozen entries for it (RULES.md D139).

use std::sync::atomic::Ordering;

use super::node::{Entry, Node};
use super::{Pin, RTree, Visit, corrupt};
use crate::disk::pager::PageId;
use crate::error::{Result, SqlError};
use crate::spatial::Bounds;
use crate::storage::RowId;

/// A streaming rectangle search. Memory holds one leaf's entries and a
/// depth-first stack of at most one node's children per level. No tree
/// latch is held between calls to `next`.
pub struct Scan {
    walk: Walk,
    entries: std::vec::IntoIter<Entry>,
    failed: bool,
}

impl Scan {
    pub(super) fn new(tree: RTree, bounds: Option<Bounds>) -> Scan {
        Scan { walk: Walk::new(tree, bounds), entries: Vec::new().into_iter(), failed: false }
    }
}

impl Iterator for Scan {
    type Item = Result<(Bounds, RowId)>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed {
            return None;
        }
        loop {
            if let Some(entry) = self.entries.next() {
                if self.walk.bounds.is_none_or(|b| b.intersects(entry.bounds)) {
                    return Some(Ok((entry.bounds, entry.id)));
                }
            } else {
                match self.walk.leaf() {
                    Ok(Some((_, node))) => self.entries = node.entries.into_iter(),
                    Ok(None) => return None,
                    Err(error) => {
                        self.failed = true;
                        return Some(Err(error));
                    }
                }
            }
        }
    }
}

pub(super) struct Walk {
    tree: RTree,
    bounds: Option<Bounds>,
    stack: Vec<Visit>,
    error: Option<SqlError>,
    visited: u64,
    limit: u64,
    _pin: Pin,
}

impl Walk {
    pub(super) fn new(tree: RTree, bounds: Option<Bounds>) -> Walk {
        let pin = tree.pin();
        let root = tree.root();
        let mut walk = Walk { tree, bounds, stack: Vec::new(), error: None, visited: 0, limit: 0, _pin: pin };
        match root {
            Ok(root) => walk.stack.push(root),
            Err(error) => walk.error = Some(error),
        }
        walk
    }

    pub(super) fn leaf(&mut self) -> Result<Option<(PageId, Node)>> {
        while let Some((id, node)) = self.node()? {
            if node.level == 0 {
                return Ok(Some((id, node)));
            }
        }
        Ok(None)
    }

    pub(super) fn node(&mut self) -> Result<Option<(PageId, Node)>> {
        if let Some(error) = self.error.take() {
            return Err(error);
        }
        let Some(visit) = self.stack.pop() else { return Ok(None) };
        self.count_visit()?;
        let node = self.tree.read(visit.id)?;
        if node.level != visit.level {
            return Err(corrupt("a child is not at the level its parent describes"));
        }
        if node.stamp != visit.stamp {
            let id = node.right.ok_or_else(|| corrupt("a split node has no right link"))?;
            self.stack.push(Visit { id, ..visit });
            self.tree.inner.right_moves.fetch_add(1, Ordering::Relaxed);
        }
        if node.level > 0 {
            for entry in node.entries.iter().rev() {
                if self.bounds.is_none_or(|b| b.intersects(entry.bounds)) {
                    self.stack.push(Visit { id: PageId(entry.id), stamp: entry.value, level: node.level.saturating_sub(1) });
                }
            }
        }
        Ok(Some((visit.id, node)))
    }

    /// A valid traversal visits each physical node at most once. Read the
    /// allocated-page count only when necessary, allowing concurrent tree
    /// growth while bounding a corrupt cycle without a set of every page.
    fn count_visit(&mut self) -> Result<()> {
        self.visited = self.visited.saturating_add(1);
        if self.visited > self.limit {
            self.limit = self.tree.inner.pool.pages_in_use(self.tree.inner.file)?;
            if self.visited > self.limit {
                return Err(corrupt("its links revisit more nodes than its file holds"));
            }
        }
        Ok(())
    }
}

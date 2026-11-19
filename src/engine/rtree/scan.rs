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
/// latch is held between calls to `next`. Leaves are read into one
/// buffer, and branches into another, so a search allocates only as the
/// largest node it reads grows them.
pub struct Scan {
    walk: Walk,
    entries: Vec<Entry>,
    at: usize,
    failed: bool,
}

impl Scan {
    pub(super) fn new(tree: RTree, bounds: Option<Bounds>) -> Scan {
        Scan { walk: Walk::new(tree, bounds), entries: Vec::new(), at: 0, failed: false }
    }
}

impl Iterator for Scan {
    type Item = Result<(Bounds, RowId)>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed {
            return None;
        }
        loop {
            if let Some(&entry) = self.entries.get(self.at) {
                self.at = self.at.saturating_add(1);
                if self.walk.bounds.is_none_or(|b| b.intersects(entry.bounds)) {
                    return Some(Ok((entry.bounds, entry.id)));
                }
            } else {
                match self.walk.leaf_into(&mut self.entries) {
                    Ok(true) => self.at = 0,
                    Ok(false) => return None,
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
    /// The branch read last, its entries' allocation kept for the next.
    scratch: Node,
    _pin: Pin,
}

impl Walk {
    pub(super) fn new(tree: RTree, bounds: Option<Bounds>) -> Walk {
        let pin = tree.pin();
        let root = tree.root();
        let scratch = Node::empty(0, 0);
        let mut walk = Walk { tree, bounds, stack: Vec::new(), error: None, visited: 0, limit: 0, scratch, _pin: pin };
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
        let mut node = Node::empty(0, 0);
        Ok(self.visit_into(&mut node)?.map(|id| (id, node)))
    }

    /// The next leaf's entries, into `entries`, reusing both buffers;
    /// false once the walk is done.
    pub(super) fn leaf_into(&mut self, entries: &mut Vec<Entry>) -> Result<bool> {
        let mut node = std::mem::replace(&mut self.scratch, Node::empty(0, 0));
        let mut found = false;
        while self.visit_into(&mut node)?.is_some() {
            if node.level == 0 {
                std::mem::swap(&mut node.entries, entries);
                found = true;
                break;
            }
        }
        self.scratch = node;
        Ok(found)
    }

    /// Reads the next node into `node`, queueing its split sibling and the
    /// children its bounds allow; its page, or none once the walk is done.
    fn visit_into(&mut self, node: &mut Node) -> Result<Option<PageId>> {
        if let Some(error) = self.error.take() {
            return Err(error);
        }
        let Some(visit) = self.stack.pop() else { return Ok(None) };
        self.count_visit()?;
        self.tree.read_into(visit.id, node)?;
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
        Ok(Some(visit.id))
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

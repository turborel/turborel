//! \[DESIGN\] What reclamation leaves above the parent it changed (RULES.md
//! D139). Each ancestor's entry for the node below is narrowed to that
//! node's envelope, so a search stops reading branches with nothing in
//! its window; and a root left with a single child gives way to it.
//!
//! A narrowed entry is written with the child's lock and then its parent's
//! held, bottom to top. A writer enlarges a parent's entry before it locks
//! the child, so a narrowing between the two could drop the writer's
//! bounds. The writer therefore checks, once it holds the child, that the
//! parent's entry still covers its bounds, and descends again if not
//! (`write.rs`). Narrowing needs that child's lock, so once the check
//! passes no narrowing of that entry can come before the writer has
//! widened the child too.
//!
//! The root gives way under its own lock and then the meta page's, as a
//! root grows. The meta page names the child, at the stamp the root's
//! entry remembers, then the old root is retired. A scan that began at
//! the old root reads it frozen and goes on to the child. If the child
//! split meanwhile, the stamp sends readers right along its link, and its
//! writer finishes the split by growing a new root, as for any root.

use std::sync::atomic::Ordering;

use super::super::node::MAX_LEVEL;
use super::super::{Meta, RTree, meta_bytes};
use crate::disk::pager::PageId;
use crate::error::Result;

impl RTree {
    /// Narrows the entry for `child` in each ancestor, from its parent up,
    /// while one changes.
    pub(super) fn narrow_above(&self, mut child: PageId) -> Result<()> {
        for _ in 0..=MAX_LEVEL {
            let Some(envelope) = self.read(child)?.bounds() else { return Ok(()) };
            let Some(parent) = self.parent_of(child, envelope)? else { return Ok(()) };
            if !self.narrow(child, parent)? {
                return Ok(());
            }
            child = parent;
        }
        Ok(())
    }

    /// Makes `parent`'s entry for `child` `child`'s envelope, if it posts
    /// `child` at its current stamp and is larger, and says whether it
    /// changed.
    fn narrow(&self, child: PageId, parent: PageId) -> Result<bool> {
        let _child = self.inner.locks.lock(child)?;
        let _parent = self.inner.locks.lock(parent)?;
        let below = self.read(child)?;
        let mut above = self.read(parent)?;
        let Some(envelope) = below.bounds() else { return Ok(false) };
        if below.retired || above.retired {
            return Ok(false);
        }
        let Some(entry) = above.entries.iter_mut().find(|e| e.id == child.0 && e.value == below.stamp) else { return Ok(false) };
        if entry.bounds == envelope || !entry.bounds.contains(envelope) {
            return Ok(false);
        }
        entry.bounds = envelope;
        self.store(parent, &above)?;
        self.inner.narrowed.fetch_add(1, Ordering::Relaxed);
        Ok(true)
    }

    /// While the root is a branch with one child, posted at its stamp,
    /// makes that child the root and retires the old one.
    pub(super) fn shrink_root(&self) -> Result<()> {
        for _ in 0..=MAX_LEVEL {
            let root = self.meta_fields()?.root;
            let _root = self.inner.locks.lock(root)?;
            let _meta = self.inner.locks.lock(self.inner.meta)?;
            let meta = self.meta_fields()?;
            let mut node = self.read(root)?;
            let alone = matches!(node.entries.as_slice(), [_]);
            let current = meta.root == root && node.stamp == meta.stamp && !node.retired;
            if !(current && node.level > 0 && alone && !node.pending) {
                return Ok(());
            }
            let Some(child) = node.entries.first().copied() else { return Ok(()) };
            *self.inner.pool.write(self.inner.file, self.inner.meta)? =
                meta_bytes(Meta { root: PageId(child.id), stamp: child.value, ..meta });
            // The meta page names the child already, so a failure to freeze
            // the old root leaves a page no new operation reaches.
            node.retired = true;
            self.store(root, &node)?;
            self.inner.epochs.retire(root);
            self.inner.retired.fetch_add(1, Ordering::Relaxed);
        }
        Ok(())
    }
}

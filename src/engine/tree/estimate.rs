//! \[DESIGN\] Where a key lies among a tree's entries, as a share of them,
//! by one descent (RULES.md D381): at each node the share of its children
//! left of the child the descent takes, of the share of the tree that
//! node holds, and at the leaf the share of its entries below the key.
//! Each child is taken to hold as many entries as its siblings, so the
//! share is near where nodes are about as full, which a B-tree's are
//! within a factor of two. The entries between two keys are the
//! difference of their shares times the tree's entries: an estimate of
//! any range in a descent to each bound, where counting its entries reads
//! every leaf it spans. A node split while it is read leaves its keys
//! moved right out, which shifts the share, never fails it.

use std::ops::Bound;
use std::sync::atomic::Ordering;

use super::BLinkTree;
use super::node::Node;
use crate::disk::pager::PageId;
use crate::error::Result;
use crate::stats::parts::count;

/// Which side of the keys equal to a bound's the share is taken at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    /// Below them: a lower bound including them, an upper bound not.
    Below,
    /// Above them.
    Above,
}

impl BLinkTree {
    /// The share of its entries a scan from `lower` to `upper` reads, by a
    /// descent to each (see the module's documentation).
    ///
    /// # Errors
    ///
    /// As reading a node fails.
    pub fn range_share(&self, lower: Bound<&[u8]>, upper: Bound<&[u8]>) -> Result<f64> {
        let from = match lower {
            Bound::Included(key) => self.share_below(key, Side::Below)?,
            Bound::Excluded(key) => self.share_below(key, Side::Above)?,
            Bound::Unbounded => 0.0_f64,
        };
        let to = match upper {
            Bound::Included(key) => self.share_below(key, Side::Above)?,
            Bound::Excluded(key) => self.share_below(key, Side::Below)?,
            Bound::Unbounded => 1.0_f64,
        };
        Ok((to - from).clamp(0.0, 1.0))
    }

    /// The share of its entries below `key`, or of those equal to it too
    /// if `side` is `Above`, by one descent.
    fn share_below(&self, key: &[u8], side: Side) -> Result<f64> {
        let _guard = self.epochs.guard();
        let mut id = PageId(self.root.load(Ordering::Acquire));
        let (mut before, mut width) = (0.0_f64, 1.0_f64);
        for _ in 0..=u8::MAX {
            let page = self.pool.read(self.file, id.0)?;
            let node = Node::parse(&page)?;
            let found = node.search(key)?;
            if node.level == 0 {
                let at = match (found, side) {
                    (Ok(i), Side::Above) => i.saturating_add(1),
                    (Ok(i) | Err(i), Side::Below | Side::Above) => i,
                };
                let entries = count(u64::try_from(node.len()).unwrap_or(u64::MAX));
                let share = if entries > 0.0_f64 { count(u64::try_from(at).unwrap_or(u64::MAX)) / entries } else { 0.0_f64 };
                return Ok(before + width * share);
            }
            // A key equal to an entry's is of the child on its left.
            let at = found.unwrap_or_else(|at| at);
            let children = count(u64::try_from(node.len().saturating_add(1)).unwrap_or(u64::MAX));
            before += width * count(u64::try_from(at).unwrap_or(u64::MAX)) / children;
            width /= children;
            id = node.child_at(at)?.0;
        }
        Ok(before)
    }
}

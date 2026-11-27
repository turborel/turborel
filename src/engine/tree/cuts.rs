//! \[DESIGN\] Keys a range of a tree is cut at, for scans of its parts run
//! apart (RULES.md D297). The keys of the tree's inner nodes split its
//! leaves' keys into runs of about a leaf's or more, so the range is cut
//! at those of the highest level that holds enough of them within it,
//! read level by level down from the root and no further than the level
//! above the leaves. A node split or emptied while they are read leaves
//! its keys out or in, which makes the parts less even, never wrong, since any
//! keys in order cut a range into parts that together hold each of its
//! keys once, as a scan of each reads its own (`scan.rs`).

use std::ops::Bound;
use std::sync::atomic::Ordering;

use super::BLinkTree;
use super::node::{KeyRef, Node};
use crate::disk::pager::PageId;
use crate::error::Result;

/// \[DESIGN\] The most nodes of one level read for cuts: a level of more
/// has keys enough above it.
const MOST_NODES: usize = 4096;

impl BLinkTree {
    /// Up to about `most` keys within `lower` and `upper`, in order, each
    /// above the one before: what cuts the range into parts of about as
    /// many keys each.
    ///
    /// # Errors
    ///
    /// As reading a node fails.
    pub fn cuts(&self, lower: Bound<&[u8]>, upper: Bound<&[u8]>, most: usize) -> Result<Vec<Vec<u8>>> {
        let op = self.epochs.guard();
        let mut nodes = vec![PageId(self.root.load(Ordering::Acquire))];
        let mut level = None;
        loop {
            let (mut keys, mut children) = (Vec::new(), Vec::new());
            for id in &nodes {
                let page = self.page(op.within(), *id)?;
                let node = Node::parse(&page)?;
                // A node of another level than its first's was moved past.
                if node.deleted || node.level == 0 || level.is_some_and(|l| l != node.level) {
                    continue;
                }
                level = Some(node.level);
                within(&node, (lower, upper), &mut keys, &mut children)?;
            }
            let lowest = level.is_none_or(|l| l <= 1);
            if keys.len() >= most || lowest || children.len() > MOST_NODES {
                return Ok(spread(keys, most));
            }
            nodes = children;
            level = level.map(|l| l.saturating_sub(1));
        }
    }
}

/// Adds to `keys` those of inner node `node` within `bounds`, and to
/// `children` the children whose ranges meet them.
fn within(
    node: &Node<'_>,
    (lower, upper): (Bound<&[u8]>, Bound<&[u8]>),
    keys: &mut Vec<Vec<u8>>,
    children: &mut Vec<PageId>,
) -> Result<()> {
    for at in 0..=node.len() {
        let (child, below) = node.child_at(at)?;
        let below = below.map(KeyRef::to_vec);
        let above = if at < node.len() { Some(node.key(at)?.to_vec()) } else { node.high.map(<[u8]>::to_vec) };
        let past_upper = below.as_deref().is_some_and(|b| match upper {
            Bound::Included(u) | Bound::Excluded(u) => b >= u,
            Bound::Unbounded => false,
        });
        let before_lower = above.as_deref().is_some_and(|a| match lower {
            Bound::Included(l) | Bound::Excluded(l) => a < l,
            Bound::Unbounded => false,
        });
        if past_upper || before_lower {
            continue;
        }
        if let Some(b) = below.filter(|b| inside(b, lower)) {
            keys.push(b);
        }
        children.push(child);
    }
    Ok(())
}

/// Whether `key` is above `lower`.
fn inside(key: &[u8], lower: Bound<&[u8]>) -> bool {
    match lower {
        Bound::Included(l) | Bound::Excluded(l) => key > l,
        Bound::Unbounded => true,
    }
}

/// Up to about `most` of `keys`, evenly spread, sorted and each once.
fn spread(mut keys: Vec<Vec<u8>>, most: usize) -> Vec<Vec<u8>> {
    keys.sort_unstable();
    keys.dedup();
    let step = keys.len().div_ceil(most.max(1)).max(1);
    keys.into_iter().step_by(step).collect()
}

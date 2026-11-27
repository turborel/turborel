//! \[DESIGN\] A split whose half is past the page (RULES.md D500, D31).
//! A node keeps the prefix its keys share once, so it may hold more keys
//! than would fit whole. A split's halves each keep their own prefix, and
//! the split moves to an entry at which both fit (`node/split.rs`). Where
//! none does, as when every key is long, they share a long prefix, and the
//! key added shares little of it, a half past the page is split again,
//! and so on, until every node fits: one entry always does, as the
//! longest entry is a quarter of a page.
//!
//! The nodes are linked left to right as one split's are. The new pages
//! are written first, then the old one, so the change reaches the tree in
//! one write, as a split does (Lehman and Yao 1981, §6.2). Each new page's
//! separator goes up the tree after the first, which takes the child's
//! lock with it; until it is posted, its page is reached through the
//! right link of the one left of it, as the B-link tree allows (§4).
//!
//! A split that fails after it took a page, its file refusing the next,
//! gives back every page it took (RULES.md D1271): the old node is not
//! yet written over, so the tree is as it was.

use super::node::{self, Node};
use super::{BLinkTree, Split, count};
use crate::error::Result;

impl BLinkTree {
    /// `content`, a node, if it fits its page; else its left part that
    /// does, the rest split off to new pages, written, each
    /// split added to `splits`.
    pub(super) fn fitted(&self, content: Vec<u8>, splits: &mut Vec<Split>) -> Result<Vec<u8>> {
        if content.len() <= self.capacity {
            return Ok(content);
        }
        let node = Node::parse(&content)?;
        let entries = node.entries()?;
        let (new, mut page) = self.pool.new_page(self.file)?;
        let halves = match node::split(&node.shape(), &entries, None, (new, self.capacity)) {
            Ok(halves) => halves,
            Err(e) => {
                drop(page);
                // Never linked in, so nothing refers to it.
                let _ = self.pool.free(self.file, new);
                return Err(e);
            }
        };
        splits.push(Split { separator: halves.separator, right: new, level: node.level });
        *page = self.fitted(halves.right, splits)?;
        drop(page);
        count(&self.counters.splits);
        self.fitted(halves.left, splits)
    }

    /// \[DESIGN\] (RULES.md D1271) Gives back the pages of `splits`, those
    /// a split took before it failed, as when its file had no room for
    /// the next. The node split was not written over, so nothing links to
    /// them, and each would stay allocated for good, one already written
    /// holding a leaf's content that a leaf's number noted for a row
    /// could be trusted to (`near.rs`). Each is emptied, so one that
    /// cannot be freed is no node, and freed once no read holds it; a
    /// failure costs its space.
    pub(super) fn give_back(&self, splits: &[Split]) {
        for split in splits {
            if let Ok(mut page) = self.pool.write(self.file, split.right) {
                page.clear();
            }
            // Unlinked and emptied, so a failure leaves only space taken.
            let _ = self.pool.free_when_released(self.file, split.right);
        }
    }
}

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
}

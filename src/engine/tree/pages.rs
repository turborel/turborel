//! \[DESIGN\] The pages a B-link tree takes, as TABLE_STATISTICS and
//! COLUMN_STORAGE show a table's bytes (RULES.md D1165): its nodes above
//! its leaves read, each level by its right links from its leftmost node,
//! and its leaves counted as the nodes of the level above them name them,
//! none read. Of a tree being written, as each level was as it was read.

use std::sync::atomic::Ordering;

use super::{BLinkTree, Node, corrupt};
use crate::disk::pager::PageId;
use crate::error::Result;

impl BLinkTree {
    /// The pages it takes: its meta page, its nodes and its leaves.
    ///
    /// # Errors
    ///
    /// The buffer pool's; SQLCODE -916 if a node is malformed, or its links
    /// name more nodes than its file holds.
    pub fn pages(&self) -> Result<u64> {
        let (most, _) = self.pool.page_count(self.file)?;
        let mut pages = 1_u64;
        let mut leftmost = Some(PageId(self.root.load(Ordering::Acquire)));
        while let Some(first) = leftmost.take() {
            let mut next = Some(first);
            while let Some(at) = next {
                if pages > most {
                    return Err(corrupt("its links name more nodes than its file holds"));
                }
                let page = self.pool.read(self.file, at.0)?;
                let node = Node::parse(&page)?;
                pages = pages.saturating_add(1);
                if node.level == 1 {
                    // Its leaves: its first child and each entry's.
                    pages = pages.saturating_add(u64::try_from(node.len()).unwrap_or(u64::MAX).saturating_add(1));
                } else if node.level > 1 && at == first {
                    leftmost = Some(node.first_child());
                }
                next = node.right;
            }
        }
        Ok(pages)
    }
}

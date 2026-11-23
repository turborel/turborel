//! \[DESIGN\] The nodes a descent read at each level above the one it
//! stopped at (RULES.md D839), kept in place, so a descent allocates
//! nothing. The path is only where a writer looks for a parent first: past
//! its top, or with no node noted for a level, the parent is found again
//! from the root (`tree.rs`, Lehman and Yao 1981, §5). So a path taller than
//! it keeps gives up its top levels, which only costs such a search.

use crate::disk::pager::PageId;

/// \[DESIGN\] The levels a path keeps: more than a tree of 8 KiB pages
/// reaches, which at four entries a node, the fewest, would hold 4^24
/// nodes.
pub(super) const KEPT: usize = 24;

/// A descent's nodes, the top first, the lowest last.
#[derive(Clone, Copy)]
pub(super) struct Levels {
    nodes: [PageId; KEPT],
    /// The nodes kept, at the front of `nodes`.
    len: usize,
}

impl Default for Levels {
    fn default() -> Levels {
        Levels { nodes: [PageId(0); KEPT], len: 0 }
    }
}

impl Levels {
    /// Notes `id` as the node read one level below the last noted. A path
    /// full gives up its top node.
    pub(super) fn push(&mut self, id: PageId) {
        if self.len == KEPT {
            self.nodes.rotate_left(1);
            self.len = KEPT.saturating_sub(1);
        }
        if let Some(slot) = self.nodes.get_mut(self.len) {
            *slot = id;
            self.len = self.len.saturating_add(1);
        }
    }

    /// The lowest node noted, taken off the path.
    pub(super) fn pop(&mut self) -> Option<PageId> {
        self.len = self.len.checked_sub(1)?;
        self.nodes.get(self.len).copied()
    }

    /// The lowest node noted.
    pub(super) fn last(&self) -> Option<&PageId> {
        self.nodes.get(..self.len)?.last()
    }

    /// The nodes noted, the top first.
    #[cfg(test)]
    pub(super) fn as_slice(&self) -> &[PageId] {
        self.nodes.get(..self.len).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D839: a path gives its nodes back lowest
    /// first, and one taller than it keeps gives up its top nodes, keeping
    /// the lowest `KEPT`.
    #[test]
    fn a_path_keeps_its_lowest_levels() {
        let mut path = Levels::default();
        assert_eq!(path.last(), None);
        assert_eq!(path.pop(), None);
        path.push(PageId(1));
        path.push(PageId(2));
        assert_eq!(path.last(), Some(&PageId(2)));
        assert_eq!(path.pop(), Some(PageId(2)));
        assert_eq!(path.as_slice(), [PageId(1)]);
        let mut tall = Levels::default();
        for id in 0..u64::try_from(KEPT).unwrap() + 3 {
            tall.push(PageId(id));
        }
        assert_eq!(tall.as_slice().len(), KEPT);
        assert_eq!(tall.as_slice().first(), Some(&PageId(3)), "the top three given up");
        assert_eq!(tall.pop(), Some(PageId(u64::try_from(KEPT).unwrap() + 2)), "the lowest kept");
    }
}

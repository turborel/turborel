//! \[DESIGN\] Sparse leaves merged into their right neighbour (RULES.md
//! D814), after Lanin and Shasha, "A Symmetric Concurrent B-Tree
//! Algorithm", FJCC 1986, whose merge moves a node's keys into its right
//! neighbour as its range goes there.
//!
//! A table's rows take new ids at the right edge of its tree, so a row
//! deleted and inserted again leaves the leaf it was in for good. Taking
//! out only the leaves deletes empty leaves the rest sparse for ever, the
//! tree's live entries spread over ever more pages. So a delete that
//! leaves its leaf at most half of `MERGED` of a page full tries to merge
//! it: if its entries and its right neighbour's fit in `MERGED` of a
//! page, they all go to the right neighbour, and the leaf is taken out as
//! an emptied one is (`reclaim.rs`), its range given to the neighbour
//! that now holds its keys.
//!
//! Keys move only right, as in a split. The right neighbour is written
//! with both leaves' entries before the leaf is marked deleted, under the
//! locks of both, so no writer changes either between, and a reader sees
//! each entry in one of them or both, with the same value. A scan forward
//! takes from each leaf only the keys past the last it returned, and one
//! backward only those up to the key below the last leaf it read, so it
//! returns none twice. A read that reaches the leaf after it is marked
//! deleted moves right to where its keys went, as past a split.
//!
//! A merged leaf filled to `MERGED` keeps room for inserts and for values
//! that grow before it splits again, and two leaves just split, each
//! about half full, together are past `MERGED`, so a split is not undone
//! at once.

use super::BLinkTree;
use super::node::{self, HEADER, Node};
use crate::error::Result;

/// \[DESIGN\] The most of its page a leaf merged into its right neighbour
/// may fill with the entries of both, as a fraction (RULES.md D814): three
/// quarters, by a sweep of a half, two thirds, three quarters and seven
/// eighths on a table whose rows are deleted and inserted again.
pub(super) const MERGED: (usize, usize) = (3, 4);

/// What a merge of a leaf into its right neighbour makes of the neighbour.
pub(super) enum Merging {
    /// Nothing: the leaf is empty, so the neighbour takes only its range.
    Range,
    /// This content, the entries of both, the neighbour's shape kept.
    Content(Vec<u8>),
    /// Nothing: the entries of both fill more than `MERGED` of a page.
    Full,
}

impl BLinkTree {
    /// Whether leaf `node`, after a delete, is sparse enough to try to
    /// merge into its right neighbour: empty, or at most half of `MERGED`
    /// of a page full.
    pub(super) fn underfull(&self, node: &Node<'_>) -> bool {
        node.len() == 0 || node.live().saturating_mul(2).saturating_mul(MERGED.1) <= self.capacity.saturating_mul(MERGED.0)
    }

    /// Whether the entries of `leaf` and `right` may fit in `MERGED` of a
    /// page, by the bytes they take less one header and the leaf's high
    /// key, without building them: a merge that cannot is not tried.
    pub(super) fn may_merge(&self, leaf: &Node<'_>, right: &Node<'_>) -> bool {
        let spare = HEADER.saturating_add(leaf.high.map_or(0, <[u8]>::len));
        let least = leaf.live().saturating_add(right.live()).saturating_sub(spare);
        leaf.len() == 0 || !self.past_merged(least)
    }

    /// What merging `leaf` into `right`, its right neighbour, makes of
    /// `right`.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if a node is malformed.
    pub(super) fn merged(&self, leaf: &Node<'_>, right: &Node<'_>) -> Result<Merging> {
        if leaf.len() == 0 {
            return Ok(Merging::Range);
        }
        let mut entries = leaf.entries()?;
        entries.extend(right.entries()?);
        if self.past_merged(node::built_size(right.high.map_or(0, <[u8]>::len), &entries)) {
            return Ok(Merging::Full);
        }
        Ok(Merging::Content(node::built(&right.shape(), &entries, None, None)?))
    }

    /// Whether a node of `bytes` fills more than `MERGED` of a page.
    fn past_merged(&self, bytes: usize) -> bool {
        bytes.saturating_mul(MERGED.1) > self.capacity.saturating_mul(MERGED.0)
    }
}

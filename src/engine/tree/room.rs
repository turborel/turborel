//! \[DESIGN\] Room in a node for a change in place (RULES.md D500, D31).
//! An entry removed, or replaced by a longer one, leaves its bytes in the
//! node's heap as garbage, and only building the node again reclaims
//! them. A node full of live entries, whose entries are replaced or
//! removed and added again, as a row's versions and its index entries
//! are, would be built again at nearly every write, each change leaving a
//! little more garbage than the room the next one needs. So garbage is
//! reclaimed only once it is a share of the page worth a node built
//! again, and a node short of that with no room splits, as one full of
//! live entries does; its halves have room for many writes before either
//! is built again or splits.

use super::node::{self, Node};
use super::{BLinkTree, NodeLock, Slot, starts_run};
use crate::error::Result;

/// \[DESIGN\] The share of its page a node's garbage must reach before a
/// write that needs the room builds the node again rather than splitting
/// it: an eighth, so a node is built again at most once per eighth of a
/// page of entries replaced or removed (`BLinkTree::reclaims`).
const RECLAIMED_SHARE: usize = 8;
/// \[DESIGN\] The fewest entries a node splits with rather than being
/// built again: a split needs two entries a side, the separator's and one
/// past it.
const FEWEST_SPLIT: usize = 4;

/// What a change to a node needs of it (`BLinkTree::change`).
pub(super) struct Room {
    /// The bytes its live entries take once it is changed.
    pub(super) after: usize,
    /// The bytes the entry adds to its page, appended.
    pub(super) stored: usize,
    /// Whether the change starts a run of inserts, or splits the node early.
    pub(super) run: bool,
    /// Whether its garbage is reclaimed for the room (`reclaims`).
    pub(super) reclaim: bool,
    /// Whether building it again may give it room: its garbage reclaimed,
    /// or its prefix changed, shorter for the key or longer as keys added
    /// since it was built share more.
    pub(super) rebuild: bool,
}

impl BLinkTree {
    /// What `slot`'s change of `key` to `value` needs of node `node`,
    /// owned as `held`; none if the value is written over in place, of
    /// its length or shorter.
    pub(super) fn room(&self, held: &NodeLock<'_>, node: &Node<'_>, (key, value): (&[u8], &[u8]), slot: Slot) -> Result<Option<Room>> {
        let stored = node::stored(node, key, value);
        // The node's keys grow by what its prefix loses, if the key does
        // not begin with it.
        let grown = matches!(slot, Slot::Insert(_)).then(|| node::grown(node, key)).unwrap_or(0);
        let after = match slot {
            Slot::Insert(_) => node.live().saturating_add(grown).saturating_add(stored),
            Slot::Replace(at) => {
                let old = node.value(at)?;
                if old.len() >= value.len() {
                    return Ok(None);
                }
                node.live().saturating_sub(node::stored(node, key, old)).saturating_add(stored)
            }
        };
        let reclaim = self.reclaims(node);
        // An insert that shortens the prefix builds the node again anyway.
        let rebuild = reclaim || grown > 0 || (matches!(slot, Slot::Insert(_)) && node::lengthens(node, key)?);
        let run = starts_run(node, slot)? || self.crowded(held, node, slot);
        Ok(Some(Room { after, stored, run, reclaim, rebuild }))
    }

    /// \[DESIGN\] Whether node `node`'s garbage is reclaimed, by building it
    /// again, when an entry needs the room: once it is `RECLAIMED_SHARE`
    /// of the page, or the node has too few entries to split. A node built
    /// again for less would be built again at nearly every write once
    /// full, each replaced or removed entry leaving a little more.
    pub(super) fn reclaims(&self, node: &Node<'_>) -> bool {
        node.garbage().saturating_mul(RECLAIMED_SHARE) >= self.capacity || node.len() < FEWEST_SPLIT
    }

    /// Whether an entry of `key`, `added` bytes whole, fits the node `page`
    /// holds once it is built again, its garbage gone and its prefix what
    /// its first and last keys now share (RULES.md D500); if it does, the
    /// node is left built again.
    pub(super) fn fits_compacted(&self, page: &mut Vec<u8>, key: &[u8], added: usize) -> Result<bool> {
        let compacted = node::compact(page)?;
        let grown = node::grown(&Node::parse(&compacted)?, key);
        let fits = compacted.len().saturating_add(grown).saturating_add(added) <= self.capacity;
        if fits {
            *page = compacted;
        }
        Ok(fits)
    }
}

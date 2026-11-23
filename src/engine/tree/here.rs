//! \[DESIGN\] A write made under the one latch its leaf is read under
//! (RULES.md D852). A writer owns its leaf (`disk/buffer/owned.rs`), so no
//! other writer changes it between the writer's read and its write. The
//! read took the leaf's latch shared, searched it, copied out the value
//! found and let the latch go, and the write then took the latch again,
//! exclusively, and parsed the node again. The leaf is now latched once,
//! exclusively: it is searched, the value found is compared where it lies,
//! and the change is made, all under that latch.
//!
//! A leaf that no longer covers the key, split or taken out since its
//! page was found, is left as it is, and the writer moves right from it
//! (Lehman and Yao 1981, §5) and tries the leaf it reaches.
//!
//! A change that asks its caller what to do with the value found
//! (`BLinkTree::update_alone`), and a delete, are made the same way
//! (RULES.md D856): the caller is shown the value where it lies, under
//! the latch, and copies none.

use super::node::Node;
use super::write::Mode;
use super::{BLinkTree, NodeLock, Slot, Split};
use crate::disk::buffer::PageMut;
use crate::error::Result;

/// What a write found in the node it looked in.
enum Sought {
    /// Nothing: the node does not cover the key, which is to its right.
    Elsewhere,
    /// An entry the write's mode refuses, or none where it needs one.
    Refused,
    /// The slot to change, the value there if the mode gives it back, and
    /// whether an insert goes past the end of the rightmost leaf.
    At(Slot, Option<Vec<u8>>, bool),
}

/// What a write did in the node it owns.
pub(super) enum Here {
    /// Nothing: the node does not cover the key, which is to its right.
    Elsewhere,
    /// Nothing: the key was present for an insert, absent for a replace,
    /// or its value not the one expected.
    Refused,
    /// The change: the value a `Mode::Replace` replaced, whether an insert
    /// went past the end of the rightmost leaf, and the splits to post.
    Done(Option<Vec<u8>>, bool, Vec<Split>),
}

/// What a write of `key` by `mode` finds in `node`. A value to compare is
/// compared where it lies, and only a `Mode::Replace` copies the old one.
fn sought(node: &Node<'_>, key: &[u8], mode: Mode<'_>) -> Result<Sought> {
    if node.deleted || !node.covers(key) {
        return Ok(Sought::Elsewhere);
    }
    Ok(match (node.search(key)?, mode) {
        (Ok(at), Mode::Replace) => Sought::At(Slot::Replace(at), Some(node.value(at)?.to_vec()), false),
        (Ok(at), Mode::ReplaceIf(expected)) => {
            if node.value(at)? == expected {
                Sought::At(Slot::Replace(at), None, false)
            } else {
                Sought::Refused
            }
        }
        (Err(at), Mode::Insert) => Sought::At(Slot::Insert(at), None, node.level == 0 && node.right.is_none() && at == node.len()),
        (Ok(_), Mode::Insert) | (Err(_), Mode::Replace | Mode::ReplaceIf(_)) => Sought::Refused,
    })
}

impl BLinkTree {
    /// The node covering `key`, latched for writing: `held`, or the node
    /// reached by moving right from it while it does not cover `key`,
    /// which `held` then is (§5, move.right).
    pub(super) fn latched_covering<'a>(&'a self, held: &mut NodeLock<'a>, key: &[u8]) -> Result<PageMut<'a>> {
        loop {
            let page = self.pool.write_owned(held)?;
            let node = Node::parse(&page)?;
            if !node.deleted && node.covers(key) {
                return Ok(page);
            }
            drop(page);
            self.move_right(held, key, |_| Ok(()))?;
        }
    }

    /// Writes `key` with `value` as `mode` says into the node `held`, if
    /// it covers `key`: searched and changed under one latch, split if the
    /// change does not fit.
    pub(super) fn write_here(&self, held: &NodeLock<'_>, (key, value): (&[u8], &[u8]), mode: Mode<'_>) -> Result<Here> {
        let (copy, slot, old, past_the_end) = {
            let mut page = self.pool.write_owned(held)?;
            let (slot, old, past_the_end) = match sought(&Node::parse(&page)?, key, mode)? {
                Sought::Elsewhere => return Ok(Here::Elsewhere),
                Sought::Refused => return Ok(Here::Refused),
                Sought::At(slot, old, past_the_end) => (slot, old, past_the_end),
            };
            let Some(copy) = self.change_latched(held, &mut page, (key, value), slot)? else {
                return Ok(Here::Done(old, past_the_end, Vec::new()));
            };
            (copy, slot, old, past_the_end)
        };
        let splits = self.split(held.id, &copy, (key, value), slot)?;
        Ok(Here::Done(old, past_the_end, splits))
    }
}

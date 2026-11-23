//! \[DESIGN\] Writes that tell where they left their entry, and a change
//! in place that starts there with no descent (RULES.md D843). A commit
//! stamps each row it wrote, which took a descent of the row's tree to
//! read the entry and another to write it. The write that made the entry
//! already held its leaf, so it hands the leaf on, with the tree's stamp
//! read before it (`reclaim.rs`). While the stamp is unchanged no leaf has
//! been retired, so the leaf is still a leaf of this tree, split or not,
//! and the entry is in it or to its right, as a key moves only right with
//! a split (Lehman and Yao 1981, §4). A changed stamp, or an entry not
//! found from the leaf, sends the change down from the root.

use super::BLinkTree;
use super::node::{self, Node};
use super::write::{Mode, Written};
use crate::disk::pager::PageId;
use crate::error::Result;

/// \[DESIGN\] Where a write left its entry: the leaf, and the tree's stamp
/// before the write. Leaf 0, the pager's own page, is none.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Placed {
    /// The leaf's page number.
    pub leaf: u64,
    /// The tree's stamp before the write.
    pub stamp: u64,
}

impl Placed {
    /// Whether it names a leaf.
    #[must_use]
    pub fn is_some(self) -> bool {
        self.leaf != 0
    }
}

impl BLinkTree {
    /// As `insert`, and where the entry went: none if `key` was there.
    ///
    /// # Errors
    ///
    /// As `insert`.
    pub fn insert_placed(&self, key: &[u8], value: &[u8]) -> Result<Option<Placed>> {
        let stamp = self.epochs.stamp();
        Ok(match self.write(key, value, Mode::Insert)? {
            Written::Done(_, into) => Some(Placed { leaf: into.0, stamp }),
            Written::Refused => None,
        })
    }

    /// As `replace_found`, and where the entry went: none if it was no
    /// longer `expected`.
    ///
    /// # Errors
    ///
    /// As `insert`.
    pub fn replace_found_placed(&self, found: &super::Descended, key: &[u8], (expected, value): (&[u8], &[u8])) -> Result<Option<Placed>> {
        let _guard = self.epochs.guard();
        let stamp = self.epochs.stamp();
        let written = if stamp == found.stamp {
            self.fits(key, value)?;
            self.write_from(&found.path, found.leaf, (key, value), Mode::ReplaceIf(expected))?
        } else {
            self.write(key, value, Mode::ReplaceIf(expected))?
        };
        Ok(match written {
            Written::Done(_, into) => Some(Placed { leaf: into.0, stamp }),
            Written::Refused => None,
        })
    }

    /// Changes `key`'s value in place, from the leaf `placed` names, as
    /// `change` says under the leaf's lock, keeping the value's length;
    /// `change`'s answer, or none if the leaf may have left the tree or
    /// holds no `key` in reach, which the caller then changes by a descent.
    /// `change` must not use the tree.
    ///
    /// # Errors
    ///
    /// `change`'s errors; the buffer pool's; SQLCODE -916 if a node is
    /// malformed.
    pub fn change_placed<R>(&self, placed: Placed, key: &[u8], change: impl FnOnce(&mut [u8]) -> Result<R>) -> Result<Option<R>> {
        let _guard = self.epochs.guard();
        if !placed.is_some() || self.epochs.stamp() != placed.stamp {
            return Ok(None);
        }
        let mut held = self.lock(PageId(placed.leaf))?;
        // Searched and changed under one latch, as a write is (`here.rs`).
        loop {
            let mut page = self.pool.write_owned(&held)?;
            let node = Node::parse(&page)?;
            if node.deleted || !node.covers(key) {
                drop(page);
                self.move_right(&mut held, key, |_| Ok(()))?;
                continue;
            }
            let Ok(at) = node.search(key)? else { return Ok(None) };
            return change(node::value_mut(&mut page, at)?).map(Some);
        }
    }
}

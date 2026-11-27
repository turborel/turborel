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

use super::node::{self, Node};
use super::write::{Mode, Written};
use super::{BLinkTree, Path, Slot};
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

    /// \[DESIGN\] Replaces `key`'s value with the one `f` gives, shown the
    /// value there where it lies, under one exclusive hold of the leaf
    /// `found` reached (RULES.md D866): `f` may copy the old value
    /// elsewhere, and gives the bytes to put, or none to leave the entry.
    /// Where the entry went; none if `key` is no longer in the tree, `f`
    /// then not called, or `f` left it. A leaf retired since the read is
    /// found again by a descent, as `replace_found` does. `f` must not use
    /// the tree nor wait.
    ///
    /// # Errors
    ///
    /// `f`'s errors; as `insert`.
    pub fn replace_found_with<'v>(
        &self,
        found: &super::Descended,
        key: &[u8],
        f: impl FnOnce(&[u8]) -> Result<Option<&'v [u8]>>,
    ) -> Result<Option<Placed>> {
        let _guard = self.epochs.guard();
        let stamp = self.epochs.stamp();
        let mut descended = Path::default();
        let leaf = if stamp == found.stamp { found.leaf } else { self.leaf_for_write(key, &mut descended)? };
        let path = if stamp == found.stamp { &found.path } else { &descended };
        let mut held = self.lock(leaf)?;
        let mut page = self.latched_covering(&mut held, key)?;
        let (slot, value) = {
            let node = Node::parse(&page)?;
            let Ok(at) = node.search(key)? else { return Ok(None) };
            let Some(value) = f(node.value(at)?)? else { return Ok(None) };
            (Slot::Replace(at), value)
        };
        self.fits(key, value)?;
        let copy = self.change_latched(&held, &mut page, (key, value), slot)?;
        drop(page);
        let splits = match copy {
            Some(copy) => self.split(held.id, &copy, (key, value), slot)?,
            None => Vec::new(),
        };
        let into = self.posted(path, held, key, splits)?;
        // A table's row: its leaf noted, as a write notes it (`hints.rs`).
        if let Some(row) = self.row_of(key) {
            super::hints::note(row, into.0);
        }
        Ok(Some(Placed { leaf: into.0, stamp }))
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
        let mut page = self.latched_covering(&mut held, key)?;
        let Ok(at) = Node::parse(&page)?.search(key)? else { return Ok(None) };
        change(node::value_mut(&mut page, at)?).map(Some)
    }

    /// \[DESIGN\] As `change_placed`, of each of `keys`, ascending, that
    /// the leaf `placed` names holds, under one hold of the leaf (RULES.md
    /// D924): a commit's rows a statement left in one leaf are stamped
    /// with the leaf latched once, not once a row. `change` is given each
    /// key's place among `keys` and its value. How many of `keys`, from
    /// the first, were changed there. The first not changed is not in the
    /// node that covers the first, which the caller then changes alone.
    /// None of them if the leaf may have left the tree. `change` must not
    /// use the tree.
    ///
    /// # Errors
    ///
    /// `change`'s errors; the buffer pool's; SQLCODE -916 if a node is
    /// malformed.
    pub fn change_placed_run<K: AsRef<[u8]>>(
        &self,
        placed: Placed,
        keys: &[K],
        mut change: impl FnMut(usize, &mut [u8]) -> Result<()>,
    ) -> Result<usize> {
        let _guard = self.epochs.guard();
        let Some(first) = keys.first().filter(|_| placed.is_some() && self.epochs.stamp() == placed.stamp) else { return Ok(0) };
        let mut held = self.lock(PageId(placed.leaf))?;
        let mut page = self.latched_covering(&mut held, first.as_ref())?;
        for (n, key) in keys.iter().enumerate() {
            let found = {
                let node = Node::parse(&page)?;
                if node.covers(key.as_ref()) { node.search(key.as_ref())?.ok() } else { None }
            };
            let Some(at) = found else { return Ok(n) };
            change(n, node::value_mut(&mut page, at)?)?;
        }
        Ok(keys.len())
    }
}

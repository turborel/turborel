//! \[DESIGN\] Reads and writes of one key of a B-link tree (RULES.md
//! D31): a writer goes down locking nothing, locks the leaf, moves right
//! past splits, and changes it in place or splits it, posting the split up
//! the tree (`tree.rs`, §5).

use super::locks::NodeLock;
use super::node::{self, CHILD, Node};
use super::{BLinkTree, Path, Slot, Target};
use crate::disk::pager::PageId;
use crate::error::{Code, Result, SqlError};

/// What a write asks of the entry it finds.
#[derive(Clone, Copy)]
pub(super) enum Mode<'a> {
    /// None may be there.
    Insert,
    /// One must be there.
    Replace,
    /// One must be there, with this value.
    ReplaceIf(&'a [u8]),
}

/// \[DESIGN\] What `update` does to an entry.
pub enum Update {
    /// Leaves it as it is, or absent.
    Keep,
    /// Gives it this value, adding it if absent.
    Put(Vec<u8>),
    /// Removes it, if there.
    Remove,
}

/// What a write did.
pub(super) enum Written {
    /// Nothing: the key was present for an insert, absent for a replace.
    Refused,
    /// The change, and the value replaced, if any.
    Done(Option<Vec<u8>>),
}

impl BLinkTree {
    /// The value of `key`, if it is in the tree.
    ///
    /// # Errors
    ///
    /// The buffer pool's errors; SQLCODE -916 if a node is malformed.
    pub fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>> {
        let _guard = self.epochs.guard();
        let (_, value) = self.descend(Target::Key(key), 0, &mut Path::default(), |leaf| match leaf.search(key)? {
            Ok(at) => Ok(Some(leaf.entry(at)?.1.to_vec())),
            Err(_) => Ok(None),
        })?;
        Ok(value)
    }

    /// Adds `key` with `value`; whether it did: not if `key` is already in
    /// the tree, which is left as it was.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the entry is longer than `max_entry`; as `get`.
    pub fn insert(&self, key: &[u8], value: &[u8]) -> Result<bool> {
        Ok(matches!(self.write(key, value, Mode::Insert)?, Written::Done(_)))
    }

    /// Replaces the value of `key` with `value`; the value replaced, read
    /// under the leaf's lock, so two writers never both see it: none if
    /// `key` is not in the tree.
    ///
    /// # Errors
    ///
    /// As `insert`.
    pub fn replace(&self, key: &[u8], value: &[u8]) -> Result<Option<Vec<u8>>> {
        Ok(match self.write(key, value, Mode::Replace)? {
            Written::Done(old) => old,
            Written::Refused => None,
        })
    }

    /// Replaces the value of `key` with `value` if it is still `expected`,
    /// compared under the leaf's lock; whether it did.
    ///
    /// # Errors
    ///
    /// As `insert`.
    pub fn replace_if(&self, key: &[u8], expected: &[u8], value: &[u8]) -> Result<bool> {
        Ok(matches!(self.write(key, value, Mode::ReplaceIf(expected))?, Written::Done(_)))
    }

    /// Removes `key` if its value is still `expected`, compared under the
    /// leaf's lock; whether it did.
    ///
    /// # Errors
    ///
    /// As `get`.
    pub fn delete_if(&self, key: &[u8], expected: &[u8]) -> Result<bool> {
        let _guard = self.epochs.guard();
        let mut path = Path::default();
        let (leaf, ()) = self.descend(Target::Key(key), 0, &mut path, |_| Ok(()))?;
        Ok(self.delete_from(&path, leaf, key, Some(expected))?.is_some())
    }

    /// Removes `key`; its value, read under the leaf's lock: none if it was
    /// not in the tree. A leaf it empties is taken out (`reclaim.rs`).
    ///
    /// # Errors
    ///
    /// As `get`.
    pub fn delete(&self, key: &[u8]) -> Result<Option<Vec<u8>>> {
        let _guard = self.epochs.guard();
        let mut path = Path::default();
        let (leaf, ()) = self.descend(Target::Key(key), 0, &mut path, |_| Ok(()))?;
        self.delete_from(&path, leaf, key, None)
    }

    /// The rest of `delete`, from `leaf`, reached by `path`, which may have
    /// split since it was read; only if the value is `expected`, if given.
    pub(super) fn delete_from(&self, path: &Path, leaf: PageId, key: &[u8], expected: Option<&[u8]>) -> Result<Option<Vec<u8>>> {
        let mut held = self.locks.lock(leaf)?;
        let found = self.move_right(&mut held, key, |node| match node.search(key)? {
            Ok(at) => Ok(Some((at, node.entry(at)?.1.to_vec()))),
            Err(_) => Ok(None),
        })?;
        let Some((at, old)) = found.filter(|(_, old)| expected.is_none_or(|e| e == old.as_slice())) else { return Ok(None) };
        self.remove_at(path, held, at)?;
        Ok(Some(old))
    }

    /// Removes entry `at` of the leaf `held`, reached by `path`; takes the
    /// leaf out if that empties it, once its lock is released.
    fn remove_at(&self, path: &Path, held: NodeLock<'_>, at: usize) -> Result<()> {
        let leaf = held.id;
        let emptied = {
            let mut page = self.pool.write(self.file, leaf)?;
            node::remove(&mut page, at)?;
            Node::parse(&page)?.len() == 0
        };
        drop(held);
        if emptied { self.unlink(leaf, path) } else { Ok(()) }
    }

    /// Changes `key`'s entry as `change` says, given its value, or none if
    /// there is none: in one descent, `change` called under the leaf's
    /// lock, so no other writer changes the entry in between. `change` must
    /// not use the tree.
    ///
    /// # Errors
    ///
    /// `change`'s errors; as `insert`.
    pub fn update(&self, key: &[u8], change: impl FnOnce(Option<&[u8]>) -> Result<Update>) -> Result<()> {
        self.update_alone(key, 0, change).map(drop)
    }

    /// \[DESIGN\] As `update`, and whether, under the leaf's lock, no
    /// other entry of the tree begins with `key`'s first `prefix` bytes:
    /// found of the entries beside its place in its leaf; so not if its
    /// place is first in the leaf, the leaf before beyond reach, nor if it
    /// is last and the leaf's high key begins with them, an entry of the
    /// leaf after then possibly so (`Index::insert_alone`).
    ///
    /// # Errors
    ///
    /// As `update`.
    pub fn update_alone(&self, key: &[u8], prefix: usize, change: impl FnOnce(Option<&[u8]>) -> Result<Update>) -> Result<bool> {
        let _guard = self.epochs.guard();
        let mut path = Path::default();
        let (leaf, ()) = self.descend(Target::Key(key), 0, &mut path, |_| Ok(()))?;
        let mut held = self.locks.lock(leaf)?;
        let begins = key.get(..prefix).unwrap_or(key);
        let (found, alone) = self.move_right(&mut held, key, |node| {
            let found = match node.search(key)? {
                Ok(at) => Ok((at, node.entry(at)?.1.to_vec())),
                Err(at) => Err(at),
            };
            let (before, after) = match &found {
                Ok((at, _)) => (at.checked_sub(1), at.saturating_add(1)),
                Err(at) => (at.checked_sub(1), *at),
            };
            let shares = |at: usize| node.key(at).map(|k| k.starts_with(begins));
            let none_before = before.map(shares).transpose()?.is_some_and(|s| !s);
            let none_after = if after < node.len() { !shares(after)? } else { node.high.is_none_or(|h| !h.starts_with(begins)) };
            Ok((found, none_before && none_after))
        })?;
        let (slot, value) = match (change(found.as_ref().ok().map(|(_, old)| old.as_slice()))?, found) {
            (Update::Keep, _) | (Update::Remove, Err(_)) => return Ok(alone),
            (Update::Remove, Ok((at, _))) => return self.remove_at(&path, held, at).map(|()| alone),
            (Update::Put(value), Ok((at, _))) => (Slot::Replace(at), value),
            (Update::Put(value), Err(at)) => (Slot::Insert(at), value),
        };
        self.fits(key, &value)?;
        self.place(path, held, (key, &value), slot).map(|()| alone)
    }

    /// Refuses an entry longer than `max_entry`.
    fn fits(&self, key: &[u8], value: &[u8]) -> Result<()> {
        if key.len().saturating_add(value.len().max(CHILD)) > self.max_entry() {
            return Err(SqlError::new(
                Code::Api,
                format!(
                    "[DESIGN] an entry of {} bytes is longer than a B-link tree's {} bytes",
                    key.len().saturating_add(value.len()),
                    self.max_entry()
                ),
            ));
        }
        Ok(())
    }

    /// Inserts or replaces `key`'s entry, as `mode` says (§5, insert).
    fn write(&self, key: &[u8], value: &[u8], mode: Mode<'_>) -> Result<Written> {
        let _guard = self.epochs.guard();
        self.fits(key, value)?;
        let mut path = Path::default();
        let (leaf, ()) = self.descend(Target::Key(key), 0, &mut path, |_| Ok(()))?;
        self.write_from(path, leaf, (key, value), mode)
    }

    /// The rest of `write`, from `leaf`, reached by `path`: either may be
    /// out of date, the leaf split or the tree grown since they were read.
    pub(super) fn write_from(&self, path: Path, leaf: PageId, (key, value): (&[u8], &[u8]), mode: Mode<'_>) -> Result<Written> {
        let mut held = self.locks.lock(leaf)?;
        let replace = !matches!(mode, Mode::Insert);
        let found = self.move_right(&mut held, key, |node| match node.search(key)? {
            Ok(at) => Ok(Ok((at, if replace { node.entry(at)?.1.to_vec() } else { Vec::new() }))),
            Err(at) => Ok(Err(at)),
        })?;
        let (slot, old) = match (found, mode) {
            (Ok((at, old)), Mode::Replace) => (Slot::Replace(at), Some(old)),
            (Ok((at, old)), Mode::ReplaceIf(expected)) => {
                if old != expected {
                    return Ok(Written::Refused);
                }
                (Slot::Replace(at), Some(old))
            }
            (Err(at), Mode::Insert) => (Slot::Insert(at), None),
            (Ok(_), Mode::Insert) | (Err(_), Mode::Replace | Mode::ReplaceIf(_)) => return Ok(Written::Refused),
        };
        self.place(path, held, (key, value), slot)?;
        Ok(Written::Done(old))
    }

    /// Makes `slot`'s change in the leaf `held`, reached by `path`, and
    /// posts the splits it causes up the tree.
    pub(super) fn place<'a>(&'a self, mut path: Path, mut held: NodeLock<'a>, (key, value): (&[u8], &[u8]), slot: Slot) -> Result<()> {
        let mut split = self.change(&held, key, value, slot)?;
        while let Some(done) = split {
            let Some((parent, at)) = self.lock_parent(&mut path, &done)? else { break };
            // The child's lock is released only now, with the parent's
            // held and moved right (§5).
            held = parent;
            split = self.change(&held, &done.separator, &done.right.0.to_le_bytes(), Slot::Insert(at))?;
        }
        Ok(())
    }
}

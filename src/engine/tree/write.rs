//! \[DESIGN\] Reads and writes of one key of a B-link tree (RULES.md
//! D31): a writer goes down locking nothing, locks the leaf, moves right
//! past splits, and changes it in place or splits it, posting the split up
//! the tree (`tree.rs`, §5).

use super::node::{self, CHILD, Node};
use super::{BLinkTree, Path, Slot, Split, Target};
use crate::disk::buffer::Owned as NodeLock;
use crate::disk::pager::PageId;
use crate::error::{Code, Result, SqlError};
use crate::storage::Near;

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
    /// The change: the value replaced, if any, and the leaf the entry is in.
    Done(Option<Vec<u8>>, PageId),
}

impl BLinkTree {
    /// The value of `key`, if it is in the tree.
    ///
    /// # Errors
    ///
    /// The buffer pool's errors; SQLCODE -916 if a node is malformed.
    pub fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>> {
        let _guard = self.epochs.guard();
        let (_, value) =
            self.descend(Target::Key(key), 0, &mut Path { tallied: true, ..Path::default() }, |leaf| match leaf.search(key)? {
                Ok(at) => Ok(Some(leaf.entry(at)?.1.to_vec())),
                Err(_) => Ok(None),
            })?;
        Ok(value)
    }

    /// \[DESIGN\] As `get`, for a write over the value found
    /// (`replace_found`), which then starts from the leaf this descent
    /// reached (RULES.md D446).
    ///
    /// # Errors
    ///
    /// As `get`.
    pub fn get_for_write(&self, key: &[u8]) -> Result<Option<(Vec<u8>, Descended)>> {
        let _guard = self.epochs.guard();
        let stamp = self.epochs.stamp();
        let mut path = Path { tallied: true, ..Path::default() };
        let (leaf, value) = self.descend(Target::Key(key), 0, &mut path, |leaf| match leaf.search(key)? {
            Ok(at) => Ok(Some(leaf.entry(at)?.1.to_vec())),
            Err(_) => Ok(None),
        })?;
        path.tallied = false;
        Ok(value.map(|value| (value, Descended { path, leaf, stamp })))
    }

    /// As `replace_if`, from where `get_for_write`'s descent reached: the
    /// leaf it found is still in the tree while no leaf was retired since
    /// (`reclaim.rs`), split or not, which the write moves right across;
    /// else a descent of its own.
    ///
    /// # Errors
    ///
    /// As `insert`.
    pub fn replace_found(&self, found: &Descended, key: &[u8], (expected, value): (&[u8], &[u8])) -> Result<bool> {
        let _guard = self.epochs.guard();
        if self.epochs.stamp() != found.stamp {
            return self.replace_if(key, expected, value);
        }
        self.fits(key, value)?;
        Ok(matches!(self.write_from(&found.path, found.leaf, (key, value), Mode::ReplaceIf(expected))?, Written::Done(..)))
    }

    /// As `replace_if`, from the leaf `near` holds after a `get_near` of
    /// `key` (`near.rs`): it is still in the tree while no leaf was retired
    /// since, split or not, which the write moves right across; else a
    /// descent of its own. A leaf left by a read of another key may not
    /// hold `key`, so a write refused from it descends too.
    ///
    /// # Errors
    ///
    /// As `insert`.
    pub fn replace_if_near(&self, key: &[u8], (expected, value): (&[u8], &[u8]), near: &Near) -> Result<bool> {
        let _guard = self.epochs.guard();
        let Some((leaf, _)) = near.leaf.filter(|&(_, stamp)| stamp == self.epochs.stamp()) else {
            return self.replace_if(key, expected, value);
        };
        self.fits(key, value)?;
        let Written::Done(_, into) = self.write_from(&Path::default(), PageId(leaf), (key, value), Mode::ReplaceIf(expected))? else {
            return self.replace_if(key, expected, value);
        };
        if let Some(row) = self.row_of(key) {
            super::hints::note(row, into.0);
        }
        Ok(true)
    }

    /// Adds `key` with `value`; whether it did: not if `key` is already in
    /// the tree, which is left as it was.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the entry is longer than `max_entry`; as `get`.
    pub fn insert(&self, key: &[u8], value: &[u8]) -> Result<bool> {
        Ok(matches!(self.write(key, value, Mode::Insert)?, Written::Done(..)))
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
            Written::Done(old, _) => old,
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
        Ok(matches!(self.write(key, value, Mode::ReplaceIf(expected))?, Written::Done(..)))
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
        let leaf = self.leaf_for_write(key, &mut path)?;
        Ok(self.delete_from(&path, leaf, key, Some(expected))?.is_some())
    }

    /// Removes `key`; its value, read under the leaf's lock: none if it was
    /// not in the tree. A leaf it empties, or leaves sparse, is taken out
    /// (`reclaim.rs`, `merge.rs`).
    ///
    /// # Errors
    ///
    /// As `get`.
    pub fn delete(&self, key: &[u8]) -> Result<Option<Vec<u8>>> {
        let _guard = self.epochs.guard();
        let mut path = Path::default();
        let leaf = self.leaf_for_write(key, &mut path)?;
        self.delete_from(&path, leaf, key, None)
    }

    /// The rest of `delete`, from `leaf`, reached by `path`, which may have
    /// split since it was read; only if the value is `expected`, if given.
    pub(super) fn delete_from(&self, path: &Path, leaf: PageId, key: &[u8], expected: Option<&[u8]>) -> Result<Option<Vec<u8>>> {
        let mut held = self.lock(leaf)?;
        let found = self.move_right(&mut held, key, |node| match node.search(key)? {
            Ok(at) => Ok(Some((at, node.entry(at)?.1.to_vec()))),
            Err(_) => Ok(None),
        })?;
        let Some((at, old)) = found.filter(|(_, old)| expected.is_none_or(|e| e == *old)) else { return Ok(None) };
        self.remove_at(path, held, at)?;
        Ok(Some(old))
    }

    /// Removes entry `at` of the leaf `held`, reached by `path`; takes the
    /// leaf out, its entries merged into its right neighbour, if that
    /// leaves it empty or sparse (`merge.rs`), once its lock is released.
    fn remove_at(&self, path: &Path, held: NodeLock<'_>, at: usize) -> Result<()> {
        let leaf = held.id;
        let sparse = {
            let mut page = self.pool.write_owned(&held)?;
            node::remove(&mut page, at)?;
            self.underfull(&Node::parse(&page)?)
        };
        drop(held);
        if sparse { self.unlink(leaf, path) } else { Ok(()) }
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
        let leaf = self.leaf_for_write(key, &mut path)?;
        let mut held = self.lock(leaf)?;
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
        self.place(&path, held, (key, &value), slot).map(|_| alone)
    }

    /// The leaf a writer of `key` locks, `path` the way to it: its page
    /// number read from its parent, the leaf itself not read, as the
    /// writer reads it once it owns it, moving right past a split or a
    /// leaf taken out, as from a leaf read (RULES.md D839). A root that is
    /// a leaf is the leaf.
    fn leaf_for_write(&self, key: &[u8], path: &mut Path) -> Result<PageId> {
        path.above_leaf = true;
        let found = self.descend(Target::Key(key), 0, path, |node| match node.level {
            0 => Ok(None),
            _ => node.child(key).map(|(child, _)| Some(child)),
        });
        path.above_leaf = false;
        Ok(match found? {
            (parent, Some(child)) => {
                path.stack.push(parent);
                child
            }
            (leaf, None) => leaf,
        })
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
        let leaf = match self.edge_for(key)? {
            Some(leaf) => leaf,
            None => self.leaf_for_write(key, &mut path)?,
        };
        let written = self.write_from(&path, leaf, (key, value), mode)?;
        // A table's row: its leaf noted for its index entries (`hints.rs`).
        if let (Written::Done(_, into), Some(row)) = (&written, self.row_of(key)) {
            super::hints::note(row, into.0);
        }
        Ok(written)
    }

    /// The rest of `write`, from `leaf`, reached by `path`: either may be
    /// out of date, the leaf split or the tree grown since they were read.
    pub(super) fn write_from(&self, path: &Path, leaf: PageId, (key, value): (&[u8], &[u8]), mode: Mode<'_>) -> Result<Written> {
        let mut held = self.lock(leaf)?;
        let replace = !matches!(mode, Mode::Insert);
        let mut past_the_end = false;
        let found = self.move_right(&mut held, key, |node| match node.search(key)? {
            Ok(at) => Ok(Ok((at, if replace { node.entry(at)?.1.to_vec() } else { Vec::new() }))),
            Err(at) => {
                past_the_end = node.level == 0 && node.right.is_none() && at == node.len();
                Ok(Err(at))
            }
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
        let into = self.place(path, held, (key, value), slot)?;
        if past_the_end {
            self.note_edge(into);
        }
        Ok(Written::Done(old, into))
    }

    /// Makes `slot`'s change in the leaf `held`, reached by `path`, and
    /// posts the splits it causes up the tree. The leaf the entry is in:
    /// the right page of the highest separator it is past, if any.
    pub(super) fn place<'a>(&'a self, path: &Path, held: NodeLock<'a>, (key, value): (&[u8], &[u8]), slot: Slot) -> Result<PageId> {
        let splits = self.change(&held, key, value, slot)?;
        let leaf = splits
            .iter()
            .filter(|s| key > s.separator.as_slice())
            .max_by(|a, b| a.separator.cmp(&b.separator))
            .map_or(held.id, |s| s.right);
        self.post(path, held, splits)?;
        Ok(leaf)
    }

    /// Posts `splits` of node `held`, reached by `path`, to the level
    /// above, and the splits that causes, up the tree. The child's lock is
    /// released only once the first parent's is held and moved right
    /// (§5); a further split of one change, its page reached meanwhile
    /// through the right links, is posted after, as a split past the top
    /// of a path is (`fit.rs`).
    fn post<'a>(&'a self, path: &Path, held: NodeLock<'a>, splits: Vec<Split>) -> Result<()> {
        let mut held = Some(held);
        for done in splits {
            let mut up = Path { stack: path.stack, ..Path::default() };
            let Some((parent, at)) = self.lock_parent(&mut up, &done)? else { continue };
            drop(held.take());
            let more = self.change(&parent, &done.separator, &done.right.0.to_le_bytes(), Slot::Insert(at))?;
            self.post(&up, parent, more)?;
        }
        drop(held);
        Ok(())
    }
}

/// \[DESIGN\] Where `BLinkTree::get_for_write`'s descent found a key: the
/// path to its leaf, the leaf, and the tree's stamp then.
pub struct Descended {
    path: Path,
    leaf: PageId,
    stamp: u64,
}

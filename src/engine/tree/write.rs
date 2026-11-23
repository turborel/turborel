//! \[DESIGN\] Reads and writes of one key of a B-link tree (RULES.md
//! D31): a writer goes down locking nothing, locks the leaf, moves right
//! past splits, and changes it in place or splits it, posting the split up
//! the tree (`tree.rs`, §5).

use std::cell::Cell;

use super::here::Here;
use super::node::{self, CHILD, Node};
use super::{BLinkTree, Path, Slot, Split, Target};
use crate::disk::buffer::{Owned as NodeLock, PageMut};
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

/// \[DESIGN\] What `update_alone_into`'s change does to an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Updated {
    /// Leaves it as it is, or absent.
    Keep,
    /// Gives it the value written into the buffer, adding it if absent.
    Put,
    /// Removes it, if there.
    Remove,
}

/// \[DESIGN\] The most bytes of the buffer a thread keeps for the values
/// its changes put: an entry is at most a quarter of a page, and the
/// largest page is 64 KiB.
const KEPT_BYTES: usize = 65_536;

thread_local! {
    /// The buffer of the thread's last change that put a value.
    static VALUE: Cell<Vec<u8>> = const { Cell::new(Vec::new()) };
}

/// What a write did.
pub(super) enum Written {
    /// Nothing: the key was present for an insert, absent for a replace.
    Refused,
    /// The change: the value a `Mode::Replace` replaced, and the leaf the
    /// entry is in.
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
        let (_, value) = self.descend(Target::Key(key), 0, &mut Path { tallied: true, ..Path::default() }, |leaf| value_of(leaf, key))?;
        Ok(value)
    }

    /// \[DESIGN\] As `get`, for a write over the value found
    /// (`replace_found`), which then starts from the leaf this read
    /// reached (RULES.md D446). A table's row is read from the leaf noted
    /// for it, if that leaf answers, with no descent (`near.rs`, RULES.md
    /// D853): the read of the row before its update noted it.
    ///
    /// # Errors
    ///
    /// As `get`.
    pub fn get_for_write(&self, key: &[u8]) -> Result<Option<(Vec<u8>, Descended)>> {
        let mut value = Vec::new();
        Ok(self.get_for_write_into(key, &mut value)?.map(|found| (value, found)))
    }

    /// \[DESIGN\] As `get_for_write`, the value found left in `into`,
    /// cleared first, so a caller that keeps a buffer from write to write
    /// allocates none for the value (RULES.md D865).
    ///
    /// # Errors
    ///
    /// As `get`.
    pub fn get_for_write_into(&self, key: &[u8], into: &mut Vec<u8>) -> Result<Option<Descended>> {
        let _guard = self.epochs.guard();
        let stamp = self.epochs.stamp();
        into.clear();
        if let Some((leaf, found)) = self.at_noted(key, |leaf| value_into(leaf, key, into))? {
            return Ok(found.then(|| Descended { path: Path::default(), leaf: PageId(leaf), stamp }));
        }
        let mut path = Path { tallied: true, ..Path::default() };
        let (leaf, found) = self.descend(Target::Key(key), 0, &mut path, |leaf| value_into(leaf, key, into))?;
        path.tallied = false;
        Ok(found.then_some(Descended { path, leaf, stamp }))
    }

    /// As `replace_if`, from where `get_for_write`'s read reached: the
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
    /// split since it was read; only if the value is `expected`, if given,
    /// and then an empty value stands for the one removed.
    /// The leaf is searched, the value compared and the entry removed
    /// under one latch (`here.rs`).
    pub(super) fn delete_from(&self, path: &Path, leaf: PageId, key: &[u8], expected: Option<&[u8]>) -> Result<Option<Vec<u8>>> {
        let mut held = self.lock(leaf)?;
        let page = self.latched_covering(&mut held, key)?;
        let node = Node::parse(&page)?;
        let Ok(at) = node.search(key)? else { return Ok(None) };
        let old = node.value(at)?;
        if expected.is_some_and(|expected| expected != old) {
            return Ok(None);
        }
        // Only a delete of whatever is there gives the value back: one of
        // a value expected has it, and copies none.
        let old = if expected.is_some() { Vec::new() } else { old.to_vec() };
        self.removed(path, held, page, (key, at))?;
        Ok(Some(old))
    }

    /// Removes entry `at`, of `key`, of the leaf `held`, latched for
    /// writing as `page`, reached by `path`; takes the leaf out, its
    /// entries merged into its right neighbour, if that leaves it empty or
    /// sparse (`merge.rs`), once its latch and its lock are released.
    fn removed<'a>(&'a self, path: &Path, held: NodeLock<'a>, mut page: PageMut<'a>, (key, at): (&[u8], usize)) -> Result<()> {
        let leaf = held.id;
        node::remove(&mut page, at)?;
        let sparse = self.underfull(&Node::parse(&page)?);
        drop(page);
        drop(held);
        if sparse { self.unlink((leaf, key), path) } else { Ok(()) }
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
        self.update_alone_into(key, prefix, |old, into| {
            Ok(match change(old)? {
                Update::Keep => Updated::Keep,
                Update::Remove => Updated::Remove,
                Update::Put(value) => {
                    *into = value;
                    Updated::Put
                }
            })
        })
    }

    /// \[DESIGN\] As `update_alone`, `change` writing the value it puts
    /// into the buffer it is given, empty, which the thread keeps from one
    /// such change to the next (RULES.md D865): a change that puts a value
    /// allocates none for it.
    ///
    /// # Errors
    ///
    /// As `update`.
    pub fn update_alone_into(
        &self,
        key: &[u8],
        prefix: usize,
        change: impl FnOnce(Option<&[u8]>, &mut Vec<u8>) -> Result<Updated>,
    ) -> Result<bool> {
        let mut value = VALUE.take();
        value.clear();
        let alone = self.updated_into(key, prefix, &mut value, change);
        if value.capacity() <= KEPT_BYTES {
            VALUE.set(value);
        }
        alone
    }

    /// `update_alone_into`, the value put written into `value`.
    fn updated_into(
        &self,
        key: &[u8],
        prefix: usize,
        value: &mut Vec<u8>,
        change: impl FnOnce(Option<&[u8]>, &mut Vec<u8>) -> Result<Updated>,
    ) -> Result<bool> {
        let _guard = self.epochs.guard();
        let mut path = Path::default();
        let leaf = self.leaf_for_write(key, &mut path)?;
        let mut held = self.lock(leaf)?;
        let begins = key.get(..prefix).unwrap_or(key);
        // Searched, shown to `change` where it lies, and changed under one
        // latch (`here.rs`).
        let mut page = self.latched_covering(&mut held, key)?;
        let (found, alone, update) = {
            let node = Node::parse(&page)?;
            let found = node.search(key)?;
            let old = match found {
                Ok(at) => Some(node.value(at)?),
                Err(_) => None,
            };
            (found, alone_at(&node, found, begins)?, change(old, value)?)
        };
        let slot = match (update, found) {
            (Updated::Keep, _) | (Updated::Remove, Err(_)) => return Ok(alone),
            (Updated::Remove, Ok(at)) => return self.removed(&path, held, page, (key, at)).map(|()| alone),
            (Updated::Put, Ok(at)) => Slot::Replace(at),
            (Updated::Put, Err(at)) => Slot::Insert(at),
        };
        self.fits(key, value)?;
        let copy = self.change_latched(&held, &mut page, (key, value), slot)?;
        drop(page);
        let splits = match copy {
            Some(copy) => self.split(held.id, &copy, (key, value), slot)?,
            None => Vec::new(),
        };
        self.posted(&path, held, key, splits).map(|_| alone)
    }

    /// The leaf a writer of `key` locks, `path` the way to it: its page
    /// number read from its parent, the leaf itself not read, as the
    /// writer reads it once it owns it, moving right past a split or a
    /// leaf taken out, as from a leaf read (RULES.md D840). A root that is
    /// a leaf is the leaf.
    pub(super) fn leaf_for_write(&self, key: &[u8], path: &mut Path) -> Result<PageId> {
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
    pub(super) fn fits(&self, key: &[u8], value: &[u8]) -> Result<()> {
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
    pub(super) fn write(&self, key: &[u8], value: &[u8], mode: Mode<'_>) -> Result<Written> {
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
    /// The leaf is searched and changed under one latch (`here.rs`); one
    /// that no longer covers `key` is moved right from, and the leaf
    /// reached is written the same way.
    pub(super) fn write_from(&self, path: &Path, leaf: PageId, (key, value): (&[u8], &[u8]), mode: Mode<'_>) -> Result<Written> {
        let mut held = self.lock(leaf)?;
        loop {
            match self.write_here(&held, (key, value), mode)? {
                Here::Elsewhere => self.move_right(&mut held, key, |_| Ok(()))?,
                Here::Refused => return Ok(Written::Refused),
                Here::Done(old, past_the_end, splits) => {
                    let into = self.posted(path, held, key, splits)?;
                    if past_the_end {
                        self.note_edge(into);
                    }
                    return Ok(Written::Done(old, into));
                }
            }
        }
    }

    /// Posts `splits`, those a change of `key` in the leaf `held` caused,
    /// up the tree from `path`. The leaf the entry is in: the right page
    /// of the highest separator it is past, if any.
    pub(super) fn posted<'a>(&'a self, path: &Path, held: NodeLock<'a>, key: &[u8], splits: Vec<Split>) -> Result<PageId> {
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

/// Whether no entry beside the place `found` of a key in `node` begins
/// with `begins`: not if the place is first in the node, the node before
/// beyond reach, nor if it is last and the node's high key begins with
/// them (`BLinkTree::update_alone`).
fn alone_at(node: &Node<'_>, found: std::result::Result<usize, usize>, begins: &[u8]) -> Result<bool> {
    let (before, after) = match found {
        Ok(at) => (at.checked_sub(1), at.saturating_add(1)),
        Err(at) => (at.checked_sub(1), at),
    };
    let shares = |at: usize| node.key(at).map(|k| k.starts_with(begins));
    let none_before = before.map(shares).transpose()?.is_some_and(|s| !s);
    let none_after = if after < node.len() { !shares(after)? } else { node.high.is_none_or(|h| !h.starts_with(begins)) };
    Ok(none_before && none_after)
}

/// Whether `key` is in `leaf`, its value appended to `into`.
fn value_into(leaf: &Node<'_>, key: &[u8], into: &mut Vec<u8>) -> Result<bool> {
    let Ok(at) = leaf.search(key)? else { return Ok(false) };
    into.extend_from_slice(leaf.value(at)?);
    Ok(true)
}

/// The value of `key` in `leaf`, copied, if it is there.
fn value_of(leaf: &Node<'_>, key: &[u8]) -> Result<Option<Vec<u8>>> {
    match leaf.search(key)? {
        Ok(at) => Ok(Some(leaf.value(at)?.to_vec())),
        Err(_) => Ok(None),
    }
}

/// \[DESIGN\] Where `BLinkTree::get_for_write` found a key: the path to
/// its leaf, none if the leaf was the one noted for its row, the leaf, and
/// the tree's stamp then.
pub struct Descended {
    pub(super) path: Path,
    pub(super) leaf: PageId,
    pub(super) stamp: u64,
}

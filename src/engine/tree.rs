//! \[DESIGN\] A B-link tree over the buffer pool (RULES.md D31), after
//! Lehman and Yao, "Efficient Locking for Concurrent Operations on
//! B-Trees", ACM TODS 6(4):650–670, 1981: keys and values are bytes, keys
//! unique and compared bytewise. A table is a tree keyed by row id; an
//! index, one keyed by its encoded key then the row id, so its keys are
//! unique even when the values indexed repeat.
//!
//! Every node has a high key, the highest key it may hold, and a link to
//! its right neighbour (`node.rs`). A split moves keys only right, to a new
//! node linked in by the same write that lowers the old node's high key,
//! so a key is always reachable from where it was: a reader that finds a
//! key above a node's high key follows the link (§3, §4).
//!
//! * **Readers lock nothing.** They read a node under the buffer pool's
//!   latch, held only while that one node is read (the paper's atomic
//!   `get`, §2), and move right when the key is past it.
//! * **Writers** go down the same way, remembering the rightmost node they
//!   read at each level. They lock the leaf (`locks.rs`), move right if it
//!   has split, and change it in place if the change fits. Otherwise they
//!   split it: the right half written to a new page first, then the left
//!   half over the old one, which together are one change to the tree
//!   (§6.2, Lemma 2). Then they lock the remembered parent, move right
//!   from it as far as the separator needs, release the child, and insert
//!   the separator and the new page there, the same way, up the tree.
//!   At most three nodes are locked at once, always bottom to top and left
//!   to right, so writers never deadlock (§5, §6.1).
//! * Past the top of the remembered path, the split node's level is the
//!   root's or the tree has grown since: a new root is made above the old
//!   one, the leftmost node of its level, under the meta page's lock; or
//!   the parent is found again from the current root.
//! * **Deletes** remove an entry from its leaf; a key in an inner node
//!   stays, as it is only an upper bound (§7). A leaf a delete empties is
//!   taken out, its range given to its right neighbour, and its page freed
//!   once no operation that may hold its number is left (`reclaim.rs`).
//!
//! The paper notes a reader may in principle be overtaken forever by
//! splits ahead of it (livelock, §6.4); `TreeCounts::right_moves` measures
//! how often readers and writers follow links at all.
//!
//! One `BLinkTree` value works a tree at a time: it keeps the root's page
//! number and the node locks. The pages reach the disk as the buffer pool
//! writes them back; making a split survive a crash, the right half before
//! the left, is the log's work, with full page images (RULES.md D25).

mod load;
mod locks;
mod node;
mod reclaim;
mod scan;
mod write;

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use locks::{Locks, NodeLock};
use node::{ENTRY_OVERHEAD, HEADER, Node, Shape};
pub use scan::{Direction, Scan, scan_of};
pub use write::Update;
#[cfg(test)]
use write::{Mode, Written};

use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::pager::PageId;
use crate::error::{Code, Result, SqlError};

/// The format of a tree's meta page: the format, then the root's page.
pub const TREE_FORMAT: u8 = 1;
const META_LEN: usize = 9;

fn corrupt(why: &str) -> SqlError {
    SqlError::new(Code::Corrupt, format!("[DESIGN] a B-link tree is malformed: {why}"))
}

/// The counters of a tree, for the heuristics above it (RULES.md D30).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TreeCounts {
    /// Nodes split.
    pub splits: u64,
    /// Of those, roots: the tree grew a level.
    pub root_splits: u64,
    /// Links followed to a right neighbour, by readers and writers: a
    /// node had split since its parent was read.
    pub right_moves: u64,
    /// Leaves emptied and taken out of the tree (`reclaim.rs`).
    pub unlinked: u64,
}

#[derive(Default)]
struct Counters {
    splits: AtomicU64,
    root_splits: AtomicU64,
    right_moves: AtomicU64,
    unlinked: AtomicU64,
}

/// Adds 1 to `counter`; it would wrap only after 2^64.
fn count(counter: &AtomicU64) {
    counter.fetch_add(1, Ordering::Relaxed);
}

/// \[DESIGN\] A B-link tree in a pager file of a buffer pool. It may be
/// shared between threads.
pub struct BLinkTree {
    pool: Arc<BufferPool>,
    file: FileId,
    /// The page naming the root, so the tree is found again by it.
    meta: PageId,
    /// The root's page, as the meta page names it.
    root: AtomicU64,
    /// The most content a node holds in its page.
    capacity: usize,
    locks: Locks,
    counters: Counters,
    /// The operations under way by epoch, and the pages taken out
    /// (`reclaim.rs`).
    epochs: reclaim::Epochs,
}

/// What a descent looks for at each level.
#[derive(Clone, Copy)]
enum Target<'k> {
    /// The node whose range holds a key.
    Key(&'k [u8]),
    /// The leftmost node.
    First,
    /// The rightmost node.
    Last,
}

impl Target<'_> {
    fn covered_by(self, node: &Node<'_>) -> bool {
        match self {
            Target::Key(key) => node.covers(key),
            Target::First => true,
            Target::Last => node.high.is_none(),
        }
    }

    fn child<'n>(self, node: &Node<'n>) -> Result<(PageId, Option<&'n [u8]>)> {
        match self {
            Target::Key(key) => node.child(key),
            Target::First => Ok((node.first_child(), None)),
            Target::Last => node.child_at(node.len()),
        }
    }
}

/// Where a descent went.
#[derive(Default)]
struct Path {
    /// The rightmost node read at each level above the one it stopped at,
    /// the top first (§5).
    stack: Vec<PageId>,
    /// Whether to keep `low`.
    track_low: bool,
    /// The key below the range of the node it stopped at; none for the
    /// leftmost node of its level.
    low: Option<Vec<u8>>,
}

/// A change to a node, at an entry.
#[derive(Clone, Copy)]
enum Slot {
    /// A new entry, before entry `n`.
    Insert(usize),
    /// Entry `n`'s value replaced.
    Replace(usize),
}

/// Whether `slot`'s insert into leaf `node` starts a run of its own: past
/// the end of the rightmost leaf, while the leaf's last insert went
/// elsewhere, so another run is going on inside it. The leaf is then split
/// at once, the new run starting a node of its own, so neither run's
/// entries ride along with the other's as it fills its nodes.
fn starts_run(node: &Node<'_>, slot: Slot) -> Result<bool> {
    let Slot::Insert(at) = slot else { return Ok(false) };
    if node.level != 0 || node.right.is_some() || at != node.len() || at == 0 {
        return Ok(false);
    }
    Ok(node.last_insert()?.is_some_and(|last| last.saturating_add(1) != at))
}

/// A split, to be posted to the parent level.
struct Split {
    /// The left half's high key.
    separator: Vec<u8>,
    /// The right half's page.
    right: PageId,
    /// The level of the node split.
    level: u8,
}

impl BLinkTree {
    /// Creates an empty tree in pager file `file` of `pool`: its meta page
    /// and an empty root leaf.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if `file` is not a registered pager file; the buffer
    /// pool's errors.
    pub fn create(pool: Arc<BufferPool>, file: FileId) -> Result<BLinkTree> {
        let capacity = pool.capacity(file)?;
        let root = {
            let (root, mut page) = pool.new_page(file)?;
            *page = node::build(&Shape { level: 0, high: None, right: None, first: PageId(0) }, &[])?;
            root
        };
        let meta = {
            let (meta, mut page) = pool.new_page(file)?;
            *page = meta_content(root);
            meta
        };
        Ok(BLinkTree::with(pool, file, meta, root, capacity))
    }

    /// Opens the tree whose meta page is `meta` in pager file `file`.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if `meta` is not a tree's meta page; as `create`.
    pub fn open(pool: Arc<BufferPool>, file: FileId, meta: PageId) -> Result<BLinkTree> {
        let capacity = pool.capacity(file)?;
        let root = {
            let page = pool.read(file, meta.0)?;
            match <[u8; META_LEN]>::try_from(&*page) {
                Ok([TREE_FORMAT, root @ ..]) => PageId(u64::from_le_bytes(root)),
                Ok(_) | Err(_) => return Err(corrupt(&format!("page {} is not a meta page of this format", meta.0))),
            }
        };
        Ok(BLinkTree::with(pool, file, meta, root, capacity))
    }

    fn with(pool: Arc<BufferPool>, file: FileId, meta: PageId, root: PageId, capacity: usize) -> BLinkTree {
        BLinkTree {
            pool,
            file,
            meta,
            root: AtomicU64::new(root.0),
            capacity,
            locks: Locks::new(),
            counters: Counters::default(),
            epochs: reclaim::Epochs::new(),
        }
    }

    /// Its meta page, by which `open` finds it.
    pub fn meta(&self) -> PageId {
        self.meta
    }

    /// The most bytes a key and its value may take together; a value
    /// shorter than 8 bytes counts as 8, the size of a page number, as its
    /// key may become a separator in an inner node. Entries this size keep
    /// at least four to a node, so a split always leaves halves that fit.
    pub fn max_entry(&self) -> usize {
        self.capacity.saturating_sub(HEADER.saturating_mul(2)).checked_div(4).unwrap_or_default().saturating_sub(ENTRY_OVERHEAD)
    }

    /// Levels in the tree: 1 while the root is a leaf.
    ///
    /// # Errors
    ///
    /// The buffer pool's errors; SQLCODE -916 if the root is not a node.
    pub fn height(&self) -> Result<u8> {
        let page = self.pool.read(self.file, self.root.load(Ordering::Acquire))?;
        Ok(Node::parse(&page)?.level.saturating_add(1))
    }

    /// Frees every page of the tree, its meta page too: for a tree no one
    /// reads or writes again, a dropped index's. Each level is walked from
    /// its leftmost node, the first child of the level above's, along the
    /// right links.
    ///
    /// # Errors
    ///
    /// The buffer pool's errors; SQLCODE -916 if a page is not a node, or a
    /// level's links come back to a node already walked.
    pub fn free(&self) -> Result<()> {
        let mut pages = vec![self.meta];
        pages.extend(self.epochs.all_retired());
        let mut seen = HashSet::new();
        let mut leftmost = Some(PageId(self.root.load(Ordering::Acquire)));
        while let Some(first) = leftmost.take() {
            let mut next = Some(first);
            while let Some(at) = next {
                if !seen.insert(at.0) {
                    return Err(corrupt(&format!("page {} is linked to twice", at.0)));
                }
                let page = self.pool.read(self.file, at.0)?;
                let node = Node::parse(&page)?;
                if at == first && node.level > 0 {
                    leftmost = Some(node.first_child());
                }
                next = node.right;
                pages.push(at);
            }
        }
        pages.into_iter().try_for_each(|p| self.pool.free(self.file, p))
    }

    /// The pages taken out of it and not yet freed, freed: for a tree no
    /// one reads or writes any more.
    ///
    /// # Errors
    ///
    /// The buffer pool's errors.
    pub fn free_retired(&self) -> Result<()> {
        self.epochs.all_retired().into_iter().try_for_each(|p| self.pool.free(self.file, p))
    }

    /// The counters now.
    pub fn counts(&self) -> TreeCounts {
        let get = |c: &AtomicU64| c.load(Ordering::Relaxed);
        let c = &self.counters;
        TreeCounts {
            splits: get(&c.splits),
            root_splits: get(&c.root_splits),
            right_moves: get(&c.right_moves),
            unlinked: get(&c.unlinked),
        }
    }
}

/// The pages taken out and not yet freed are freed as the tree goes: no
/// operation can hold them then. A failure leaves them to the file, which
/// costs space only.
impl Drop for BLinkTree {
    fn drop(&mut self) {
        let _ = self.free_retired();
    }
}

/// The content of a meta page naming `root`.
fn meta_content(root: PageId) -> Vec<u8> {
    let mut content = Vec::with_capacity(META_LEN);
    content.push(TREE_FORMAT);
    content.extend_from_slice(&root.0.to_le_bytes());
    content
}

impl BLinkTree {
    /// Goes down from the root, locking nothing, to the node at `level`
    /// that `target` is in, moving right past splits; `path` remembers the
    /// way. What `at` finds in that node, read under its latch, and its
    /// page.
    fn descend<R>(&self, target: Target<'_>, level: u8, path: &mut Path, at: impl FnOnce(&Node<'_>) -> Result<R>) -> Result<(PageId, R)> {
        let mut id = PageId(self.root.load(Ordering::Acquire));
        let mut expected = None;
        loop {
            let page = self.pool.read(self.file, id.0)?;
            let node = Node::parse(&page)?;
            if expected.is_some_and(|l| l != node.level) || node.level < level {
                return Err(corrupt(&format!("node {} is not at the level its parent says", id.0)));
            }
            // Taken out after its parent or its left neighbour was read: its
            // range is its right neighbour's, whose range begins where its
            // did, so the key below it stays (`reclaim.rs`).
            if node.deleted {
                id = node.right.ok_or_else(|| corrupt("a node taken out has no link"))?;
                count(&self.counters.right_moves);
                continue;
            }
            if !target.covered_by(&node) {
                if path.track_low {
                    path.low = node.high.map(<[u8]>::to_vec);
                }
                id = node.right.ok_or_else(|| corrupt("a node with a high key has no right neighbour"))?;
                count(&self.counters.right_moves);
                continue;
            }
            if node.level == level {
                return Ok((id, at(&node)?));
            }
            path.stack.push(id);
            let (child, low) = target.child(&node)?;
            if let Some(low) = low.filter(|_| path.track_low) {
                path.low = Some(low.to_vec());
            }
            expected = node.level.checked_sub(1);
            id = child;
        }
    }

    /// With `held` locked, moves right along its level to the node covering
    /// `key`, locking the next node before releasing the last (§5,
    /// move.right). What `at` finds in that node.
    fn move_right<'t, R>(&'t self, held: &mut NodeLock<'t>, key: &[u8], at: impl FnOnce(&Node<'_>) -> Result<R>) -> Result<R> {
        loop {
            let right = {
                let page = self.pool.read(self.file, held.id.0)?;
                let node = Node::parse(&page)?;
                // A node taken out leads to the one that took its range
                // (`reclaim.rs`).
                if node.covers(key) && !node.deleted {
                    return at(&node);
                }
                node.right.ok_or_else(|| corrupt("a node with a high key has no right neighbour"))?
            };
            *held = self.locks.lock(right)?;
            count(&self.counters.right_moves);
        }
    }

    /// Makes `slot`'s change to locked node `held`: in place if the node
    /// then fits its page, else by splitting it. The split, if any.
    fn change(&self, held: &NodeLock<'_>, key: &[u8], value: &[u8], slot: Slot) -> Result<Option<Split>> {
        let copy = {
            let mut page = self.pool.write(self.file, held.id)?;
            let (live, old, new_run) = {
                let node = Node::parse(&page)?;
                let old = match slot {
                    Slot::Insert(_) => 0,
                    Slot::Replace(at) => node::entry_size(node.key(at)?, node.entry(at)?.1),
                };
                (node.live(), old, starts_run(&node, slot)?)
            };
            // A value of the same length, as a version stamped committed:
            // written over in place, nothing moved.
            if let Slot::Replace(at) = slot
                && old == node::entry_size(key, value)
            {
                node::overwrite(&mut page, at, value)?;
                return Ok(None);
            }
            let added = node::entry_size(key, value);
            if new_run || live.saturating_sub(old).saturating_add(added) > self.capacity {
                page.clone()
            } else {
                let at = match slot {
                    Slot::Insert(at) => at,
                    Slot::Replace(at) => {
                        node::remove(&mut page, at)?;
                        at
                    }
                };
                if page.len().saturating_add(added) > self.capacity {
                    *page = node::compact(&page)?;
                }
                node::insert(&mut page, at, key, value)?;
                return Ok(None);
            }
        };
        self.split(held.id, &copy, (key, value), slot).map(Some)
    }

    /// Splits node `id`, whose content was `copy`, making `slot`'s change
    /// (§4): the right half written to a new page, then the left half over
    /// the old, linked to it.
    fn split(&self, id: PageId, copy: &[u8], entry: (&[u8], &[u8]), slot: Slot) -> Result<Split> {
        let node = Node::parse(copy)?;
        let mut entries = node.entries()?;
        let run = match slot {
            Slot::Insert(at) => {
                if at > entries.len() {
                    return Err(corrupt("an entry inserted is past its node"));
                }
                // Inserts run in key order through the node if this one goes
                // right after the last, or past the end of the rightmost node.
                let after_last = at.checked_sub(1).is_some_and(|before| node.last_insert().ok().flatten() == Some(before));
                let past_the_end = node.right.is_none() && at == entries.len();
                entries.insert(at, entry);
                (after_last || past_the_end).then_some(at)
            }
            Slot::Replace(at) => {
                *entries.get_mut(at).ok_or_else(|| corrupt("an entry replaced is past its node"))? = entry;
                None
            }
        };
        let (new, mut page) = self.pool.new_page(self.file)?;
        let halves = match node::split(&node.shape(), &entries, run, new) {
            Ok(halves) => halves,
            Err(e) => {
                drop(page);
                // Never linked in, so nothing refers to it.
                let _ = self.pool.free(self.file, new);
                return Err(e);
            }
        };
        *page = halves.right;
        drop(page);
        *self.pool.write(self.file, id)? = halves.left;
        count(&self.counters.splits);
        Ok(Split { separator: halves.separator, right: new, level: node.level })
    }

    /// Locks the node one level above `split` that takes its separator, and
    /// the slot for it: the node remembered on the way down, moved right
    /// from as far as the separator needs (§5). Past the top of the path, a
    /// new root takes it if the split's level is the root's, and `None` is
    /// returned; else the parent is found again from the root.
    fn lock_parent(&self, path: &mut Path, split: &Split) -> Result<Option<(NodeLock<'_>, usize)>> {
        let level = split.level.checked_add(1).ok_or_else(|| corrupt("it is too tall"))?;
        loop {
            if let Some(parent) = path.stack.pop() {
                let mut held = self.locks.lock(parent)?;
                let at =
                    self.move_right(&mut held, &split.separator, |node| match (node.level == level, node.search(&split.separator)?) {
                        (true, Err(at)) => Ok(at),
                        (false, _) | (true, Ok(_)) => Err(corrupt("a separator's parent is not one level up, or holds it already")),
                    })?;
                return Ok(Some((held, at)));
            }
            if self.grow(split)? {
                return Ok(None);
            }
            let (parent, ()) = self.descend(Target::Key(&split.separator), level, path, |_| Ok(()))?;
            path.stack.push(parent);
        }
    }

    /// Makes a new root above `split`'s level if that level is the root's:
    /// its first child the root, the leftmost node of its level, and its one
    /// entry the separator, with the right half. Under the meta page's lock,
    /// the highest in the locking order. Whether it did.
    fn grow(&self, split: &Split) -> Result<bool> {
        let _meta = self.locks.lock(self.meta)?;
        let root = PageId(self.root.load(Ordering::Acquire));
        let root_level = Node::parse(&self.pool.read(self.file, root.0)?)?.level;
        if root_level != split.level {
            return Ok(false);
        }
        let level = root_level.checked_add(1).ok_or_else(|| corrupt("it is too tall"))?;
        let new_root = {
            let (new_root, mut page) = self.pool.new_page(self.file)?;
            let child = split.right.0.to_le_bytes();
            *page = node::build(&Shape { level, high: None, right: None, first: root }, &[(&split.separator, &child)])?;
            new_root
        };
        *self.pool.write(self.file, self.meta)? = meta_content(new_root);
        self.root.store(new_root.0, Ordering::Release);
        count(&self.counters.root_splits);
        Ok(true)
    }
}

// Unit tests: the concurrent paths, set up on one thread, and malformed
// trees (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;

//! \[DESIGN\] (RULES.md D502) Copies of inner nodes, kept per thread: a
//! descent reads an inner node from its copy while the node's frame is at
//! the version the copy was taken at (`disk/buffer/versions.rs`), and so
//! takes no latch and writes no shared word on the way down. Leis et al.,
//! "The ART of Practical Synchronization", 2016, section 3, validate a
//! node's version the same way, there over the node itself.
//!
//! A copy read is exactly the node's content now, as the version moves on
//! before any change, so a descent from it goes where one through the
//! latch would. A leaf is not copied here: a read of one takes its latch,
//! unless the thread reads it often and finds it unchanged (`leaves.rs`).
//!
//! Each copy keeps the node's header as parsed beside it (RULES.md D840),
//! so a descent through it reads no header and checks nothing again, as a
//! leaf's copy does (D808).
//!
//! A thread keeps 256 copies, found by their page, across all the trees it
//! reads; the one to give way is chosen by a clock bit set as a descent
//! reads it (RULES.md D807).

use std::cell::RefCell;
use std::ops::Deref;
use std::rc::Rc;

use std::sync::atomic::Ordering;

use super::node::{Node, Parsed, child_in};
use super::{BLinkTree, Within, leaves};
use crate::disk::buffer::{BufferPool, FileId, PageRef, Stamp, Tally};
use crate::error::Result;
use crate::hash::xxhash::{Xxh3BuildHasher, Xxh3Map};

/// \[DESIGN\] The copies a thread keeps, across every tree it reads (RULES.md
/// D807): the roots and the levels below them of the tables a transaction
/// mix reads together, at most 2 MiB of 8 KiB pages.
pub(super) const KEPT: usize = 256;
/// \[DESIGN\] The most levels a descent through the copies goes down: a
/// tree of 4 KiB pages holding four entries a node is no taller over
/// 2^32 pages. A taller one is descended node by node.
const LEVELS: usize = 16;

/// A copy of page `id` of `file`, read with `stamp`, and its clock bit,
/// set when a descent reads it and cleared as the hand passes.
struct Copy {
    file: FileId,
    id: u64,
    stamp: Stamp,
    bytes: Rc<[u8]>,
    parsed: Parsed,
    used: bool,
}

/// A thread's copies, where each is by its page, and the clock's hand.
struct Copies {
    kept: Vec<Copy>,
    at: Xxh3Map<(FileId, u64), usize>,
    hand: usize,
}

impl Default for Copies {
    fn default() -> Copies {
        Copies { kept: Vec::new(), at: Xxh3Map::with_hasher(Xxh3BuildHasher::with_seed(0)), hand: 0 }
    }
}

impl Copies {
    /// The place of a copy to give way, by CLOCK (Corbató, 1968). It is the
    /// first the hand finds unused, and the hand clears the bits it passes.
    fn victim(&mut self) -> usize {
        loop {
            let at = self.hand;
            self.hand = at.checked_add(1).and_then(|n| n.checked_rem(self.kept.len())).unwrap_or(0);
            let Some(copy) = self.kept.get_mut(at) else { return 0 };
            if !copy.used {
                return at;
            }
            copy.used = false;
        }
    }

    /// Keeps `bytes`, page `id` of `file` read with `stamp`, at the page's
    /// place, or a free one, or the victim's; into the bytes of the copy
    /// it replaces when no descent holds them, so a page read again
    /// allocates nothing.
    fn put(&mut self, (file, id, stamp): (FileId, u64, Stamp), bytes: &[u8], parsed: Parsed) {
        let found = self.at.get(&(file, id)).copied();
        if found.is_none() && self.kept.len() < KEPT {
            self.at.insert((file, id), self.kept.len());
            self.kept.push(Copy { file, id, stamp, bytes: Rc::from(bytes), parsed, used: true });
            return;
        }
        let at = found.unwrap_or_else(|| self.victim());
        let Some(old) = self.kept.get_mut(at) else { return };
        if (old.file, old.id) != (file, id) {
            self.at.remove(&(old.file, old.id));
            self.at.insert((file, id), at);
        }
        match Rc::get_mut(&mut old.bytes).filter(|b| b.len() == bytes.len()) {
            Some(kept) => kept.copy_from_slice(bytes),
            None => old.bytes = Rc::from(bytes),
        }
        (old.file, old.id, old.stamp, old.parsed, old.used) = (file, id, stamp, parsed, true);
    }
}

thread_local! {
    static COPIES: RefCell<Copies> = RefCell::default();
}

/// A node's page as a descent reads it: latched in its frame, or a copy
/// and its header as parsed.
pub(super) enum Read<'p> {
    Held(PageRef<'p>, Stamp),
    Copied(Rc<[u8]>, Parsed),
}

impl Read<'_> {
    /// The node it holds: a copy's from its header as parsed, with no
    /// check made again; one latched parsed.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if a page latched is not a node.
    pub(super) fn node(&self) -> Result<Node<'_>> {
        match self {
            Read::Held(page, _) => Node::parse(page),
            Read::Copied(bytes, parsed) => Node::parsed_again(bytes, parsed).map_or_else(|| Node::parse(bytes), Ok),
        }
    }
}

impl Deref for Read<'_> {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Read::Held(page, _) => page,
            Read::Copied(bytes, _) => bytes,
        }
    }
}

/// Page `id` of `file` in `pool`: from a copy still current, if `copied`
/// and the thread keeps one; else latched, as `read_tallied` reads it.
#[cfg(test)]
pub(super) fn read<'p>(pool: &'p BufferPool, (file, id): (FileId, u64), tally: Option<&Tally>, copied: bool) -> Result<Read<'p>> {
    if let Some((bytes, parsed)) = copied.then(|| current(pool, file, id)).flatten() {
        return Ok(Read::Copied(bytes, parsed));
    }
    let (page, stamp) = pool.read_stamped(file, id, tally)?;
    Ok(Read::Held(page, stamp))
}

/// The thread's copy of page `id` of `file`, if it is still current; its
/// clock bit set.
fn current(pool: &BufferPool, file: FileId, id: u64) -> Option<(Rc<[u8]>, Parsed)> {
    COPIES.with(|copies| {
        let mut copies = copies.try_borrow_mut().ok()?;
        let copy = copies.current(pool, (file, id))?;
        Some((Rc::clone(&copy.bytes), copy.parsed))
    })
}

impl Copies {
    /// Its copy of page `id` of `file`, if it is still current; its clock
    /// bit set.
    fn current(&mut self, pool: &BufferPool, (file, id): (FileId, u64)) -> Option<&Copy> {
        let Copies { kept, at, .. } = self;
        let copy = kept.get_mut(*at.get(&(file, id))?).filter(|c| pool.unchanged(file, id, c.stamp))?;
        copy.used = true;
        Some(copy)
    }
}

/// As `read`, of a page that may be a leaf if `leaf`: then from the
/// thread's copy of the leaf, if it has one still current (`leaves.rs`,
/// RULES.md D858). `hash` is the page's (`BufferPool::page_hash`), made
/// once for the leaf's place and the pool's (D951).
pub(super) fn read_at<'p>(
    pool: &'p BufferPool,
    (file, id, hash): (FileId, u64, u64),
    tally: Option<&Tally>,
    (copied, leaf): (bool, bool),
) -> Result<Read<'p>> {
    if let Some((bytes, parsed)) = leaf.then(|| leaves::current_at(pool, (file, id), hash)).flatten() {
        return Ok(Read::Copied(bytes, parsed));
    }
    if let Some((bytes, parsed)) = copied.then(|| current(pool, file, id)).flatten() {
        return Ok(Read::Copied(bytes, parsed));
    }
    let (page, stamp) = pool.read_stamped_at((file, id), hash, tally)?;
    Ok(Read::Held(page, stamp))
}

/// \[DESIGN\] (RULES.md D955) The child of inner node `id` of `file`
/// whose range holds `key`, and the node's level, read from the thread's
/// copy of the node where it is kept, with no node made of it
/// (`node/lean.rs`) and no count of the copy's holders moved: none if the
/// thread keeps no copy still current, or the copy is no inner node in
/// its tree whose range holds `key`.
fn child_in_copy(pool: &BufferPool, (file, id): (FileId, u64), key: &[u8]) -> Option<(u64, u8)> {
    COPIES.with(|copies| {
        let mut copies = copies.try_borrow_mut().ok()?;
        child_in(&copies.current(pool, (file, id))?.bytes, key)
    })
}

impl BLinkTree {
    /// \[DESIGN\] (RULES.md D955) The leaf `key` is in, found through the
    /// thread's copies of the inner nodes alone, from the root down: each
    /// copy still its node's content, an inner node in the tree whose
    /// range holds `key`, at the level its parent says. So the leaf is
    /// the one a descent that latched each node now would reach, and its
    /// range begins before `key`. A root with no copy is given itself, as
    /// it may be a leaf, which the caller checks. None where a copy is
    /// missing or does not hold `key`'s range, as past a split not yet
    /// posted; the caller descends then. The caller is in the tree's
    /// epoch, so the page given is not freed under it.
    pub(super) fn leaf_by_copies(&self, _within: Within<'_>, key: &[u8]) -> Option<u64> {
        let root = self.root.load(Ordering::Acquire);
        let (mut id, mut above) = (root, None::<u8>);
        for _ in 0..LEVELS {
            let Some((child, level)) = child_in_copy(&self.pool, (self.file, id), key) else { break };
            if above.is_some_and(|above| level.checked_add(1) != Some(above)) {
                return None;
            }
            if level == 1 {
                return Some(child);
            }
            (id, above) = (child, Some(level));
        }
        (id == root).then_some(root)
    }
}

/// Keeps a copy of `read`, page `id` of `file`, an inner node read
/// latched, whose header parsed is `parsed`; one kept already of the page
/// gives way to it, else the one the clock finds unused (D807).
pub(super) fn keep(read: &Read<'_>, (file, id): (FileId, u64), parsed: Parsed) {
    let Read::Held(page, stamp) = read else { return };
    COPIES.with(|copies| {
        if let Ok(mut copies) = copies.try_borrow_mut() {
            copies.put((file, id, *stamp), page, parsed);
        }
    });
}

/// The pages of the copies the thread keeps, for tests.
#[cfg(test)]
pub(super) fn kept() -> Vec<(FileId, u64)> {
    COPIES.with(|copies| copies.borrow().kept.iter().map(|c| (c.file, c.id)).collect())
}

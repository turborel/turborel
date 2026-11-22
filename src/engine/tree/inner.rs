//! \[DESIGN\] (RULES.md D502) Copies of inner nodes, kept per thread: a
//! descent reads an inner node from its copy while the node's frame is at
//! the version the copy was taken at (`disk/buffer/versions.rs`), and so
//! takes no latch and writes no shared word on the way down. Leis et al.,
//! "The ART of Practical Synchronization", 2016, section 3, validate a
//! node's version the same way, there over the node itself.
//!
//! A copy read is exactly the node's content now, as the version moves on
//! before any change, so a descent from it goes where one through the
//! latch would. Leaves are never copied: a read of one takes its latch.
//!
//! A thread keeps 256 copies, found by their page, across all the trees it
//! reads; the one to give way is chosen by a clock bit set as a descent
//! reads it (RULES.md D807).

use std::cell::RefCell;
use std::ops::Deref;
use std::rc::Rc;

use crate::disk::buffer::{BufferPool, FileId, PageRef, Stamp, Tally};
use crate::error::Result;
use crate::hash::xxhash::{Xxh3BuildHasher, Xxh3Map};

/// \[DESIGN\] The copies a thread keeps, across every tree it reads (RULES.md
/// D807): the roots and the levels below them of the tables a transaction
/// mix reads together, at most 2 MiB of 8 KiB pages.
pub(super) const KEPT: usize = 256;

/// A copy of page `id` of `file`, read with `stamp`, and its clock bit:
/// set when a descent reads it, cleared as the hand passes.
struct Copy {
    file: FileId,
    id: u64,
    stamp: Stamp,
    bytes: Rc<[u8]>,
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
    /// The place of a copy to give way, by CLOCK (Corbató, 1968): the
    /// first the hand finds unused, clearing the bits of those it passes.
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
    fn put(&mut self, (file, id, stamp): (FileId, u64, Stamp), bytes: &[u8]) {
        let found = self.at.get(&(file, id)).copied();
        if found.is_none() && self.kept.len() < KEPT {
            self.at.insert((file, id), self.kept.len());
            self.kept.push(Copy { file, id, stamp, bytes: Rc::from(bytes), used: true });
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
        (old.file, old.id, old.stamp, old.used) = (file, id, stamp, true);
    }
}

thread_local! {
    static COPIES: RefCell<Copies> = RefCell::default();
}

/// A node's page as a descent reads it: latched in its frame, or a copy.
pub(super) enum Read<'p> {
    Held(PageRef<'p>, Stamp),
    Copied(Rc<[u8]>),
}

impl Deref for Read<'_> {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Read::Held(page, _) => page,
            Read::Copied(bytes) => bytes,
        }
    }
}

/// Page `id` of `file` in `pool`: from a copy still current, if `copied`
/// and the thread keeps one; else latched, as `read_tallied` reads it.
pub(super) fn read<'p>(pool: &'p BufferPool, (file, id): (FileId, u64), tally: Option<&Tally>, copied: bool) -> Result<Read<'p>> {
    if let Some(bytes) = copied.then(|| current(pool, file, id)).flatten() {
        return Ok(Read::Copied(bytes));
    }
    let (page, stamp) = pool.read_stamped(file, id, tally)?;
    Ok(Read::Held(page, stamp))
}

/// The thread's copy of page `id` of `file`, if it is still current; its
/// clock bit set.
fn current(pool: &BufferPool, file: FileId, id: u64) -> Option<Rc<[u8]>> {
    COPIES.with(|copies| {
        let mut copies = copies.try_borrow_mut().ok()?;
        let Copies { kept, at, .. } = &mut *copies;
        let copy = kept.get_mut(*at.get(&(file, id))?).filter(|c| pool.unchanged(file, id, c.stamp))?;
        copy.used = true;
        Some(Rc::clone(&copy.bytes))
    })
}

/// Keeps a copy of `read`, page `id` of `file`, an inner node read
/// latched; one kept already of the page gives way to it, else the one
/// the clock finds unused (D807).
pub(super) fn keep(read: &Read<'_>, (file, id): (FileId, u64)) {
    let Read::Held(page, stamp) = read else { return };
    COPIES.with(|copies| {
        if let Ok(mut copies) = copies.try_borrow_mut() {
            copies.put((file, id, *stamp), page);
        }
    });
}

/// The pages of the copies the thread keeps, for tests.
#[cfg(test)]
pub(super) fn kept() -> Vec<(FileId, u64)> {
    COPIES.with(|copies| copies.borrow().kept.iter().map(|c| (c.file, c.id)).collect())
}

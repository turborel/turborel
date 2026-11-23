//! \[DESIGN\] Empty and sparse leaves taken out of a B-link tree, and their
//! pages freed once no reader can hold them (RULES.md D31, D814), after
//! Lanin and Shasha, "A Symmetric Concurrent B-Tree Algorithm", FJCC 1986:
//! a leaf emptied by a delete, or left sparse and merged (`merge.rs`),
//! gives its range to its right neighbour, and keeps a link to it, so a
//! reader or writer that reaches it goes where its range went.
//!
//! **Unlinking** a leaf L whose neighbours P, left, and R, right, have
//! its parent: P, L, R and the parent are locked in that order, bottom to
//! top and left to right, as every writer locks; then, each check made
//! again under the locks, R is written with L's entries and its own, if L
//! has any, P's right link is made R, L is marked deleted with its link to
//! R, and the parent sends L's range to R: L's entry's child made R, and
//! R's own entry taken out. A node is bounded only by its high key, so R
//! holds L's range with its high key and link unchanged, and nothing above
//! grows. Whoever reaches L, by a parent or a link read before, moves
//! right to R, as past any split. A leaf first or last under its parent
//! stays.
//!
//! **Epochs.** A page freed may be given to another node, so L's page is
//! freed only once no reader may still hold its number. Every operation on
//! the tree, and every scan while it lives, is counted in the epoch current
//! as it began, on the thread's stripe of counters, each on its own cache
//! line. The epoch moves on once no operation of the one before it is left;
//! a page taken out in epoch e is freed once the epoch is e + 2, when every
//! operation that began before it was taken out has ended.
//!
//! A thread that makes many operations on one tree pins it, and is counted
//! once for all of them (`pin.rs`).
//!
//! Reads that start from a leaf remembered from the last read (`near.rs`)
//! rely on two orderings here. A leaf is marked deleted, under its lock,
//! before its page is retired, so a leaf read not deleted is still in the
//! tree. And `retire` moves the tree's stamp before it notes the page, so a
//! read that loaded the stamp inside its epoch and finds it unchanged holds
//! the number of no page retired since.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use super::merge::Merging;
use super::node::{self, Node};
use super::{BLinkTree, Path};
use crate::disk::pager::PageId;
use crate::error::Result;
use crate::sync::{lock_builtin, thread_stripe};

/// The epochs made so far, so each has a number of its own.
static MADE: AtomicU64 = AtomicU64::new(1);

/// \[DESIGN\] Stripes of the epoch counters: more than the threads that use
/// a tree at once, so they seldom share one.
const STRIPES: usize = 64;

/// The operations of one stripe under way, by the parity of their epoch.
#[repr(align(128))]
#[derive(Default)]
struct Stripe([AtomicU64; 2]);

/// The epochs of a tree, and the pages taken out of it, to free.
pub(crate) struct Epochs {
    /// Its number, its own among the process's trees (`pin.rs`).
    number: u64,
    current: AtomicU64,
    under_way: Box<[Stripe]>,
    retired: Mutex<Vec<(u64, PageId)>>,
    /// Moved as a page is retired (`near.rs`).
    stamp: AtomicU64,
}

/// An operation counted in its epoch: its stripe and the epoch's parity.
#[derive(Clone, Copy)]
pub(crate) struct Entered {
    stripe: usize,
    parity: usize,
}

/// An operation under way, counted until it is dropped.
pub(super) struct Guard<'e> {
    epochs: &'e Epochs,
    entered: Entered,
}

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        self.epochs.leave(self.entered);
    }
}

impl Epochs {
    pub(crate) fn new() -> Epochs {
        Epochs {
            number: MADE.fetch_add(1, Ordering::Relaxed),
            current: AtomicU64::new(0),
            under_way: (0..STRIPES).map(|_| Stripe::default()).collect(),
            retired: Mutex::new(Vec::new()),
            stamp: AtomicU64::new(0),
        }
    }

    /// Counts an operation beginning, in the epoch current: counted, and
    /// the epoch still current once it is, or counted again.
    pub(crate) fn enter(&self) -> Entered {
        let stripe = thread_stripe(STRIPES);
        loop {
            let epoch = self.current.load(Ordering::SeqCst);
            let parity = usize::from(!epoch.is_multiple_of(2));
            let Some(count) = self.under_way.get(stripe).and_then(|s| s.0.get(parity)) else { return Entered { stripe, parity } };
            count.fetch_add(1, Ordering::SeqCst);
            if self.current.load(Ordering::SeqCst) == epoch {
                return Entered { stripe, parity };
            }
            count.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// An operation counted until the guard is dropped, whether or not
    /// the thread has the tree pinned (`pin.rs`).
    pub(super) fn guarded(&self) -> Guard<'_> {
        Guard { epochs: self, entered: self.enter() }
    }

    /// Its number, never 0.
    pub(super) fn number(&self) -> u64 {
        self.number
    }

    /// Counts the operation `entered` ended.
    pub(crate) fn leave(&self, entered: Entered) {
        if let Some(count) = self.under_way.get(entered.stripe).and_then(|s| s.0.get(entered.parity)) {
            count.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// Moves the epoch on if no operation of the one before it is left.
    fn advance(&self) {
        let epoch = self.current.load(Ordering::SeqCst);
        // The one before the current has the parity of the one after it.
        let before = usize::from(epoch.is_multiple_of(2));
        let left = self.under_way.iter().any(|s| s.0.get(before).is_some_and(|c| c.load(Ordering::SeqCst) > 0));
        if !left {
            let _ = self.current.compare_exchange(epoch, epoch.saturating_add(1), Ordering::SeqCst, Ordering::SeqCst);
        }
    }

    /// What moves as a page is retired, loaded after an operation enters
    /// its epoch, so a leaf known at this stamp is unfreed until it leaves.
    pub(super) fn stamp(&self) -> u64 {
        self.stamp.load(Ordering::SeqCst)
    }

    /// Notes `page` taken out, to free once no operation may hold it.
    pub(crate) fn retire(&self, page: PageId) {
        self.stamp.fetch_add(1, Ordering::SeqCst);
        let epoch = self.current.load(Ordering::SeqCst);
        lock_builtin(&self.retired).push((epoch, page));
    }

    /// The pages taken out that no operation may hold now, the epoch moved
    /// on first if it can: taken from those to free.
    pub(crate) fn freeable(&self) -> Vec<PageId> {
        self.advance();
        let now = self.current.load(Ordering::SeqCst);
        let mut retired = lock_builtin(&self.retired);
        let (free, keep): (Vec<_>, Vec<_>) = retired.drain(..).partition(|(epoch, _)| epoch.saturating_add(2) <= now);
        *retired = keep;
        free.into_iter().map(|(_, page)| page).collect()
    }

    /// Keeps `pages`, taken from those to free, to free next time.
    pub(super) fn keep(&self, pages: impl Iterator<Item = PageId>) {
        lock_builtin(&self.retired).extend(pages.map(|page| (0, page)));
    }

    /// Every page taken out and not yet freed: for a tree no one uses any
    /// more.
    pub(crate) fn all_retired(&self) -> Vec<PageId> {
        lock_builtin(&self.retired).drain(..).map(|(_, page)| page).collect()
    }
}

impl BLinkTree {
    /// Takes leaf `leaf`, emptied or left sparse by a delete of `key`, out
    /// of the tree, its entries merged into its right neighbour, if its
    /// left and right neighbours have the same parent, `path`'s last, and
    /// the entries fit (`merge.rs`); with no lock held.
    /// Then frees the pages no operation may hold any more, keeping those
    /// its file fails to free (`free_freeable`).
    ///
    /// # Errors
    ///
    /// The buffer pool's errors; SQLCODE -916 if a node is malformed.
    pub(super) fn unlink(&self, (leaf, key): (PageId, &[u8]), path: &Path) -> Result<()> {
        if let Some(&parent) = path.stack.last() {
            self.unlink_under((leaf, key), parent)?;
        }
        self.free_freeable();
        Ok(())
    }

    /// \[DESIGN\] Frees the pages taken out that no operation may hold any
    /// more (RULES.md D814). A page its file fails to free now, as a
    /// failed write zeroing it, is kept with those after it, to free at a
    /// later unlink: the tree no longer reaches it, so the delete that took
    /// it out stands, and until then it is only space. Failing the delete
    /// would fail the undo of an insert, which damages its table.
    fn free_freeable(&self) {
        let freeable = self.epochs.freeable();
        if freeable.is_empty() {
            return;
        }
        // Unnoted first, so no meta page notes a page freed (`retired.rs`).
        if self.unnote_retired(&freeable).is_err() {
            self.epochs.keep(freeable.into_iter());
            return;
        }
        let mut pages = freeable.into_iter();
        while let Some(page) = pages.next() {
            if self.pool.free_when_released(self.file, page).is_err() {
                let kept: Vec<PageId> = std::iter::once(page).chain(pages).collect();
                // Noted again; if that fails too, a crash costs their space.
                self.note_retired(&kept).unwrap_or_default();
                self.epochs.keep(kept.into_iter());
                return;
            }
        }
    }

    /// The child left of `leaf` under `parent`, and `leaf`'s entry, if
    /// `leaf` is one of its entries' children: not its first; and the
    /// child of the entry after, its right neighbour's, if it has one.
    /// The entry is found by `key`, one of the leaf's range, as a descent
    /// finds a child, not by a walk of the parent's entries, which a
    /// delete from a sparse leaf that cannot merge made at every delete.
    fn around(&self, parent: PageId, (leaf, key): (PageId, &[u8])) -> Result<Option<(PageId, usize, PageId)>> {
        let page = self.pool.read(self.file, parent.0)?;
        let node = Node::parse(&page)?;
        if node.level != 1 || node.deleted {
            return Ok(None);
        }
        // The entries below `key`: the last of them has its child.
        let below = node.search(key)?.unwrap_or_else(|at| at);
        let Some(at) = below.checked_sub(1) else { return Ok(None) };
        if node::child_of(node.entry(at)?.1)? != leaf {
            return Ok(None);
        }
        let left = match at.checked_sub(1) {
            Some(before) => node::child_of(node.entry(before)?.1)?,
            None => node.first_child(),
        };
        let Some(after) = Some(below).filter(|&after| after < node.len()) else { return Ok(None) };
        Ok(Some((left, at, node::child_of(node.entry(after)?.1)?)))
    }

    /// Whether `left`, `leaf` and `right` are neighbours in that order,
    /// none taken out.
    fn linked(&self, (left, leaf, right): (PageId, PageId, PageId)) -> Result<bool> {
        let (leaf_page, left_page) = (self.pool.read(self.file, leaf.0)?, self.pool.read(self.file, left.0)?);
        let (l, p) = (Node::parse(&leaf_page)?, Node::parse(&left_page)?);
        Ok(l.level == 0 && !l.deleted && !p.deleted && p.right == Some(leaf) && l.right == Some(right))
    }

    /// What merging `leaf` into `right` makes of `right` now; if `locked`
    /// is not, only whether it may be tried (`merge.rs`).
    fn merging(&self, leaf: PageId, right: PageId, locked: bool) -> Result<Merging> {
        let (leaf_page, right_page) = (self.pool.read(self.file, leaf.0)?, self.pool.read(self.file, right.0)?);
        let (l, r) = (Node::parse(&leaf_page)?, Node::parse(&right_page)?);
        if r.deleted || r.level != 0 || !self.may_merge(&l, &r) {
            return Ok(Merging::Full);
        }
        if locked { self.merged(&l, &r) } else { Ok(Merging::Range) }
    }

    /// `unlink`'s work under `parent`: nothing if a check fails. The leaf's
    /// entries, if it has any left, go to its right neighbour first.
    fn unlink_under(&self, (leaf, key): (PageId, &[u8]), parent: PageId) -> Result<()> {
        let Some((left, _, right)) = self.around(parent, (leaf, key))? else { return Ok(()) };
        if matches!(self.merging(leaf, right, false)?, Merging::Full) {
            return Ok(());
        }
        // Bottom to top, left to right, as every writer locks.
        let _locks = (self.lock(left)?, self.lock(leaf)?, self.lock(right)?, self.lock(parent)?);
        let Some((still_left, at, still_right)) = self.around(parent, (leaf, key))? else { return Ok(()) };
        if still_left != left || still_right != right || !self.linked((left, leaf, right))? {
            return Ok(());
        }
        match self.merging(leaf, right, true)? {
            Merging::Full => return Ok(()),
            Merging::Range => {}
            Merging::Content(content) => {
                *self.pool.write(self.file, right)? = content;
                super::count(&self.counters.merged);
            }
        }
        node::set_right(&mut self.pool.write(self.file, left)?, right)?;
        *self.pool.write(self.file, leaf)? = node::deleted(right, self.meta.0)?;
        {
            let mut page = self.pool.write(self.file, parent)?;
            node::set_child(&mut page, at, right)?;
            node::remove(&mut page, at.saturating_add(1))?;
        }
        self.epochs.retire(leaf);
        // Noted once the tree no longer reaches it; if that fails, a crash
        // costs its space (`retired.rs`).
        self.note_retired(&[leaf]).unwrap_or_default();
        super::count(&self.counters.unlinked);
        Ok(())
    }
}

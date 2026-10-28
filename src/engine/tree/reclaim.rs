//! \[DESIGN\] Empty leaves taken out of a B-link tree, and their pages freed
//! once no reader can hold them (RULES.md D31), after Lanin and Shasha, "A
//! Symmetric Concurrent B-Tree Algorithm", FJCC 1986: a leaf emptied by a
//! delete gives its range to a neighbour, and keeps a link to it, so a
//! reader or writer that reaches it goes where its range went.
//!
//! **Unlinking** a leaf L whose neighbours P, left, and R, right, have
//! its parent: P, L and the parent are locked in that order, bottom to top
//! and left to right, as every writer locks; then, each check made again
//! under the locks, P's right link is made R, L is marked deleted with its
//! link to R, and the parent sends L's range to R: L's entry's child made
//! R, and R's own entry taken out. A node is bounded only by its high key,
//! so R holds L's range without changing, and nothing grows. Whoever
//! reaches L, by a parent or a link read before, moves right to R, as past
//! any split. A leaf first or last under its parent stays, empty.
//!
//! **Epochs.** A page freed may be given to another node, so L's page is
//! freed only once no reader may still hold its number. Every operation on
//! the tree, and every scan while it lives, is counted in the epoch current
//! as it began, on the thread's stripe of counters, each on its own cache
//! line. The epoch moves on once no operation of the one before it is left;
//! a page taken out in epoch e is freed once the epoch is e + 2, when every
//! operation that began before it was taken out has ended.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use super::node::{self, Node};
use super::{BLinkTree, Path};
use crate::disk::pager::PageId;
use crate::error::Result;
use crate::sync::{lock_builtin, thread_stripe};

/// \[DESIGN\] Stripes of the epoch counters: more than the threads that use
/// a tree at once, so they seldom share one.
const STRIPES: usize = 64;

/// The operations of one stripe under way, by the parity of their epoch.
#[repr(align(128))]
#[derive(Default)]
struct Stripe([AtomicU64; 2]);

/// The epochs of a tree, and the pages taken out of it, to free.
pub(super) struct Epochs {
    current: AtomicU64,
    under_way: Box<[Stripe]>,
    retired: Mutex<Vec<(u64, PageId)>>,
}

/// An operation counted in its epoch: its stripe and the epoch's parity.
#[derive(Clone, Copy)]
pub(super) struct Entered {
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
    pub(super) fn new() -> Epochs {
        Epochs { current: AtomicU64::new(0), under_way: (0..STRIPES).map(|_| Stripe::default()).collect(), retired: Mutex::new(Vec::new()) }
    }

    /// Counts an operation beginning, in the epoch current: counted, and
    /// the epoch still current once it is, or counted again.
    pub(super) fn enter(&self) -> Entered {
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

    /// An operation guarded until the guard is dropped.
    pub(super) fn guard(&self) -> Guard<'_> {
        Guard { epochs: self, entered: self.enter() }
    }

    /// Counts the operation `entered` ended.
    pub(super) fn leave(&self, entered: Entered) {
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

    /// Notes `page` taken out, to free once no operation may hold it.
    pub(super) fn retire(&self, page: PageId) {
        let epoch = self.current.load(Ordering::SeqCst);
        lock_builtin(&self.retired).push((epoch, page));
    }

    /// The pages taken out that no operation may hold now, the epoch moved
    /// on first if it can: taken from those to free.
    pub(super) fn freeable(&self) -> Vec<PageId> {
        self.advance();
        let now = self.current.load(Ordering::SeqCst);
        let mut retired = lock_builtin(&self.retired);
        let (free, keep): (Vec<_>, Vec<_>) = retired.drain(..).partition(|(epoch, _)| epoch.saturating_add(2) <= now);
        *retired = keep;
        free.into_iter().map(|(_, page)| page).collect()
    }

    /// Every page taken out and not yet freed: for a tree no one uses any
    /// more.
    pub(super) fn all_retired(&self) -> Vec<PageId> {
        lock_builtin(&self.retired).drain(..).map(|(_, page)| page).collect()
    }
}

impl BLinkTree {
    /// Takes leaf `leaf`, emptied by a delete, out of the tree, if its left
    /// neighbour has the same parent, `path`'s last; with no lock held.
    /// Then frees the pages no operation may hold any more.
    ///
    /// # Errors
    ///
    /// The buffer pool's errors; SQLCODE -916 if a node is malformed.
    pub(super) fn unlink(&self, leaf: PageId, path: &Path) -> Result<()> {
        if let Some(&parent) = path.stack.last() {
            self.unlink_under(leaf, parent)?;
        }
        for page in self.epochs.freeable() {
            self.pool.free_when_released(self.file, page)?;
        }
        Ok(())
    }

    /// The child left of `leaf` under `parent`, and `leaf`'s entry, if
    /// `leaf` is one of its entries' children: not its first; and the
    /// child of the entry after, its right neighbour's, if it has one.
    fn around(&self, parent: PageId, leaf: PageId) -> Result<Option<(PageId, usize, PageId)>> {
        let page = self.pool.read(self.file, parent.0)?;
        let node = Node::parse(&page)?;
        if node.level != 1 || node.deleted {
            return Ok(None);
        }
        for at in 0..node.len() {
            if node::child_of(node.entry(at)?.1)? != leaf {
                continue;
            }
            let left = match at.checked_sub(1) {
                Some(before) => node::child_of(node.entry(before)?.1)?,
                None => node.first_child(),
            };
            let Some(after) = at.checked_add(1).filter(|&after| after < node.len()) else { return Ok(None) };
            return Ok(Some((left, at, node::child_of(node.entry(after)?.1)?)));
        }
        Ok(None)
    }

    /// Whether `left`, `leaf` and `right` are neighbours in that order,
    /// `leaf` empty, none taken out.
    fn linked(&self, (left, leaf, right): (PageId, PageId, PageId)) -> Result<bool> {
        let (leaf_page, left_page) = (self.pool.read(self.file, leaf.0)?, self.pool.read(self.file, left.0)?);
        let (l, p) = (Node::parse(&leaf_page)?, Node::parse(&left_page)?);
        Ok(l.len() == 0 && l.level == 0 && !l.deleted && !p.deleted && p.right == Some(leaf) && l.right == Some(right))
    }

    /// `unlink`'s work under `parent`: nothing if a check fails.
    fn unlink_under(&self, leaf: PageId, parent: PageId) -> Result<()> {
        let Some((left, _, _)) = self.around(parent, leaf)? else { return Ok(()) };
        // Bottom to top, left to right, as every writer locks.
        let (_left_lock, _leaf_lock, _parent_lock) = (self.locks.lock(left)?, self.locks.lock(leaf)?, self.locks.lock(parent)?);
        let Some((still_left, at, right)) = self.around(parent, leaf)? else { return Ok(()) };
        if still_left != left || !self.linked((left, leaf, right))? {
            return Ok(());
        }
        node::set_right(&mut self.pool.write(self.file, left)?, right)?;
        *self.pool.write(self.file, leaf)? = node::deleted(right)?;
        {
            let mut page = self.pool.write(self.file, parent)?;
            node::set_child(&mut page, at, right)?;
            node::remove(&mut page, at.saturating_add(1))?;
        }
        self.epochs.retire(leaf);
        super::count(&self.counters.unlinked);
        Ok(())
    }
}

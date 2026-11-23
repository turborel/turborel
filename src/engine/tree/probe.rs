//! \[DESIGN\] A probe from a key (RULES.md D859): the entries at or after
//! a key, shown where they lie, in key order, until the caller has seen
//! enough. A probe of a unique key was a scan, which keeps its bounds as
//! bytes of its own, two allocations for a read of one entry, and copies
//! what it takes. A probe borrows its key, shows each entry under its
//! leaf's latch, or from the thread's copy of the leaf (`leaves.rs`), and
//! puts each key together in a buffer its thread keeps. So one that ends
//! in the leaf it began in allocates nothing.
//!
//! Past a leaf it moves right as a scan does (Lehman and Yao 1981, §4):
//! it shows of each leaf only the entries past the last key it showed,
//! which the buffer still holds, so a leaf merged into the next since
//! shows none twice (`merge.rs`).
//!
//! It is a lookup in the tree's tally, and each leaf it reads is noted for
//! EXPLAIN ANALYZE, as a scan from a key is (RULES.md D621).

use std::cell::Cell;

use super::node::Node;
use super::scan::Visit;
use super::{BLinkTree, Path, Target, corrupt, leaves};
use crate::disk::pager::PageId;
use crate::error::Result;
use crate::noted::{self, Noted};

/// \[DESIGN\] The most bytes of the buffer a thread keeps for the keys its
/// probes put together: a key is at most a quarter of a page, and the
/// largest page is 64 KiB.
const KEPT_BYTES: usize = 65_536;

thread_local! {
    /// The buffer of the thread's last probe, for its next.
    static KEY: Cell<Vec<u8>> = const { Cell::new(Vec::new()) };
}

/// What a probe has shown: the last key, whole, if it has shown any.
struct Shown {
    last: Vec<u8>,
    any: bool,
}

/// Shows `f` the entries of `leaf` at or after `key`, past the last key
/// shown if one was, until `f` has enough. The leaf to go on to: its right
/// neighbour, unless `f` stopped or it has none.
fn show(leaf: &Node<'_>, key: &[u8], shown: &mut Shown, f: &mut impl FnMut(&[u8], &[u8]) -> Result<Visit>) -> Result<Option<PageId>> {
    if leaf.level != 0 {
        return Err(corrupt("a leaf's neighbour is not a leaf"));
    }
    // Taken out of the tree, its range is in the node its link leads to.
    if leaf.deleted {
        return Ok(leaf.right);
    }
    let start = if shown.any {
        leaf.search(&shown.last)?.map_or_else(|at| at, |at| at.saturating_add(1))
    } else {
        leaf.search(key)?.unwrap_or_else(|at| at)
    };
    for at in start..leaf.len() {
        let (entry, value) = leaf.entry(at)?;
        shown.last.clear();
        entry.put(&mut shown.last);
        shown.any = true;
        match f(&shown.last, value)? {
            Visit::Taken => {}
            Visit::Later | Visit::Stop => return Ok(None),
        }
    }
    Ok(leaf.right)
}

/// What came of a probe's walk through the inner nodes' copies.
enum ByCopies {
    /// The entries of the key's leaf shown, and the leaf to go on to.
    Shown(Option<PageId>),
    /// Nothing shown: the probe descends.
    Descend,
}

impl BLinkTree {
    /// \[DESIGN\] Shows `f` the entries with keys at or after `key`, in key
    /// order, each key and value borrowed where it lies, until `f` answers
    /// anything but `Visit::Taken` or no entry is left (RULES.md D859).
    /// `f` runs under a leaf's latch, so it must not use the tree nor
    /// wait. The key is borrowed, and nothing is allocated once the thread
    /// has probed a key as long.
    ///
    /// # Errors
    ///
    /// `f`'s errors; as `get`.
    pub fn first_from(&self, key: &[u8], mut f: impl FnMut(&[u8], &[u8]) -> Result<Visit>) -> Result<()> {
        let _guard = self.epochs.guard();
        let mut shown = Shown { last: KEY.take(), any: false };
        let done = self.probe(key, &mut shown, &mut f);
        if shown.last.capacity() <= KEPT_BYTES {
            KEY.set(shown.last);
        }
        done
    }

    /// \[DESIGN\] (RULES.md D955) Shows `f` the entries of `key`'s leaf
    /// as `show` does, if the thread's copies of the inner nodes lead to
    /// it (`inner.rs`) and it is a leaf of the tree, in it, whose range
    /// holds `key`: the leaf to go on to. `Descend` if not, nothing shown;
    /// the probe descends then.
    fn shown_by_copies(&self, key: &[u8], shown: &mut Shown, f: &mut impl FnMut(&[u8], &[u8]) -> Result<Visit>) -> Result<ByCopies> {
        let Some(leaf) = self.leaf_by_copies(key) else { return Ok(ByCopies::Descend) };
        let page = leaves::read(&self.pool, (self.file, leaf), Some(&self.tally))?;
        let node = page.node()?;
        if node.level != 0 || node.deleted || node.tree != self.meta.0 || !node.covers(key) {
            return Ok(ByCopies::Descend);
        }
        self.tally.lookup();
        show(&node, key, shown, f).map(ByCopies::Shown)
    }

    /// `first_from`, its keys put together in `shown`.
    fn probe(&self, key: &[u8], shown: &mut Shown, f: &mut impl FnMut(&[u8], &[u8]) -> Result<Visit>) -> Result<()> {
        let notes = noted::noting();
        let mut next = match self.shown_by_copies(key, shown, f)? {
            ByCopies::Shown(next) => next,
            ByCopies::Descend => {
                let mut path = Path { tallied: true, ..Path::default() };
                self.descend(Target::Key(key), 0, &mut path, |leaf| show(leaf, key, &mut *shown, &mut *f))?.1
            }
        };
        noted::add(notes.as_deref(), Noted::Leaves, 1);
        while let Some(right) = next {
            let page = self.pool.read(self.file, right.0)?;
            next = show(&Node::parse(&page)?, key, shown, f)?;
            noted::add(notes.as_deref(), Noted::Leaves, 1);
        }
        Ok(())
    }
}

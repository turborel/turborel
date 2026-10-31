//! \[DESIGN\] Range scans of a B-link tree, in either direction, locking
//! nothing (RULES.md D31). A scan copies the entries it wants from one leaf
//! at a time, under that leaf's latch, and returns them before reading the
//! next, so it holds no latch between entries.
//!
//! * **Forward**, it follows right links. Keys only ever move right, so
//!   every key past the last one returned is in the leaf linked to, or
//!   further right (Lehman and Yao 1981, §4). Each leaf's entries are taken
//!   past the last key returned, so none is returned twice. A leaf linked
//!   to from the page before it in the file, as a tree filled in key order
//!   lays its leaves, is read with the pages after it, ahead of the scan
//!   (`BufferPool::read_ahead`).
//! * **Backward**, it goes down from the root again for each leaf: to the
//!   leaf holding the bound, then to the one holding the key below that
//!   leaf's range, found on the way down (a parent's key, or a left
//!   neighbour's high key). Leaves have no left links, as keeping them
//!   would make a split change a third node, which the paper's proofs do
//!   not cover (§6.2, Lemma 2).
//!
//! Entries added or removed while a scan runs may or may not be seen, as
//! the scan reaches their leaf before or after the change; isolation is
//! the transaction layer's (RULES.md D29).

use std::ops::{Bound, Deref};

use super::node::Node;
use super::reclaim::Entered;
use super::{BLinkTree, Path, Target, corrupt};
use crate::disk::pager::PageId;
use crate::error::Result;

/// The order a scan returns entries in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Ascending keys.
    Forward,
    /// Descending keys.
    Backward,
}

/// Where a scan reads next.
#[derive(Clone, Copy)]
enum Next {
    /// Down from the root, to the leaf holding the bound in its direction.
    Descend,
    /// The leaf right of the last one read.
    Leaf(PageId),
    /// Nowhere: it is done.
    Done,
}

/// An entry: its key and value.
type Entry = (Vec<u8>, Vec<u8>);

/// \[DESIGN\] A scan of a tree's entries with keys within bounds: an
/// iterator of keys and values. It reads the tree through `T`: a borrow,
/// or something that owns the tree, so the scan can outlive the call that
/// started it (`scan_of`).
pub struct Scan<T: Deref<Target = BLinkTree>> {
    tree: T,
    /// Keys at or above this. Forward, the last key returned, once one is.
    lower: Bound<Vec<u8>>,
    /// Keys at or below this. Backward, the key below the last leaf read.
    upper: Bound<Vec<u8>>,
    direction: Direction,
    /// Entries copied from the last leaf, the next to return last.
    ready: Vec<Entry>,
    next: Next,
    /// The leaf it read last, forward: a leaf after it in the file, linked
    /// to, is read ahead.
    last: Option<PageId>,
    /// Its epoch, counted while it lives: it holds a leaf's page number
    /// between reads (`reclaim.rs`).
    entered: Entered,
}

impl<T: Deref<Target = BLinkTree>> Drop for Scan<T> {
    fn drop(&mut self) {
        self.tree.epochs.leave(self.entered);
    }
}

impl BLinkTree {
    /// The entries with keys within `lower` and `upper`, in `direction`.
    pub fn scan(&self, lower: Bound<&[u8]>, upper: Bound<&[u8]>, direction: Direction) -> Scan<&Self> {
        scan_of(self, lower, upper, direction)
    }
}

/// The entries of the tree `tree` reaches, with keys within `lower` and
/// `upper`, in `direction`.
pub fn scan_of<T: Deref<Target = BLinkTree>>(tree: T, lower: Bound<&[u8]>, upper: Bound<&[u8]>, direction: Direction) -> Scan<T> {
    let entered = tree.epochs.enter();
    Scan {
        tree,
        lower: lower.map(<[u8]>::to_vec),
        upper: upper.map(<[u8]>::to_vec),
        direction,
        ready: Vec::new(),
        next: Next::Descend,
        last: None,
        entered,
    }
}

/// Whether `key` is at or above `bound`, a lower bound.
fn above(key: &[u8], bound: &Bound<Vec<u8>>) -> bool {
    match bound {
        Bound::Unbounded => true,
        Bound::Included(b) => key >= b.as_slice(),
        Bound::Excluded(b) => key > b.as_slice(),
    }
}

/// Whether `key` is at or below `bound`, an upper bound.
fn below(key: &[u8], bound: &Bound<Vec<u8>>) -> bool {
    match bound {
        Bound::Unbounded => true,
        Bound::Included(b) => key <= b.as_slice(),
        Bound::Excluded(b) => key < b.as_slice(),
    }
}

/// What a scan takes from a leaf.
struct Taken {
    /// Its entries within the bounds, in key order.
    found: Vec<Entry>,
    /// Its right neighbour, if keys past its high key may be within the
    /// upper bound.
    further: Option<PageId>,
}

/// Takes what is wanted from `leaf`.
fn take(leaf: &Node<'_>, lower: &Bound<Vec<u8>>, upper: &Bound<Vec<u8>>) -> Result<Taken> {
    if leaf.level != 0 {
        return Err(corrupt("a leaf's neighbour is not a leaf"));
    }
    // Taken out of the tree: its range is in the node its link leads to.
    if leaf.deleted {
        return Ok(Taken { found: Vec::new(), further: leaf.right });
    }
    let start = match lower {
        Bound::Unbounded => 0,
        Bound::Included(b) => leaf.search(b)?.unwrap_or_else(|at| at),
        Bound::Excluded(b) => leaf.search(b)?.map_or_else(|at| at, |at| at.saturating_add(1)),
    };
    let mut found = Vec::new();
    for at in start..leaf.len() {
        let (key, value) = leaf.entry(at)?;
        if !below(key, upper) {
            break;
        }
        found.push((key.to_vec(), value.to_vec()));
    }
    let past = |high: &[u8]| match upper {
        Bound::Unbounded => true,
        Bound::Included(b) | Bound::Excluded(b) => high < b.as_slice(),
    };
    Ok(Taken { found, further: leaf.right.filter(|_| leaf.high.is_some_and(past)) })
}

impl<T: Deref<Target = BLinkTree>> Scan<T> {
    /// Reads the next leaf: its entries made ready, and where to read next
    /// decided.
    fn advance(&mut self) -> Result<()> {
        let tree = &*self.tree;
        let (mut taken, low) = match self.next {
            Next::Done => return Ok(()),
            Next::Leaf(id) => {
                if self.last.and_then(|last| last.0.checked_add(1)) == Some(id.0) {
                    tree.pool.read_ahead(tree.file, id.0);
                }
                self.last = Some(id);
                let page = tree.pool.read(tree.file, id.0)?;
                (take(&Node::parse(&page)?, &self.lower, &self.upper)?, None)
            }
            Next::Descend => {
                let bound = match self.direction {
                    Direction::Forward => &self.lower,
                    Direction::Backward => &self.upper,
                };
                let target = match (bound, self.direction) {
                    (Bound::Included(key) | Bound::Excluded(key), _) => Target::Key(key),
                    (Bound::Unbounded, Direction::Forward) => Target::First,
                    (Bound::Unbounded, Direction::Backward) => Target::Last,
                };
                let mut path = Path { track_low: true, ..Path::default() };
                let (leaf, taken) = tree.descend(target, 0, &mut path, |leaf| take(leaf, &self.lower, &self.upper))?;
                self.last = Some(leaf);
                (taken, path.low)
            }
        };
        self.next = Next::Done;
        match self.direction {
            Direction::Forward => {
                if let Some((key, _)) = taken.found.last() {
                    self.lower = Bound::Excluded(key.clone());
                }
                taken.found.reverse();
                self.next = taken.further.map_or(Next::Done, Next::Leaf);
            }
            Direction::Backward => {
                // The next leaf down holds `low`, if keys that low are
                // wanted; the leftmost leaf has none below it.
                if let Some(low) = low.filter(|low| above(low, &self.lower)) {
                    self.upper = Bound::Included(low);
                    self.next = Next::Descend;
                }
            }
        }
        self.ready = taken.found;
        Ok(())
    }
}

impl<T: Deref<Target = BLinkTree>> Iterator for Scan<T> {
    type Item = Result<Entry>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(entry) = self.ready.pop() {
                return Some(Ok(entry));
            }
            if matches!(self.next, Next::Done) {
                return None;
            }
            if let Err(e) = self.advance() {
                self.next = Next::Done;
                return Some(Err(e));
            }
        }
    }
}

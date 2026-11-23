//! \[DESIGN\] Range scans of a B-link tree, in either direction, locking
//! nothing (RULES.md D31). A scan copies the entries it wants from one leaf
//! at a time, under that leaf's latch, and returns them before reading the
//! next, so it holds no latch between entries.
//!
//! * **Forward**, it follows right links. Keys only ever move right, so
//!   every key past the last one returned is in the leaf linked to, or
//!   further right (Lehman and Yao 1981, §4). Each leaf's entries are taken
//!   past the last key returned, so none is returned twice. Reading a leaf
//!   linked to from the page before it in the file, as a tree filled in
//!   key order lays its leaves, it asks for the pages after it to be read
//!   ahead (`BufferPool::ahead_of`): by the database's read-ahead job while
//!   it goes on, those it has not read passed over once it ends.
//! * **Backward**, it goes down from the root again for each leaf: to the
//!   leaf holding the bound, then to the one holding the key below that
//!   leaf's range, found on the way down (a parent's key, or a left
//!   neighbour's high key), or just below the lowest key it took, if a
//!   leaf merged into it since holds keys at or below that (`merge.rs`).
//!   Leaves have no left links, as keeping them
//!   would make a split change a third node, which the paper's proofs do
//!   not cover (§6.2, Lemma 2).
//!
//! \[EXTENSION\] A scan made for an operator EXPLAIN ANALYZE counts notes
//! each leaf it reads of it (`noted.rs`, RULES.md D621).
//!
//! Entries added or removed while a scan runs may or may not be seen, as
//! the scan reaches their leaf before or after the change; isolation is
//! the transaction layer's (RULES.md D29).

use std::ops::{Bound, Deref};
use std::sync::Arc;

use super::node::{KeyRef, Node};
use super::reclaim::Entered;
use super::{BLinkTree, Path, Target, corrupt};
use crate::disk::buffer::Ahead;
use crate::disk::pager::PageId;
use crate::error::Result;
use crate::noted::{self, Noted, Notes};

mod ready;
mod visit;

pub use visit::{Visit, Visited};

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

/// An entry lent by `Scan::next_entry`, its key and value, with what the
/// scan reads the tree through.
pub type Lent<'s, T> = (&'s T, (&'s [u8], &'s [u8]));

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
    /// Entries copied from the last leaf, not yet given (`ready.rs`).
    ready: ready::Ready,
    next: Next,
    /// The leaf it read last, forward: a leaf after it in the file, linked
    /// to, has the pages after it read ahead.
    last: Option<PageId>,
    /// What it asked to be read ahead.
    ahead: Ahead,
    /// Its epoch, counted while it lives: it holds a leaf's page number
    /// between reads (`reclaim.rs`).
    entered: Entered,
    /// The notes of the operator it reads for, if it is counted.
    notes: Option<Arc<Notes>>,
}

impl<T: Deref<Target = BLinkTree>> Drop for Scan<T> {
    fn drop(&mut self) {
        self.tree.epochs.leave(self.entered);
        std::mem::take(&mut self.ready).give_back();
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
    scan_owned(tree, lower.map(<[u8]>::to_vec), upper.map(<[u8]>::to_vec), direction)
}

/// As `scan_of`, its bounds given as the scan keeps them, so a caller that
/// built them hands them over with no copy (RULES.md D839).
pub fn scan_owned<T: Deref<Target = BLinkTree>>(tree: T, lower: Bound<Vec<u8>>, upper: Bound<Vec<u8>>, direction: Direction) -> Scan<T> {
    let entered = tree.epochs.enter();
    Scan {
        tree,
        lower,
        upper,
        direction,
        ready: ready::Ready::reused(),
        next: Next::Descend,
        last: None,
        ahead: Ahead::default(),
        entered,
        notes: noted::noting(),
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
fn below(key: KeyRef<'_>, bound: &Bound<Vec<u8>>) -> bool {
    match bound {
        Bound::Unbounded => true,
        Bound::Included(b) => key.cmp_bytes(b) != std::cmp::Ordering::Greater,
        Bound::Excluded(b) => key.cmp_bytes(b) == std::cmp::Ordering::Less,
    }
}

/// The place of `leaf`'s first entry at or above `lower`.
fn start_of(leaf: &Node<'_>, lower: &Bound<Vec<u8>>) -> Result<usize> {
    Ok(match lower {
        Bound::Unbounded => 0,
        Bound::Included(b) => leaf.search(b)?.unwrap_or_else(|at| at),
        Bound::Excluded(b) => leaf.search(b)?.map_or_else(|at| at, |at| at.saturating_add(1)),
    })
}

/// `leaf`'s right neighbour, if keys past its high key may be within
/// `upper`.
fn further_of(leaf: &Node<'_>, upper: &Bound<Vec<u8>>) -> Option<PageId> {
    let past = |high: &[u8]| match upper {
        Bound::Unbounded => true,
        Bound::Included(b) | Bound::Excluded(b) => high < b.as_slice(),
    };
    leaf.right.filter(|_| leaf.high.is_some_and(past))
}

/// How many of `leaf`'s entries are within the bounds, found by their
/// places, no entry copied, its right neighbour, as `take`'s, and its high
/// key, past which the next leaf's are counted.
fn tally(leaf: &Node<'_>, lower: &Bound<Vec<u8>>, upper: &Bound<Vec<u8>>) -> Result<(usize, Option<PageId>, Option<Vec<u8>>)> {
    if leaf.level != 0 {
        return Err(corrupt("a leaf's neighbour is not a leaf"));
    }
    if leaf.deleted {
        return Ok((0, leaf.right, None));
    }
    let start = start_of(leaf, lower)?;
    let end = match upper {
        Bound::Unbounded => leaf.len(),
        Bound::Included(b) => leaf.search(b)?.map_or_else(|at| at, |at| at.saturating_add(1)),
        Bound::Excluded(b) => leaf.search(b)?.unwrap_or_else(|at| at),
    };
    Ok((end.saturating_sub(start), further_of(leaf, upper), leaf.high.map(<[u8]>::to_vec)))
}

/// Takes what is wanted from `leaf` into `ready`, cleared first; its right
/// neighbour, if keys past its high key may be within `upper`.
fn take(leaf: &Node<'_>, (lower, upper): (&Bound<Vec<u8>>, &Bound<Vec<u8>>), ready: &mut ready::Ready) -> Result<Option<PageId>> {
    if leaf.level != 0 {
        return Err(corrupt("a leaf's neighbour is not a leaf"));
    }
    ready.clear();
    // Taken out of the tree, its range is in the node its link leads to.
    if leaf.deleted {
        return Ok(leaf.right);
    }
    for at in start_of(leaf, lower)?..leaf.len() {
        let (key, value) = leaf.entry(at)?;
        if !below(key, upper) {
            break;
        }
        ready.push(key, value);
    }
    Ok(further_of(leaf, upper))
}

impl<T: Deref<Target = BLinkTree>> Scan<T> {
    /// Reads the next leaf: its entries made ready, and where to read next
    /// decided.
    fn advance(&mut self) -> Result<()> {
        let tree = &*self.tree;
        let ready = &mut self.ready;
        let (further, low) = match self.next {
            Next::Done => return Ok(()),
            Next::Leaf(id) => {
                if self.last.and_then(|last| last.0.checked_add(1)) == Some(id.0) {
                    tree.pool.ahead_of(&mut self.ahead, tree.file, id.0);
                }
                self.last = Some(id);
                let page = tree.pool.read(tree.file, id.0)?;
                (take(&Node::parse(&page)?, (&self.lower, &self.upper), ready)?, None)
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
                // A lookup: its first descent, to a key; a backward scan's
                // next ones go on from where it was.
                let tallied = self.last.is_none() && matches!(target, Target::Key(_));
                // Only a backward scan goes on from the key below the leaf,
                // so only it copies that key on the way down.
                let track_low = matches!(self.direction, Direction::Backward);
                let mut path = Path { track_low, tallied, ..Path::default() };
                let bounds = (&self.lower, &self.upper);
                let (leaf, further) = tree.descend(target, 0, &mut path, |leaf| take(leaf, bounds, ready))?;
                self.last = Some(leaf);
                (further, path.low)
            }
        };
        self.next = Next::Done;
        noted::add(self.notes.as_deref(), Noted::Leaves, 1);
        match self.direction {
            Direction::Forward => {
                if let Some(key) = self.ready.last_key() {
                    past(&mut self.lower, key);
                }
                self.next = further.map_or(Next::Done, Next::Leaf);
            }
            Direction::Backward => {
                // The next leaf down holds `low`, if keys that low are
                // wanted; the leftmost leaf has none below it. A leaf the
                // one below merged into since its parent was read holds
                // that one's keys too, all taken: the next read takes
                // those below the lowest (`merge.rs`).
                if let Some(low) = low.filter(|low| above(low, &self.lower)) {
                    self.upper = below_taken(self.ready.first_key(), low);
                    self.next = Next::Descend;
                }
            }
        }
        Ok(())
    }
}

/// A backward scan's next upper bound, after a leaf whose lowest key
/// taken is `first`, and whose range was known to begin above `low`: at
/// or below `low`, unless keys that low were taken, from a leaf merged
/// into it since its parent was read (`merge.rs`); then below the lowest.
pub(super) fn below_taken(first: Option<&[u8]>, low: Vec<u8>) -> Bound<Vec<u8>> {
    match first {
        Some(first) if first <= low.as_slice() => Bound::Excluded(first.to_vec()),
        _ => Bound::Included(low),
    }
}

/// Makes `lower` the bound just past `key`, in its own bytes when it has
/// some, so a forward scan allocates no bound per leaf.
fn past(lower: &mut Bound<Vec<u8>>, key: &[u8]) {
    match lower {
        Bound::Included(bytes) | Bound::Excluded(bytes) => {
            bytes.clear();
            bytes.extend_from_slice(key);
            *lower = Bound::Excluded(std::mem::take(bytes));
        }
        Bound::Unbounded => *lower = Bound::Excluded(key.to_vec()),
    }
}

impl<T: Deref<Target = BLinkTree>> Scan<T> {
    /// How many entries a forward scan would give, none known past `most`,
    /// read a leaf at a time as the scan is, no entry copied.
    ///
    /// # Errors
    ///
    /// As reading a leaf's.
    pub fn counted(mut self, most: usize) -> Result<Option<usize>> {
        let mut counted = 0_usize;
        while let Some(found) = self.count_leaf()? {
            counted = counted.saturating_add(found);
            if counted > most {
                return Ok(None);
            }
        }
        Ok(Some(counted))
    }

    /// `counted`'s step: the entries of the next leaf within the bounds,
    /// none once the scan is done, and where to count next decided.
    pub(super) fn count_leaf(&mut self) -> Result<Option<usize>> {
        let tree = &*self.tree;
        let (found, further, high) = match self.next {
            Next::Done => return Ok(None),
            Next::Leaf(id) => tally(&Node::parse(&tree.pool.read(tree.file, id.0)?)?, &self.lower, &self.upper)?,
            Next::Descend => {
                let target = match &self.lower {
                    Bound::Included(key) | Bound::Excluded(key) => Target::Key(key),
                    Bound::Unbounded => Target::First,
                };
                let mut path = Path { tallied: matches!(target, Target::Key(_)), ..Path::default() };
                tree.descend(target, 0, &mut path, |leaf| tally(leaf, &self.lower, &self.upper))?.1
            }
        };
        // Keys of a leaf merged into the next since it was counted are at
        // or below its high key (`merge.rs`): counted once.
        if let Some(high) = high {
            self.lower = Bound::Excluded(high);
        }
        self.next = further.map_or(Next::Done, Next::Leaf);
        Ok(Some(found))
    }
}

impl<T: Deref<Target = BLinkTree>> Scan<T> {
    /// The next entry, its key and value borrowed until the scan is used
    /// again, so a reader done with each before the next copies none
    /// (RULES.md D809), and what the scan reads the tree through.
    ///
    /// # Errors
    ///
    /// As reading a leaf's; the scan then ends.
    pub fn next_entry(&mut self) -> Option<Result<Lent<'_, T>>> {
        while self.ready.is_empty() {
            if matches!(self.next, Next::Done) {
                return None;
            }
            if let Err(e) = self.advance() {
                self.next = Next::Done;
                self.ready.clear();
                return Some(Err(e));
            }
        }
        let (key, value) = self.ready.give(self.direction)?;
        // A table's row has the leaf it was read from noted for its index
        // entries (`hints.rs`).
        if let (Some(row), Some(leaf)) = (self.tree.row_of(key), self.last) {
            super::hints::note(row, leaf.0);
        }
        Some(Ok((&self.tree, (key, value))))
    }
}

impl<T: Deref<Target = BLinkTree>> Iterator for Scan<T> {
    type Item = Result<Entry>;

    fn next(&mut self) -> Option<Self::Item> {
        Some(self.next_entry()?.map(|(_, (key, value))| (key.to_vec(), value.to_vec())))
    }
}

//! \[DESIGN\] Streaming intersection joins of two R-link trees. A pair
//! of branches is pruned when its rectangles are disjoint. Otherwise one
//! side descends, bounding the pending work by tree depth and fanout,
//! rather than materializing a Cartesian product of child pointers.
//! Split chains partition each side's entries independently: the right
//! sibling of the left side is paired with the entire original right
//! group; the right side's sibling is paired with the current left node.

use super::node::{Entry, Node};
use super::{Pin, RTree, Visit, corrupt};
use crate::disk::pager::PageId;
use crate::error::{Code, Result, SqlError};
use crate::memory::{MemoryBudget, Reservation};
use crate::spatial::Bounds;
use crate::storage::RowId;
use std::sync::Arc;

/// One intersecting pair, retaining both bounds for version checks and
/// exact geometry rechecks by the consumer.
pub type Joined = ((Bounds, RowId), (Bounds, RowId));

/// A streamed Cartesian intersection join, including both directions and
/// self-pairs when a tree is joined to itself. Entries held throughout
/// appear once per matching pair; concurrent changes are optional.
pub struct Join {
    left: RTree,
    right: RTree,
    stack: Vec<Pair>,
    leaves: Option<Leaves>,
    held: Reservation,
    /// Two decoded pages are accounted for before either is read.
    pages_bytes: usize,
    failed: bool,
    _pins: (Pin, Pin),
}

/// Initial pending pairs; later growth is geometric.
const INITIAL_CAPACITY: usize = 64;
const GROWTH: usize = 2;

#[derive(Clone, Copy)]
struct Pair {
    left: Visit,
    right: Visit,
    left_hops: u64,
    right_hops: u64,
}

#[derive(Clone, Copy)]
enum Side {
    Left,
    Right,
}

struct Leaves {
    left: Vec<Entry>,
    right: Vec<Entry>,
    i: usize,
    j: usize,
}

impl Leaves {
    fn next(&mut self) -> Option<Joined> {
        while let Some(left) = self.left.get(self.i) {
            if let Some(right) = self.right.get(self.j) {
                self.j = self.j.saturating_add(1);
                if left.bounds.intersects(right.bounds) {
                    return Some(((left.bounds, left.id), (right.bounds, right.id)));
                }
            } else {
                self.i = self.i.saturating_add(1);
                self.j = 0;
            }
        }
        None
    }
}

impl Join {
    pub(super) fn new(left: RTree, right: RTree, budget: &Arc<MemoryBudget>) -> Result<Join> {
        let pages_bytes =
            left.inner.capacity.checked_add(right.inner.capacity).and_then(|n| n.checked_mul(size_of::<Entry>())).ok_or_else(exhausted)?;
        let held = budget.reserve(pages_bytes, "the R-tree intersection join")?;
        let first = Pair { left: left.root()?, right: right.root()?, left_hops: 0, right_hops: 0 };
        let mut join =
            Join { _pins: (left.pin(), right.pin()), left, right, stack: Vec::new(), leaves: None, held, pages_bytes, failed: false };
        join.push(first)?;
        Ok(join)
    }

    fn push(&mut self, pair: Pair) -> Result<()> {
        if self.stack.len() == self.stack.capacity() {
            let capacity = self.stack.capacity().checked_mul(GROWTH).ok_or_else(exhausted)?.max(INITIAL_CAPACITY);
            let bytes = capacity.checked_mul(size_of::<Pair>()).and_then(|n| n.checked_add(self.pages_bytes)).ok_or_else(exhausted)?;
            self.held.resize(bytes)?;
            self.stack.try_reserve_exact(capacity.saturating_sub(self.stack.len())).map_err(|_| exhausted())?;
        }
        self.stack.push(pair);
        Ok(())
    }

    fn expand(&mut self, pair: Pair) -> Result<()> {
        let left = self.left.read(pair.left.id)?;
        let right = self.right.read(pair.right.id)?;
        if left.level != pair.left.level || right.level != pair.right.level {
            return Err(corrupt("an intersection join child is at the wrong level"));
        }
        let current = self.siblings(pair, (&left, &right))?;
        let Some((a, b)) = left.bounds().zip(right.bounds()) else { return Ok(()) };
        if !a.intersects(b) {
            return Ok(());
        }
        if left.level == 0 && right.level == 0 {
            self.leaves = Some(Leaves { left: left.entries, right: right.entries, i: 0, j: 0 });
            return Ok(());
        }
        if left.level >= right.level { self.children(current, left, Side::Left, b) } else { self.children(current, right, Side::Right, a) }
    }

    /// Normalize the pair into two current nodes and disjoint remaining
    /// sibling groups. Using the original left group for the right
    /// sibling as well would visit their right/right product twice.
    fn siblings(&mut self, pair: Pair, (left, right): (&Node, &Node)) -> Result<Pair> {
        if left.stamp != pair.left.stamp {
            let id = left.right.ok_or_else(|| corrupt("a join's left split has no sibling"))?;
            let hops = pair.left_hops.saturating_add(1);
            if hops > self.left.inner.pool.pages_in_use(self.left.inner.file)? {
                return Err(corrupt("a join's left sibling chain contains a cycle"));
            }
            self.push(Pair { left: Visit { id, ..pair.left }, left_hops: hops, ..pair })?;
        }
        let current_left = Visit { stamp: left.stamp, ..pair.left };
        if right.stamp != pair.right.stamp {
            let id = right.right.ok_or_else(|| corrupt("a join's right split has no sibling"))?;
            let hops = pair.right_hops.saturating_add(1);
            if hops > self.right.inner.pool.pages_in_use(self.right.inner.file)? {
                return Err(corrupt("a join's right sibling chain contains a cycle"));
            }
            self.push(Pair { left: current_left, right: Visit { id, ..pair.right }, right_hops: hops, ..pair })?;
        }
        Ok(Pair { left: current_left, right: Visit { stamp: right.stamp, ..pair.right }, ..pair })
    }

    fn children(&mut self, pair: Pair, node: Node, side: Side, opposite: Bounds) -> Result<()> {
        let level = node.level.checked_sub(1).ok_or_else(|| corrupt("a leaf is expanded as a branch"))?;
        for entry in node.entries.into_iter().rev() {
            if entry.bounds.intersects(opposite) {
                let child = Visit { id: PageId(entry.id), stamp: entry.value, level };
                let next = match side {
                    Side::Left => Pair { left: child, left_hops: 0, ..pair },
                    Side::Right => Pair { right: child, right_hops: 0, ..pair },
                };
                self.push(next)?;
            }
        }
        Ok(())
    }
}

impl Iterator for Join {
    type Item = Result<Joined>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed {
            return None;
        }
        loop {
            if let Some(leaves) = &mut self.leaves {
                if let Some(pair) = leaves.next() {
                    return Some(Ok(pair));
                }
                self.leaves = None;
            }
            let pair = self.stack.pop()?;
            if let Err(error) = self.expand(pair) {
                self.failed = true;
                self.stack.clear();
                return Some(Err(error));
            }
        }
    }
}

impl RTree {
    /// The hull of every entry, its unfinished splits' too, or none of an
    /// empty tree: ancestors are enlarged before an entry is inserted, so
    /// it holds each entry committed before the call (`nearest.rs`).
    ///
    /// # Errors
    ///
    /// Storage errors or malformed links.
    pub fn envelope(&self) -> Result<Option<Bounds>> {
        super::nearest::root_envelope(self, self.root()?)
    }
}

fn exhausted() -> SqlError {
    SqlError::new(Code::Resources, "[DESIGN] the R-tree intersection join cannot allocate more memory")
}

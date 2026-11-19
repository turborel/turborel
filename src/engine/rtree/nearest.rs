//! \[DESIGN\] Best-first rectangle distance search. Branch priorities
//! are rounded downward; leaf priorities are rectangle distances. A
//! branch is expanded before an equally distant entry, so rows tied on
//! distance can be returned by row id. The frontier reserves its memory
//! before growing; a cancelled caller drops it without reading the rest.

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::Arc;
use std::sync::atomic::Ordering as AtomicOrder;

use super::{RTree, Visit, corrupt};
use crate::disk::pager::PageId;
use crate::error::{Code, Result, SqlError};
use crate::memory::{MemoryBudget, Reservation};
use crate::spatial::Bounds;
use crate::storage::RowId;

/// Avoid an allocation at each of the first few queued branches.
const INITIAL_CAPACITY: usize = 64;
/// Geometric growth bounds allocation work as the frontier expands.
const GROWTH: usize = 2;

/// Rectangle entries in increasing Euclidean distance, then row-id
/// order. Distances are exact for point and rectangle entries; a geometry
/// represented by a rectangle still needs an exact distance calculation.
/// Entries held throughout appear once. Concurrently changed entries may
/// be omitted, including a new entry nearer than one already returned.
pub struct Nearest {
    tree: RTree,
    query: Bounds,
    metric: Metric,
    queue: BinaryHeap<Queued>,
    held: Reservation,
    failed: bool,
    visited: u64,
    limit: u64,
    /// An insertion may enlarge a branch whose old bound is queued. Its
    /// new entry is optional to this scan; omit it if it would go back
    /// before an entry already returned. Entries held throughout remain
    /// ordered by their admissible branch bounds.
    last: Option<(f64, RowId)>,
}

#[derive(Clone, Copy)]
enum Metric {
    Euclidean,
    LowerBound,
    /// Farthest first: each priority is the negated upper bound of the
    /// distance, so the least is popped first as with the others (D135).
    UpperBound,
}

impl Metric {
    fn distance(self, entry: Bounds, query: Bounds, leaf: bool) -> f64 {
        match self {
            Self::Euclidean => {
                if leaf {
                    entry.distance(query)
                } else {
                    entry.distance_lower_bound(query)
                }
            }
            Self::LowerBound => entry.distance_lower_bound(query),
            Self::UpperBound => -entry.distance_upper_bound(query),
        }
    }

    /// The root's priority, before every other.
    fn first(self) -> f64 {
        match self {
            Self::Euclidean | Self::LowerBound => 0.0_f64,
            Self::UpperBound => f64::NEG_INFINITY,
        }
    }
}

#[derive(Clone, Copy)]
enum Item {
    Page(Visit),
    Entry(Bounds, RowId),
}

#[derive(Clone, Copy)]
struct Queued {
    distance: f64,
    item: Item,
}

impl Queued {
    fn tie(self) -> (u8, u64) {
        match self.item {
            Item::Page(visit) => (0, visit.id.0),
            Item::Entry(_, row) => (1, row),
        }
    }
}

impl Ord for Queued {
    fn cmp(&self, other: &Self) -> Ordering {
        other.distance.total_cmp(&self.distance).then_with(|| other.tie().cmp(&self.tie()))
    }
}

impl PartialOrd for Queued {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Queued {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Queued {}

impl Nearest {
    pub(super) fn new(tree: RTree, query: Bounds, budget: &Arc<MemoryBudget>) -> Result<Nearest> {
        let root = tree.root()?;
        Self::from_root(tree, root, query, budget, Metric::Euclidean)
    }

    pub(super) fn candidates(tree: RTree, query: Bounds, domain: Bounds, budget: &Arc<MemoryBudget>) -> Result<Option<Nearest>> {
        let root = tree.root()?;
        Self::within(tree, root, (query, domain), budget)
    }

    /// Entries in nonincreasing order of their distance's upper bound,
    /// each yielded negated, if the domain certifies them as `candidates`.
    pub(super) fn farthest(tree: RTree, query: Bounds, domain: Bounds, budget: &Arc<MemoryBudget>) -> Result<Option<Nearest>> {
        let root = tree.root()?;
        Self::certified(tree, root, (query, domain), (budget, Metric::UpperBound))
    }

    pub(super) fn within(
        tree: RTree,
        root: Visit,
        (query, domain): (Bounds, Bounds),
        budget: &Arc<MemoryBudget>,
    ) -> Result<Option<Nearest>> {
        Self::certified(tree, root, (query, domain), (budget, Metric::LowerBound))
    }

    fn certified(
        tree: RTree,
        root: Visit,
        (query, domain): (Bounds, Bounds),
        (budget, metric): (&Arc<MemoryBudget>, Metric),
    ) -> Result<Option<Nearest>> {
        if !domain.contains(query) {
            return Ok(None);
        }
        let envelope = root_envelope(&tree, root)?;
        if envelope.is_some_and(|bounds| !domain.contains(bounds)) {
            return Ok(None);
        }
        Self::from_root(tree, root, query, budget, metric).map(Some)
    }

    fn from_root(tree: RTree, root: Visit, query: Bounds, budget: &Arc<MemoryBudget>, metric: Metric) -> Result<Nearest> {
        let held = budget.reserve(0, "the R-tree nearest-neighbour frontier")?;
        let mut nearest = Nearest { tree, query, metric, queue: BinaryHeap::new(), held, failed: false, visited: 0, limit: 0, last: None };
        nearest.push(Queued { distance: metric.first(), item: Item::Page(root) })?;
        Ok(nearest)
    }

    fn push(&mut self, entry: Queued) -> Result<()> {
        if self.queue.len() == self.queue.capacity() {
            let capacity = self.queue.capacity().checked_mul(GROWTH).ok_or_else(exhausted)?.max(INITIAL_CAPACITY);
            let bytes = capacity.checked_mul(size_of::<Queued>()).ok_or_else(exhausted)?;
            self.held.resize(bytes)?;
            self.queue.try_reserve_exact(capacity.saturating_sub(self.queue.len())).map_err(|_| exhausted())?;
        }
        self.queue.push(entry);
        Ok(())
    }

    fn expand(&mut self, visit: Visit, distance: f64) -> Result<()> {
        self.visited = self.visited.saturating_add(1);
        if self.visited > self.limit {
            self.limit = self.tree.inner.pool.pages_in_use(self.tree.inner.file)?;
            if self.visited > self.limit {
                return Err(corrupt("its nearest-neighbour traversal contains a cycle"));
            }
        }
        let node = self.tree.read(visit.id)?;
        if node.level != visit.level {
            return Err(corrupt("a nearest-neighbour child is at the wrong level"));
        }
        if node.stamp != visit.stamp {
            let id = node.right.ok_or_else(|| corrupt("a nearest-neighbour split has no sibling"))?;
            self.push(Queued { distance, item: Item::Page(Visit { id, ..visit }) })?;
            self.tree.inner.right_moves.fetch_add(1, AtomicOrder::Relaxed);
        }
        for entry in node.entries {
            let priority = self.metric.distance(entry.bounds, self.query, node.level == 0);
            let item = if node.level == 0 {
                Item::Entry(entry.bounds, entry.id)
            } else {
                let child = Visit { id: PageId(entry.id), stamp: entry.value, level: node.level.saturating_sub(1) };
                Item::Page(child)
            };
            self.push(Queued { distance: priority, item })?;
        }
        Ok(())
    }
}

/// Ancestors enlarge before insertion and commit publication. A snapshot
/// established before this read therefore has every retained visible
/// entry inside this envelope, including a root split not yet posted.
pub(super) fn root_envelope(tree: &RTree, mut visit: Visit) -> Result<Option<Bounds>> {
    let mut envelope = None;
    let mut visited = 0_u64;
    let mut limit = 0_u64;
    loop {
        visited = visited.saturating_add(1);
        if visited > limit {
            limit = tree.inner.pool.pages_in_use(tree.inner.file)?;
            if visited > limit {
                return Err(corrupt("its root envelope traversal contains a cycle"));
            }
        }
        let node = tree.read(visit.id)?;
        if node.level != visit.level {
            return Err(corrupt("a root envelope sibling is at the wrong level"));
        }
        if let Some(bounds) = node.bounds() {
            envelope = Some(envelope.map_or(bounds, |was: Bounds| was.union(bounds)));
        }
        if node.stamp == visit.stamp {
            return Ok(envelope);
        }
        visit.id = node.right.ok_or_else(|| corrupt("a root envelope split has no sibling"))?;
    }
}

impl Iterator for Nearest {
    type Item = Result<(f64, Bounds, RowId)>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed {
            return None;
        }
        while let Some(queued) = self.queue.pop() {
            match queued.item {
                Item::Entry(bounds, row) => {
                    let earlier =
                        self.last.is_some_and(|(distance, id)| queued.distance.total_cmp(&distance).then(row.cmp(&id)) == Ordering::Less);
                    if !earlier {
                        self.last = Some((queued.distance, row));
                        return Some(Ok((queued.distance, bounds, row)));
                    }
                }
                Item::Page(visit) => {
                    if let Err(error) = self.expand(visit, queued.distance) {
                        self.failed = true;
                        self.queue.clear();
                        return Some(Err(error));
                    }
                }
            }
        }
        None
    }
}

fn exhausted() -> SqlError {
    SqlError::new(Code::Resources, "[DESIGN] the R-tree nearest-neighbour frontier cannot allocate more memory")
}

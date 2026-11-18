//! \[DESIGN\] The hot-node cache: "caching all vertices that are C = 3 or
//! 4 hops from the starting point s" [DiskANN, NeurIPS 2019 §3.4], capped
//! at a share of the nodes. Every search passes through them, so their
//! reads are taken off every query. It is made when the index opens or is
//! loaded, and again by maintenance each time its live nodes double, so
//! an index grown by inserts caches its first hops too. The copies are
//! the index's own, reserved of the database's budget (D38), so the pool's eviction cannot
//! defeat them; if the budget refuses, the index caches nothing.
//!
//! A write of a cached node updates its copy after its page; a search
//! that reads the copy between them sees the node as it was, as any read
//! racing a write may.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::Ordering;

use crate::error::Result;
use crate::memory::Reservation;
use crate::sync::{read, write};

use super::DiskAnn;
use super::meta::NO_NODE;
use super::node::{Node, corrupt};

/// What the cache is reserved as.
const RESERVED_AS: &str = "the hot-node cache of a DiskANN index";
/// A percentage's whole.
const PERCENT: u64 = 100;
/// The cache is made again once the live nodes are twice those it was
/// made at, so an index grown by inserts makes it a logarithmic number of
/// times.
const GROWTH: u64 = 2;

/// Copies of the records of the nodes near the medoid.
#[derive(Default)]
pub(super) struct Cache {
    records: HashMap<u32, Box<[u8]>>,
    _reservation: Option<Reservation>,
}

impl Cache {
    /// Nodes cached.
    pub(super) fn len(&self) -> usize {
        self.records.len()
    }

    /// Node `id`'s record, if cached.
    pub(super) fn get(&self, id: u32) -> Option<&[u8]> {
        self.records.get(&id).map(AsRef::as_ref)
    }
}

impl DiskAnn {
    /// Makes the cache again: breadth first from the medoid, C hops, at
    /// most the tuning's share of the nodes.
    ///
    /// # Errors
    ///
    /// The buffer pool's.
    pub(super) fn refresh_cache(&self) -> Result<()> {
        let tuning = self.tuning()?;
        let medoid = self.inner.medoid.load(Ordering::Acquire);
        self.inner.cached_at.store(self.inner.live.load(Ordering::Acquire), Ordering::Release);
        let nodes = self.inner.live.load(Ordering::Acquire).saturating_add(self.inner.dead.load(Ordering::Acquire));
        let most = usize::try_from(nodes.saturating_mul(u64::from(tuning.cache_percent)).checked_div(PERCENT).unwrap_or(0)).unwrap_or(0);
        if medoid == NO_NODE || tuning.cache_hops == 0 || most == 0 {
            *write(&self.inner.cache)? = Cache::default();
            return Ok(());
        }
        let Ok(reservation) = self.inner.budget.reserve(most.saturating_mul(self.inner.shape.record), RESERVED_AS) else {
            *write(&self.inner.cache)? = Cache::default();
            return Ok(());
        };
        let records = self.breadth_first(medoid, tuning.cache_hops, most)?;
        *write(&self.inner.cache)? = Cache { records, _reservation: Some(reservation) };
        Ok(())
    }

    /// Whether the cache is due to be made again, its live nodes doubled
    /// since it was made and enough for the tuning's share of them to be
    /// at least one node.
    pub(super) fn cache_due(&self) -> bool {
        let Ok(tuning) = self.tuning() else { return false };
        let live = self.inner.live.load(Ordering::Acquire);
        let share = live.saturating_mul(u64::from(tuning.cache_percent)).checked_div(PERCENT).unwrap_or(0);
        let grown = live >= self.inner.cached_at.load(Ordering::Acquire).saturating_mul(GROWTH);
        tuning.cache_hops > 0 && share > 0 && grown
    }

    fn breadth_first(&self, medoid: u32, hops: u8, most: usize) -> Result<HashMap<u32, Box<[u8]>>> {
        let mut records = HashMap::new();
        let mut queue = VecDeque::from([(medoid, 0_u8)]);
        let mut node = Node::default();
        while let Some((id, depth)) = queue.pop_front() {
            if records.len() >= most {
                break;
            }
            if records.contains_key(&id) {
                continue;
            }
            let record = self.record(id)?;
            self.inner.shape.decode(&record, &mut node)?;
            records.insert(id, record);
            if depth < hops && !node.free() {
                queue.extend(node.neighbours.iter().map(|&n| (n, depth.saturating_add(1))));
            }
        }
        Ok(records)
    }

    /// Node `id`'s record bytes, from its page.
    pub(super) fn record(&self, id: u32) -> Result<Box<[u8]>> {
        let (page, offset) = self.locate(id)?;
        let content = self.inner.pool.read(self.inner.file, page)?;
        let end = offset.saturating_add(self.inner.shape.record);
        Ok(content.get(offset..end).ok_or_else(|| corrupt("a node is past its page"))?.into())
    }

    /// Brings node `id`'s cached copy, if any, up to `node`.
    pub(super) fn recache(&self, id: u32, node: &Node) -> Result<()> {
        if read(&self.inner.cache)?.get(id).is_none() {
            return Ok(());
        }
        let mut cache = write(&self.inner.cache)?;
        if let Some(record) = cache.records.get_mut(&id) {
            self.inner.shape.encode(node, record)?;
        }
        Ok(())
    }
}

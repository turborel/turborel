//! \[EXTENSION\] A local partitioned index: one DiskANN graph for each
//! value of a partition key, held under one index, beyond the paper. A
//! B-link tree maps each partition key's bytes to its graph's meta page;
//! a search asks the partitions named, or all, and merges their results
//! by distance. An unpartitioned index is the one partition of the empty
//! key.
//!
//! A load buffers each partition's entries while they fit the build
//! memory, then builds each partition whole; entries past it are inserted
//! one by one, as FreshDiskANN does.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::pager::PageId;
use crate::engine::tree::{BLinkTree, Direction};
use crate::error::Result;
use crate::memory::MemoryBudget;
use crate::sync::{lock_builtin, read, write};

use super::node::{corrupt, u64_at};
use super::{Candidate, DiskAnn, Layout, Plugins, Search, SearchStats, Tuning};

/// Bytes an entry costs a buffered load beside its vector.
const ENTRY_OVERHEAD: usize = 48;

/// The graphs of a partitioned index. Clones share them.
#[derive(Clone)]
pub struct Partitions {
    inner: Arc<Shared>,
}

struct Shared {
    pool: Arc<BufferPool>,
    file: FileId,
    map: BLinkTree,
    layout: Layout,
    tuning: RwLock<Tuning>,
    plugins: Plugins,
    budget: Arc<MemoryBudget>,
    graphs: RwLock<HashMap<Vec<u8>, DiskAnn>>,
    /// Serializes making a partition's graph.
    making: Mutex<()>,
}

/// A result, with the partition it came from.
pub type Partitioned = (Vec<u8>, Candidate);

impl Partitions {
    /// A new index with no partitions yet.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 if a node does not fit a page; the buffer pool's.
    pub fn create(
        pool: Arc<BufferPool>,
        file: FileId,
        layout: Layout,
        (tuning, plugins): (Tuning, Plugins),
        budget: Arc<MemoryBudget>,
    ) -> Result<Partitions> {
        super::node::Shape::new(layout.dims, layout.degree, layout.element, pool.capacity(file)?)?;
        let map = BLinkTree::create(Arc::clone(&pool), file)?;
        Ok(Self::with((pool, file, map), layout, (tuning, plugins), budget, HashMap::new()))
    }

    /// The index whose map's meta page is `meta`, every partition opened.
    ///
    /// # Errors
    ///
    /// As `DiskAnn::open`; SQLCODE -916 if a partition's layout differs.
    pub fn open(
        pool: Arc<BufferPool>,
        file: FileId,
        (meta, layout): (PageId, Layout),
        (tuning, plugins): (Tuning, Plugins),
        budget: Arc<MemoryBudget>,
    ) -> Result<Partitions> {
        let map = BLinkTree::open(Arc::clone(&pool), file, meta)?;
        let mut graphs = HashMap::new();
        for entry in map.scan(std::ops::Bound::Unbounded, std::ops::Bound::Unbounded, Direction::Forward) {
            let (key, value) = entry?;
            let graph = DiskAnn::open(Arc::clone(&pool), file, PageId(u64_at(&value, 0)), (tuning, plugins.clone()), Arc::clone(&budget))?;
            if graph.layout() != layout {
                return Err(corrupt("a partition's layout differs from its index's"));
            }
            graphs.insert(key, graph);
        }
        Ok(Self::with((pool, file, map), layout, (tuning, plugins), budget, graphs))
    }

    fn with(
        (pool, file, map): (Arc<BufferPool>, FileId, BLinkTree),
        layout: Layout,
        (tuning, plugins): (Tuning, Plugins),
        budget: Arc<MemoryBudget>,
        graphs: HashMap<Vec<u8>, DiskAnn>,
    ) -> Partitions {
        Partitions {
            inner: Arc::new(Shared {
                pool,
                file,
                map,
                layout,
                tuning: RwLock::new(tuning),
                plugins,
                budget,
                graphs: RwLock::new(graphs),
                making: Mutex::new(()),
            }),
        }
    }

    /// The page that reopens it.
    pub fn meta(&self) -> PageId {
        self.inner.map.meta()
    }

    /// Its layout.
    pub fn layout(&self) -> Layout {
        self.inner.layout
    }

    /// Partition `key`'s graph, if it has one.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if its lock is poisoned.
    pub fn get(&self, key: &[u8]) -> Result<Option<DiskAnn>> {
        Ok(read(&self.inner.graphs)?.get(key).cloned())
    }

    /// Every partition's key and graph.
    ///
    /// # Errors
    ///
    /// As `get`.
    pub fn all(&self) -> Result<Vec<(Vec<u8>, DiskAnn)>> {
        Ok(read(&self.inner.graphs)?.iter().map(|(k, g)| (k.clone(), g.clone())).collect())
    }

    /// Partition `key`'s graph, made if it has none.
    ///
    /// # Errors
    ///
    /// The buffer pool's.
    pub fn partition(&self, key: &[u8]) -> Result<DiskAnn> {
        if let Some(graph) = self.get(key)? {
            return Ok(graph);
        }
        let _making = lock_builtin(&self.inner.making);
        if let Some(graph) = self.get(key)? {
            return Ok(graph);
        }
        let i = &self.inner;
        let tuning = *read(&i.tuning)?;
        let graph = DiskAnn::create(Arc::clone(&i.pool), i.file, i.layout, (tuning, i.plugins.clone()), Arc::clone(&i.budget))?;
        if let Err(error) = i.map.insert(key, &graph.meta().0.to_le_bytes()) {
            // The graph was never published, so its pages are freed, or
            // leaked if that fails too; the first error is the one told.
            let _ = graph.free();
            return Err(error);
        }
        write(&i.graphs)?.insert(key.to_vec(), graph.clone());
        Ok(graph)
    }

    /// Adds one reference to (`vector`, `row`) in partition `key`.
    ///
    /// # Errors
    ///
    /// As `DiskAnn::insert`.
    pub fn insert(&self, key: &[u8], vector: &[f32], row: u64) -> Result<()> {
        self.partition(key)?.insert(vector, row)
    }

    /// Takes back one reference to (`vector`, `row`) in partition `key`.
    ///
    /// # Errors
    ///
    /// As `DiskAnn::remove`.
    pub fn remove(&self, key: &[u8], vector: &[f32], row: u64) -> Result<()> {
        self.get(key)?.map_or(Ok(()), |graph| graph.remove(vector, row))
    }

    /// The rows of partition `key` with a live entry of `vector`.
    ///
    /// # Errors
    ///
    /// As `DiskAnn::rows`.
    pub fn rows(&self, key: &[u8], vector: &[f32]) -> Result<Vec<u64>> {
        self.get(key)?.map_or(Ok(Vec::new()), |graph| graph.rows(vector))
    }

    /// Searches the partitions of `keys`, or all, and merges their
    /// results nearest first: the k nearest overall, or all within the
    /// radius. The stats add up the partitions'.
    ///
    /// # Errors
    ///
    /// As `DiskAnn::search`.
    pub fn search(&self, query: &[f32], search: &Search<'_>, keys: Option<&[Vec<u8>]>) -> Result<(Vec<Partitioned>, SearchStats)> {
        let graphs: Vec<(Vec<u8>, DiskAnn)> = match keys {
            Some(keys) => keys.iter().filter_map(|k| self.get(k).transpose().map(|g| g.map(|g| (k.clone(), g)))).collect::<Result<_>>()?,
            None => self.all()?,
        };
        let mut found = Vec::new();
        let mut stats = SearchStats::default();
        for (key, graph) in graphs {
            let (each, s) = graph.search(query, search)?;
            stats = add(stats, s);
            found.extend(each.into_iter().map(|c| (key.clone(), c)));
        }
        found.sort_by(|a, b| a.1.distance.total_cmp(&b.1.distance).then(a.0.cmp(&b.0)).then(a.1.node.cmp(&b.1.node)));
        if search.radius.is_none() {
            found.truncate(search.results.max(1));
        }
        Ok((found, stats))
    }

    /// Builds the index, empty, from (partition key, vector, row id)
    /// entries: each partition whole while the entries fit the build
    /// memory, the rest inserted.
    ///
    /// # Errors
    ///
    /// As `DiskAnn::load` and `insert`.
    pub fn load(&self, entries: &mut dyn Iterator<Item = Result<(Vec<u8>, Vec<f32>, u64)>>) -> Result<()> {
        let memory = read(&self.inner.tuning)?.build_memory;
        let mut held: HashMap<Vec<u8>, Vec<(Vec<f32>, u64)>> = HashMap::new();
        let mut bytes = 0_usize;
        let mut reservation = self.inner.budget.reserve(0, "the load of a partitioned DiskANN index")?;
        for entry in entries {
            let (key, vector, row) = entry?;
            bytes = bytes.saturating_add(vector.len().saturating_mul(size_of::<f32>())).saturating_add(ENTRY_OVERHEAD);
            if bytes > memory || reservation.resize(bytes).is_err() {
                self.insert(&key, &vector, row)?;
                continue;
            }
            held.entry(key).or_default().push((vector, row));
        }
        let mut keys: Vec<Vec<u8>> = held.keys().cloned().collect();
        keys.sort_unstable();
        for key in keys {
            let list = held.remove(&key).unwrap_or_default();
            let graph = self.partition(&key)?;
            if graph.counts().allocated == 0 {
                graph.load(&mut list.into_iter().map(Ok))?;
            } else {
                list.into_iter().try_for_each(|(v, r)| graph.insert(&v, r))?;
            }
        }
        Ok(())
    }

    /// Changes every partition's tuning in place.
    ///
    /// # Errors
    ///
    /// As `DiskAnn::retune`.
    pub fn retune(&self, tuning: Tuning) -> Result<()> {
        *write(&self.inner.tuning)? = tuning;
        self.all()?.into_iter().try_for_each(|(_, g)| g.retune(tuning))
    }

    /// Whether any partition has maintenance due.
    pub fn maintenance_due(&self) -> bool {
        self.all().is_ok_and(|all| all.iter().any(|(_, g)| g.maintenance_due()))
    }

    /// Maintains each partition; whether any did work.
    ///
    /// # Errors
    ///
    /// As `DiskAnn::maintain`.
    pub fn maintain(&self) -> Result<bool> {
        let mut did = false;
        for (_, graph) in self.all()? {
            did |= graph.maintain()?;
        }
        Ok(did)
    }

    /// Writes every partition's meta page.
    ///
    /// # Errors
    ///
    /// The buffer pool's.
    pub fn flush(&self) -> Result<()> {
        self.all()?.into_iter().try_for_each(|(_, g)| g.flush())
    }

    /// Frees every page of a discarded index.
    ///
    /// # Errors
    ///
    /// The buffer pool's; some pages may already be freed.
    pub fn free(&self) -> Result<()> {
        self.all()?.into_iter().try_for_each(|(_, g)| g.free())?;
        self.inner.map.free()
    }

    /// Live nodes over every partition.
    pub fn live(&self) -> u64 {
        self.all().map_or(0, |all| all.iter().map(|(_, g)| g.counts().live).sum())
    }
}

fn add(a: SearchStats, b: SearchStats) -> SearchStats {
    SearchStats {
        hops: a.hops.saturating_add(b.hops),
        expanded: a.expanded.saturating_add(b.expanded),
        reads: a.reads.saturating_add(b.reads),
        cached: a.cached.saturating_add(b.cached),
        rounds: a.rounds.saturating_add(b.rounds),
        list: a.list.max(b.list),
    }
}

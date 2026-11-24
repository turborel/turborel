//! \[EXTENSION\] DiskANN, after Subramanya, Devvrit, Kadekodi,
//! Krishnaswamy and Simhadri, "DiskANN: Fast Accurate Billion-point
//! Nearest Neighbor Search on a Single Node", NeurIPS 2019: a Vamana graph
//! of bounded degree whose nodes, each its full-precision vector followed
//! by its edges, sit in pages of the shared pager file [§3.2], searched by
//! BeamSearch [§3.3] guided by compressed codes held in memory [§3.1] and
//! re-ranked by the full vectors its reads bring [§3.5], with the nodes a
//! few hops from the medoid cached [§3.4]. Built by two Vamana passes
//! [§2.3, Algorithm 3], or, past its build memory, by the merged build of
//! overlapping k-means clusters [§3.1, §4.3].
//!
//! Beyond the paper, which builds once and never changes its graph, are
//! the FreshDiskANN updates (Singh, Subramanya, Krishnaswamy and Simhadri,
//! "FreshDiskANN", arXiv:2105.09613, 2021), insertion by search, prune
//! and back edges, deletion by tombstone, and consolidation; filtered
//! search, range search and partitioned graphs.
//!
//! The engine is generic. Distances come through `Metric` and compressed
//! codes through `Quantizer` and `Codes`, so it holds no metric of its
//! own. Rows are counted as every index's are, with one node per (vector,
//! row id) and its count the times it was inserted more than removed.
//!
//! Durability is the data file's shadowed checkpoint, as the R-tree's.
//! Nodes are written in place, never logged. Each change is ordered so a
//! failure leaves no logical change (D19), as `fresh.rs` tells.

mod beam;
mod cache;
mod chunks;
mod codes;
mod consolidate;
mod fresh;
mod frontier;
mod half;
mod kmeans;
mod load;
mod merged;
mod meta;
mod node;
mod partitioned;
mod prune;
mod random;
mod rowmap;
mod store;
mod vamana;

use std::collections::HashSet;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::pager::PageId;
use crate::engine::locks::Locks;
use crate::engine::tree::BLinkTree;
use crate::error::{Code, Result, SqlError};
use crate::memory::MemoryBudget;
use crate::sync::{lock_builtin, read, write};

pub use beam::{Candidate, Search, SearchStats};
use cache::Cache;
use chunks::{Chunks, NO_PAGE};
use codes::Book;
use frontier::Visited;
use meta::{Meta, NO_NODE};
pub use node::{Element, MOST_DEGREE, MOST_DIMENSIONS};
use node::{Shape, corrupt};
pub use partitioned::Partitions;

/// \[EXTENSION\] A distance between two vectors of the index's dimensions.
///
/// Contract: deterministic bit for bit; finite (a NaN or infinity is
/// refused with SQLCODE -910); `distance(a, a)` the least; never panics,
/// never calls back into the database. A graph searches well for a metric
/// that is symmetric with the triangle inequality; any other ordering is
/// searched as a heuristic.
pub trait Metric: Send + Sync {
    /// The distance from `a` to `b`, smaller nearer.
    fn distance(&self, a: &[f32], b: &[f32]) -> f32;
    /// The distance from `a` to `b`, each given with its squared norm
    /// (`kernels::squared_norm`), which a node record keeps and a search
    /// works out once for its query: `distance`'s bit for bit, by
    /// default `distance` itself, the norms unused.
    fn normed(&self, a: Probe<'_>, b: Probe<'_>) -> f32 {
        self.distance(a.0, b.0)
    }
    /// The distances from `a` to each of `many`, in order: `distance`'s,
    /// in one call, as a plug-in's metric measures a node's neighbours in
    /// one round trip. Default: `distance` of each.
    fn distances(&self, a: &[f32], many: &[&[f32]]) -> Vec<f32> {
        many.iter().map(|b| self.distance(a, b)).collect()
    }
    /// Whether `distances` is cheaper than `distance` of each, so a
    /// search gathers a node's neighbours first. Default: no.
    fn batches(&self) -> bool {
        false
    }
}

/// A vector and its squared norm.
pub type Probe<'a> = (&'a [f32], f32);

impl<F: Fn(&[f32], &[f32]) -> f32 + Send + Sync> Metric for F {
    fn distance(&self, a: &[f32], b: &[f32]) -> f32 {
        self(a, b)
    }
}

/// \[EXTENSION\] A trained compression of vectors into short codes, whose
/// approximate distances guide a search [DiskANN, NeurIPS 2019 §3.1].
///
/// Contract: `encode` writes exactly `code_bytes`; distances are finite
/// and approximate the metric's; `write` gives bytes the quantizer's
/// `read` turns back into an equal codebook; nothing panics or calls back.
pub trait Codes: Send + Sync {
    /// Bytes of one code.
    fn code_bytes(&self) -> usize;
    /// `vector`'s code, into `out` of `code_bytes` bytes.
    fn encode(&self, vector: &[f32], out: &mut [u8]);
    /// The measure of one query's code distances (asymmetric distance
    /// computation: one table per query, made once, a sum of lookups a
    /// code). It borrows the codes, so a codebook of any kind, a plug-in
    /// quantizer's among them, tabulates as it likes.
    fn table<'a>(&'a self, query: &[f32]) -> Box<dyn CodeTable + 'a>;
    /// The codebook's bytes, kept in the index.
    fn write(&self) -> Vec<u8>;
}

/// \[EXTENSION\] The approximate distances from one query to codes, as
/// `Codes::table` made them.
pub trait CodeTable {
    /// The approximate distance from the query to `code`.
    fn distance(&self, code: &[u8]) -> f32;
}

/// \[EXTENSION\] What trains and reads `Codes`.
///
/// Contract: training is deterministic for one sample and seed; it reads
/// no clock and no other randomness.
pub trait Quantizer: Send + Sync {
    /// A codebook trained on `sample`, vectors of `dims` coordinates one
    /// after another, from `seed`.
    ///
    /// # Errors
    ///
    /// The sample cannot train one.
    fn train(&self, sample: &[f32], dims: usize, seed: u64) -> Result<Arc<dyn Codes>>;
    /// The codebook `Codes::write` gave.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if the bytes are not one.
    fn read(&self, bytes: &[u8]) -> Result<Arc<dyn Codes>>;
}

/// The plug-ins an index computes with.
#[derive(Clone)]
pub struct Plugins {
    /// The distance it orders by.
    pub metric: Arc<dyn Metric>,
    /// What compresses its guiding vectors; none to guide by full vectors.
    pub quantizer: Option<Arc<dyn Quantizer>>,
}

/// What an index's records are fixed to when it is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    /// Coordinates of a vector.
    pub dims: u32,
    /// R, the most edges of a node [DiskANN, NeurIPS 2019 Algorithm 2].
    pub degree: u32,
    /// How node vectors are stored.
    pub element: Element,
}

/// What may change while an index is open (DISKANN.* options of
/// `Rebuild::No`). Defaults are the paper's, or the design's where the
/// paper leaves them open.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tuning {
    /// L of builds and inserts [DiskANN, NeurIPS 2019 Algorithm 3].
    pub build_list: u32,
    /// α ≥ 1 of RobustPrune [Algorithm 2].
    pub alpha: f64,
    /// 2 for α = 1 then α [§2.3]; 1 for α alone.
    pub passes: u8,
    /// L of searches, at least the results asked.
    pub search_list: u32,
    /// W of BeamSearch, 2 to 8 [§3.3].
    pub beam_width: u32,
    /// C, the hops from the medoid whose nodes are cached [§3.4].
    pub cache_hops: u8,
    /// The most nodes cached, a percentage of the nodes.
    pub cache_percent: u32,
    /// k of the merged build's k-means [§3.1].
    pub clusters: u32,
    /// ℓ, the clusters each point is sent to [§3.1].
    pub copies: u32,
    /// Dead nodes, a percentage of the nodes, past which consolidation is due.
    pub consolidate_percent: u32,
    /// Nodes an index built by inserts holds before its codes are trained.
    pub train_at: u32,
    /// Bytes a build may hold in memory; past them it builds merged.
    pub build_memory: usize,
}

/// \[DESIGN\] Defaults: L = 100, α = 1.2 [§4.1, §4.3], W = 4, C = 3 [§3.4],
/// k = 40 and ℓ = 2 [§3.1].
pub mod defaults {
    /// L of builds.
    pub const BUILD_LIST: u32 = 100;
    /// α [DiskANN, NeurIPS 2019 §4.1].
    pub const ALPHA: f64 = 1.2;
    /// Two passes [§2.3].
    pub const PASSES: u8 = 2;
    /// L of searches.
    pub const SEARCH_LIST: u32 = 100;
    /// W [§3.3].
    pub const BEAM_WIDTH: u32 = 4;
    /// C [§3.4].
    pub const CACHE_HOPS: u8 = 3;
    /// One percent of the nodes cached.
    pub const CACHE_PERCENT: u32 = 1;
    /// k [§3.1].
    pub const CLUSTERS: u32 = 40;
    /// ℓ [§3.1].
    pub const COPIES: u32 = 2;
    /// FreshDiskANN consolidates once a fifth of the nodes are dead.
    pub const CONSOLIDATE_PERCENT: u32 = 20;
    /// Nodes before an index built by inserts trains its codes: enough
    /// that 256 centroids of each subspace see several points each.
    pub const TRAIN_AT: u32 = 4096;
    /// Build memory, 256 MiB.
    pub const BUILD_MEMORY: usize = 256 << 20;
    /// R [DiskANN, NeurIPS 2019 §4.3, merged shards].
    pub const DEGREE: u32 = 64;
}

impl Default for Tuning {
    fn default() -> Tuning {
        Tuning {
            build_list: defaults::BUILD_LIST,
            alpha: defaults::ALPHA,
            passes: defaults::PASSES,
            search_list: defaults::SEARCH_LIST,
            beam_width: defaults::BEAM_WIDTH,
            cache_hops: defaults::CACHE_HOPS,
            cache_percent: defaults::CACHE_PERCENT,
            clusters: defaults::CLUSTERS,
            copies: defaults::COPIES,
            consolidate_percent: defaults::CONSOLIDATE_PERCENT,
            train_at: defaults::TRAIN_AT,
            build_memory: defaults::BUILD_MEMORY,
        }
    }
}

/// \[DESIGN\] A seed of builds when none is given: the meta page's number
/// mixed in, so two indexes differ but each repeats.
const SEED_BASE: u64 = 0x5EED_D15C_A11D_0001;

/// A DiskANN graph in one pager file. Clones share it.
#[derive(Clone)]
pub struct DiskAnn {
    inner: Arc<Inner>,
}

struct Inner {
    pool: Arc<BufferPool>,
    file: FileId,
    meta: PageId,
    shape: Shape,
    seed: u64,
    plugins: Plugins,
    tuning: RwLock<Tuning>,
    budget: Arc<MemoryBudget>,
    rowmap: BLinkTree,
    nodes: RwLock<Chunks>,
    /// Allocation: the next node, the free list's head. Its lock also
    /// orders meta page writes.
    alloc: Mutex<Alloc>,
    medoid: AtomicU32,
    live: AtomicU64,
    dead: AtomicU64,
    book: RwLock<Book>,
    cache: RwLock<Cache>,
    /// Live nodes when the cache was last made, which is made again once
    /// they double, so an index grown by inserts caches its first hops.
    cached_at: AtomicU64,
    /// Per-node latches of writers (`engine/locks.rs`), by node number.
    latches: Locks,
    /// Nodes being inserted, which consolidation leaves alone.
    pending: Mutex<HashSet<u32>>,
    /// Per-query visited arrays, reused (generation stamped).
    visits: Mutex<Vec<Visited>>,
    hops: AtomicU64,
    searches: AtomicU64,
}

#[derive(Debug, Clone, Copy)]
struct Alloc {
    next: u32,
    free: u32,
}

/// Counts of an index, for statistics and tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    /// Nodes searches return.
    pub live: u64,
    /// Tombstoned nodes awaiting consolidation.
    pub dead: u64,
    /// Node numbers allocated, free slots included.
    pub allocated: u32,
    /// Nodes in the hot-node cache.
    pub cached: usize,
    /// Searches since it opened.
    pub searches: u64,
    /// Hops of those searches.
    pub hops: u64,
}

impl DiskAnn {
    /// Creates an empty index in pager file `file`.
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
    ) -> Result<DiskAnn> {
        if layout.dims > MOST_DIMENSIONS || layout.degree > MOST_DEGREE {
            return Err(SqlError::new(
                Code::Unsupported,
                "[EXTENSION] a DiskANN index takes at most 65535 dimensions and 512 edges a node",
            ));
        }
        let shape = Shape::new(layout.dims, layout.degree, layout.element, pool.capacity(file)?)?;
        let nodes = Chunks::create(&pool, file)?;
        let codes = Chunks::create(&pool, file)?;
        let rowmap = BLinkTree::create(Arc::clone(&pool), file)?;
        let (meta, page) = pool.new_page(file)?;
        drop(page);
        let state = Meta {
            dims: layout.dims,
            degree: layout.degree,
            element: layout.element,
            seed: SEED_BASE ^ meta.0,
            medoid: NO_NODE,
            next: 0,
            free: NO_NODE,
            live: 0,
            dead: 0,
            nodes: nodes.head(),
            codes: codes.head(),
            book: NO_PAGE,
            code_width: 0,
            rowmap: rowmap.meta(),
        };
        *pool.write(file, meta)? = state.bytes();
        let book = Book::empty(codes);
        Ok(Self::with((pool, file, meta), shape, state, (nodes_of(nodes), rowmap, book), (tuning, plugins, budget)))
    }

    /// Opens the index whose meta page is `meta`: its directories, its
    /// codes into memory reserved of `budget`, and its hot-node cache.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 for a malformed index; -918 if its codes cannot be
    /// reserved; the buffer pool's.
    pub fn open(
        pool: Arc<BufferPool>,
        file: FileId,
        meta: PageId,
        (tuning, plugins): (Tuning, Plugins),
        budget: Arc<MemoryBudget>,
    ) -> Result<DiskAnn> {
        let state = Meta::parse(&pool.read(file, meta.0)?)?;
        let shape = Shape::new(state.dims, state.degree, state.element, pool.capacity(file)?)
            .map_err(|_| corrupt("its records do not fit its pages"))?;
        let nodes = Chunks::open(&pool, file, state.nodes)?;
        let codes = Chunks::open(&pool, file, state.codes)?;
        let rowmap = BLinkTree::open(Arc::clone(&pool), file, state.rowmap)?;
        let book = Book::empty(codes);
        let index = Self::with((pool, file, meta), shape, state, (nodes_of(nodes), rowmap, book), (tuning, plugins, budget));
        index.open_codes(&state)?;
        index.refresh_cache()?;
        Ok(index)
    }

    fn with(
        (pool, file, meta): (Arc<BufferPool>, FileId, PageId),
        shape: Shape,
        state: Meta,
        (nodes, rowmap, book): (RwLock<Chunks>, BLinkTree, Book),
        (tuning, plugins, budget): (Tuning, Plugins, Arc<MemoryBudget>),
    ) -> DiskAnn {
        DiskAnn {
            inner: Arc::new(Inner {
                pool,
                file,
                meta,
                shape,
                seed: state.seed,
                plugins,
                tuning: RwLock::new(tuning),
                budget,
                rowmap,
                nodes,
                alloc: Mutex::new(Alloc { next: state.next, free: state.free }),
                medoid: AtomicU32::new(state.medoid),
                live: AtomicU64::new(state.live),
                dead: AtomicU64::new(state.dead),
                book: RwLock::new(book),
                cache: RwLock::new(Cache::default()),
                cached_at: AtomicU64::new(0),
                latches: Locks::new(),
                pending: Mutex::new(HashSet::new()),
                visits: Mutex::new(Vec::new()),
                hops: AtomicU64::new(0),
                searches: AtomicU64::new(0),
            }),
        }
    }

    /// The page that reopens it.
    pub fn meta(&self) -> PageId {
        self.inner.meta
    }

    /// Its layout.
    pub fn layout(&self) -> Layout {
        let s = &self.inner.shape;
        Layout { dims: u32::try_from(s.dims).unwrap_or(u32::MAX), degree: u32::try_from(s.degree).unwrap_or(u32::MAX), element: s.element }
    }

    /// Its tuning now.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if its lock is poisoned.
    pub fn tuning(&self) -> Result<Tuning> {
        Ok(*read(&self.inner.tuning)?)
    }

    /// Changes its tuning in place (options of `Rebuild::No`); the
    /// hot-node cache is made again for a changed C or share.
    ///
    /// # Errors
    ///
    /// As `tuning`; the buffer pool's while caching.
    pub fn retune(&self, tuning: Tuning) -> Result<()> {
        let before = std::mem::replace(&mut *write(&self.inner.tuning)?, tuning);
        if before.cache_hops != tuning.cache_hops || before.cache_percent != tuning.cache_percent {
            self.refresh_cache()?;
        }
        Ok(())
    }

    /// Its counts now.
    pub fn counts(&self) -> Counts {
        let i = &self.inner;
        Counts {
            live: i.live.load(Ordering::Acquire),
            dead: i.dead.load(Ordering::Acquire),
            allocated: lock_builtin(&i.alloc).next,
            cached: read(&i.cache).map_or(0, |c| c.len()),
            searches: i.searches.load(Ordering::Relaxed),
            hops: i.hops.load(Ordering::Relaxed),
        }
    }

    /// Writes the meta page from what is in memory. Called under the
    /// allocation lock, so meta writes are in order.
    fn write_meta(&self, alloc: Alloc) -> Result<()> {
        let i = &self.inner;
        let book = read(&i.book)?;
        let state = Meta {
            dims: u32::try_from(i.shape.dims).unwrap_or(u32::MAX),
            degree: u32::try_from(i.shape.degree).unwrap_or(u32::MAX),
            element: i.shape.element,
            seed: i.seed,
            medoid: i.medoid.load(Ordering::Acquire),
            next: alloc.next,
            free: alloc.free,
            live: i.live.load(Ordering::Acquire),
            dead: i.dead.load(Ordering::Acquire),
            nodes: read(&i.nodes)?.head(),
            codes: book.pages.head(),
            book: book.chain.first().map_or(NO_PAGE, |p| p.0),
            code_width: u32::try_from(book.width).unwrap_or(0),
            rowmap: i.rowmap.meta(),
        };
        drop(book);
        *i.pool.write(i.file, i.meta)? = state.bytes();
        Ok(())
    }

    /// Writes the meta page: what a checkpoint makes durable.
    ///
    /// # Errors
    ///
    /// The buffer pool's.
    pub fn flush(&self) -> Result<()> {
        let alloc = *lock_builtin(&self.inner.alloc);
        self.write_meta(alloc)
    }

    /// Frees every page of a discarded index. Nothing may use it again.
    ///
    /// # Errors
    ///
    /// The buffer pool's; some pages may already be freed.
    pub fn free(&self) -> Result<()> {
        let i = &self.inner;
        read(&i.nodes)?.free(&i.pool, i.file)?;
        read(&i.book)?.free(&i.pool, i.file)?;
        i.rowmap.free()?;
        i.pool.free(i.file, i.meta)
    }
}

fn nodes_of(chunks: Chunks) -> RwLock<Chunks> {
    RwLock::new(chunks)
}

/// SQLCODE -910 for a plug-in's distance that is not finite.
fn broken(what: &str) -> SqlError {
    SqlError::new(Code::Api, format!("[EXTENSION] the DiskANN {what} broke its contract: a distance is not finite"))
}

/// `d`, if finite.
fn finite(d: f32, what: &str) -> Result<f32> {
    if d.is_finite() { Ok(d) } else { Err(broken(what)) }
}

#[cfg(test)]
mod tests;

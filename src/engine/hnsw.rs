//! \[DESIGN\] HNSW, Hierarchical Navigable Small World graphs (Malkov and
//! Yashunin, "Efficient and robust approximate nearest neighbor search
//! using Hierarchical Navigable Small World graphs", IEEE TPAMI 2020): the
//! vector design document's INMEMORY NEIGHBOR GRAPH (§10.1, §10.4). The
//! graph lives in memory reserved of the database's budget (RULES.md D38,
//! the document's R10.7), and is kept in pages so it survives a reopen
//! without a rebuild (R14.6):
//!
//! * an image of the whole graph (`image`), written at a checkpoint the
//!   method takes and when maintenance finds the change log long;
//! * a change log (`log`): every insert and remove since the image,
//!   appended to pages before the graph changes, so the pages a database
//!   checkpoint captures always hold the image and a prefix of the log,
//!   which opening replays.
//!
//! The meta page, fixed for the index's life, names both. A new image is
//! written to new pages and the meta page last, so a checkpoint captured
//! meanwhile holds the old image or the new, never part of one.
//!
//! Writers take the graph exclusively, readers share it. Removes leave
//! tombstones, repaired and dropped by maintenance past 20% of the nodes
//! (R10.10). A quantizer, if named, is trained once enough rows are in,
//! and the nodes then keep codes, decoded as they are measured.

mod chain;
mod fence;
mod graph;
mod image;
mod insert;
mod keep;
mod log;
pub mod measure;
mod query;
pub mod random;
mod repair;
pub mod scratch;
mod search;
mod view;

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize};
use std::sync::{Arc, Mutex, RwLock};

pub use chain::{
    HEADER as CHAIN_HEADER, NO_PAGE, Reader as ChainReader, Tail, Writer as ChainWriter, corrupt, free_pages, put_floats, put_varint,
};
use graph::{Graph, Store};
pub use keep::{SAMPLE_MOST, sample};
pub use log::{MEMORY_STEP, grow};
pub use measure::{Codec, DistanceFn, Metric, Quantizer, SQ8, Sq8};
pub use query::{Exact, Filter, RERANK_FACTOR, Search, finish};
pub use view::TRIES;

use crate::bytes::{Put, Reader};
use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::pager::PageId;
use crate::error::{Code, Result, SqlError};
use crate::memory::{MemoryBudget, Reservation};

/// The meta page's tag, 'H'.
const META_TAG: u8 = 72;
/// The meta page's format.
const META_VERSION: u8 = 1;
/// \[DESIGN\] The least M: below 2 a node has one way out of each layer.
pub const MIN_M: usize = 2;
/// \[DESIGN\] The most M: past 512 a node's lists outgrow what search
/// gains from them, and M0 = 2M stays within a `u16` degree.
pub const MAX_M: usize = 512;
/// \[DESIGN\] The most dimensions an index holds: the vector type's limit.
pub const MAX_DIMS: usize = 65_535;

/// How an index is built and searched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// Each vector's dimensions.
    pub dims: usize,
    /// M: the most neighbours on an upper layer; layer 0 keeps M0 = 2M.
    pub m: usize,
    /// efConstruction.
    pub ef_construction: usize,
    /// efSearch, a query's unless it gives its own.
    pub ef_search: usize,
    /// extendCandidates.
    pub extend: bool,
    /// keepPrunedConnections.
    pub keep_pruned: bool,
    /// The seed of every random choice.
    pub seed: u64,
    /// The live rows a quantizer is trained on once there are as many.
    pub train_rows: usize,
    /// Whether a quantized index gives its candidates re-ranked.
    pub rerank: bool,
}

/// An HNSW index in a pager file. Clones share it.
#[derive(Clone)]
pub struct Hnsw {
    inner: Arc<Inner>,
}

/// The settings a change of an index's options makes in place.
struct Tuning {
    ef_construction: AtomicUsize,
    ef_search: AtomicUsize,
    extend: AtomicBool,
    keep_pruned: AtomicBool,
    rerank: AtomicBool,
}

struct Inner {
    pool: Arc<BufferPool>,
    file: FileId,
    meta: PageId,
    dims: usize,
    m: usize,
    seed: u64,
    train_rows: usize,
    metric: Metric,
    quantizer: Option<Arc<dyn Quantizer>>,
    tuning: Tuning,
    graph: RwLock<Graph>,
    /// The change log's end and its record count; taken after the graph.
    log: Mutex<(Tail, u64)>,
    /// The image's first page; taken after the log.
    image: Mutex<PageId>,
    scratch: scratch::Pool,
    /// The graph's reservation and the bytes of it used.
    memory: Mutex<(Reservation, usize)>,
    /// The budget it is reserved of.
    budget: Arc<MemoryBudget>,
    /// What writers wait at while maintenance fences them out (`fence.rs`).
    fence: fence::Fence,
    /// Maintenance's swaps lost in a row to writers.
    lost: AtomicU32,
}

impl Hnsw {
    /// An empty index in `file`, measuring by `metric`, quantized by
    /// `quantizer` once trained, its memory reserved of `budget`.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 for an M or dimension count out of range; the buffer
    /// pool's; -918 if the budget refuses.
    pub fn create(
        pool: Arc<BufferPool>,
        file: FileId,
        config: &Config,
        (metric, quantizer): (Metric, Option<Arc<dyn Quantizer>>),
        budget: &Arc<MemoryBudget>,
    ) -> Result<Hnsw> {
        validate(config)?;
        let memory = (budget.reserve(0, "an HNSW index's graph")?, 0);
        let graph = Graph::new(config.dims, (config.m, config.m.saturating_mul(2)), Store::Full(Vec::new()));
        let mut image = ChainWriter::new(&pool, file)?;
        if let Err(error) = image::write(&graph, &[], &mut image) {
            // Unpublished pages have no readers; a failed free leaks pages only.
            let _ = image.abandon();
            return Err(error);
        }
        let (head, mut pages) = image.finish()?;
        let made = Tail::create(&pool, file).and_then(|log| {
            pages.push(log.head);
            let (meta, mut page) = pool.new_page(file)?;
            *page = meta_bytes(config, (metric.name(), quantizer.as_ref().map(|q| q.name())), (head, log.head));
            Ok((meta, log))
        });
        let (meta, log) = match made {
            Ok(made) => made,
            Err(error) => {
                // As above: nothing refers to them yet.
                let _ = free_pages(&pool, file, &pages);
                return Err(error);
            }
        };
        Ok(Hnsw::with(Parts {
            pool,
            file,
            meta,
            config: *config,
            metric,
            quantizer,
            graph,
            log,
            image: head,
            memory,
            budget: Arc::clone(budget),
        }))
    }

    fn with(p: Parts) -> Hnsw {
        let c = p.config;
        Hnsw {
            inner: Arc::new(Inner {
                pool: p.pool,
                file: p.file,
                meta: p.meta,
                dims: c.dims,
                m: c.m,
                seed: c.seed,
                train_rows: c.train_rows,
                metric: p.metric,
                quantizer: p.quantizer,
                tuning: Tuning {
                    ef_construction: AtomicUsize::new(c.ef_construction.max(1)),
                    ef_search: AtomicUsize::new(c.ef_search.max(1)),
                    extend: AtomicBool::new(c.extend),
                    keep_pruned: AtomicBool::new(c.keep_pruned),
                    rerank: AtomicBool::new(c.rerank),
                },
                graph: RwLock::new(p.graph),
                log: Mutex::new((p.log, 0)),
                image: Mutex::new(p.image),
                scratch: scratch::Pool::default(),
                memory: Mutex::new(p.memory),
                budget: p.budget,
                fence: fence::Fence::default(),
                lost: AtomicU32::new(0),
            }),
        }
    }

    /// The page that finds the index again (`open`).
    pub fn meta(&self) -> PageId {
        self.inner.meta
    }

    /// Its metric's name.
    pub fn metric(&self) -> &str {
        self.inner.metric.name()
    }

    /// Each vector's dimensions.
    pub fn dims(&self) -> usize {
        self.inner.dims
    }
}

/// What an index is made of, created or opened.
struct Parts {
    pool: Arc<BufferPool>,
    file: FileId,
    meta: PageId,
    config: Config,
    metric: Metric,
    quantizer: Option<Arc<dyn Quantizer>>,
    graph: Graph,
    log: Tail,
    image: PageId,
    memory: (Reservation, usize),
    budget: Arc<MemoryBudget>,
}

/// Refuses a configuration out of range.
fn validate(config: &Config) -> Result<()> {
    if !(MIN_M..=MAX_M).contains(&config.m) || !(1..=MAX_DIMS).contains(&config.dims) {
        return Err(SqlError::new(
            Code::Api,
            format!("[DESIGN] an HNSW index takes M from {MIN_M} to {MAX_M} and 1 to {MAX_DIMS} dimensions"),
        ));
    }
    Ok(())
}

/// The meta page: tag, version, dimensions, M, seed, train rows, the
/// metric's and quantizer's names, the image's and log's first pages.
fn meta_bytes(config: &Config, (metric, quantizer): (&str, Option<&str>), (image, log): (PageId, PageId)) -> Vec<u8> {
    let mut bytes = vec![META_TAG, META_VERSION];
    bytes.put_u32(u32::try_from(config.dims).unwrap_or(u32::MAX));
    bytes.put_u32(u32::try_from(config.m).unwrap_or(u32::MAX));
    bytes.put_u64(config.seed);
    bytes.put_u64(u64::try_from(config.train_rows).unwrap_or(u64::MAX));
    bytes.put_str(metric);
    bytes.put_str(quantizer.unwrap_or_default());
    bytes.put_u64(image.0);
    bytes.put_u64(log.0);
    bytes
}

/// What the meta page holds.
struct Meta {
    dims: usize,
    m: usize,
    seed: u64,
    train_rows: usize,
    metric: String,
    quantizer: String,
    image: PageId,
    log: PageId,
}

/// Reads the meta page `bytes`.
fn read_meta(bytes: &[u8]) -> Result<Meta> {
    let mut r = Reader::new(bytes, "HNSW metadata");
    if r.u8()? != META_TAG || r.u8()? != META_VERSION {
        return Err(r.corrupt("its tag or format version is unsupported"));
    }
    let dims = usize::try_from(r.u32()?).map_err(|_| r.corrupt("its dimensions are out of range"))?;
    let m = usize::try_from(r.u32()?).map_err(|_| r.corrupt("its M is out of range"))?;
    let seed = r.u64()?;
    let train_rows = usize::try_from(r.u64()?).unwrap_or(usize::MAX);
    let metric = r.str()?;
    let quantizer = r.str()?;
    let image = PageId(r.u64()?);
    let log = PageId(r.u64()?);
    r.finish()?;
    Ok(Meta { dims, m, seed, train_rows, metric, quantizer, image, log })
}

#[cfg(test)]
pub(crate) mod tests;

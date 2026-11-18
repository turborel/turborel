//! \[DESIGN\] IVF, the inverted file index: the vector design document's
//! NEIGHBOR PARTITIONS (§10.1, §10.4). Seeded k-means centroids divide the
//! vectors into lists (NLIST, sqrt(N) and at least 16 unless set), each a
//! chain of pages of (row, vector or code) entries; a search measures the
//! centroids, then scans the NPROBE nearest lists, widening to more while
//! a filter leaves fewer than k rows (R9.3). Only the centroids, the
//! lists' directory and a map from each entry to its page are in memory,
//! reserved of the database's budget (RULES.md D38); the vectors stay in
//! pages, so an index outgrows memory.
//!
//! Until it holds enough rows to train, an index is one list, searched
//! exactly (R10.5). Maintenance trains it, and trains it again as it
//! grows fourfold, or past 20% tombstones (R10.10): its lists are written
//! anew from the live entries, then the meta page names them, then the
//! old pages are freed, so a checkpoint captured meanwhile holds either.
//! A quantizer, if named, is trained with the first centroids, and the
//! lists then hold codes, re-ranked at full precision (R7.1).
//!
//! Every change is written through to its list's pages: an insert appends
//! an entry, a remove lowers its count in place. An entry is assigned the
//! centroid nearest its stored vector, a code decoded for a quantized
//! index, so the same vector always finds the same list.

mod kmeans;
mod lists;
mod open;
mod query;
mod train;
mod write;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex, RwLock};

use lists::{Layout, List};

use crate::bytes::{Put, Reader};
use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::pager::PageId;
use crate::engine::hnsw::{ChainReader, ChainWriter, Codec, Metric, Quantizer, free_pages, put_floats, put_varint};
use crate::error::{Code, Result, SqlError};
use crate::memory::{MemoryBudget, Reservation};

/// The meta page's tag, 'I'.
const META_TAG: u8 = 73;
/// The meta page's format.
const META_VERSION: u8 = 1;
/// \[DESIGN\] The most lists: past 65,536, measuring the centroids costs
/// more than scanning the lists saves.
pub const MAX_LISTS: usize = 65_536;
/// The least lists an automatic count gives (§10.4: "min 16").
pub const MIN_LISTS: usize = 16;
/// \[DESIGN\] The rows a list needs to train on: below 39 a centroid is
/// fit to too few points to stand for its list. An index trains once it
/// can train `MIN_LISTS`, with as many lists as its rows can train, up to
/// its target.
pub const TRAIN_PER_LIST: usize = 39;
/// \[DESIGN\] An index trained on n live rows trains again at 4n, as its
/// automatic count of lists, sqrt(N), has then doubled.
const RETRAIN_GROWTH: u64 = 4;
/// \[DESIGN\] A map entry's bytes, near enough, for its reservation: key,
/// place, and the hash map's share.
const LOCATE_BYTES: usize = 32;
/// The bits of a map entry's place given to the page; the list above.
const PAGE_BITS: u32 = 48;
/// The bytes of an `f32`.
const F32_BYTES: usize = 4;

/// How an index is built and searched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// Each vector's dimensions.
    pub dims: usize,
    /// NLIST: the lists; 0 for sqrt(N), at least `MIN_LISTS`.
    pub lists: usize,
    /// NPROBE: the lists a search scans; 0 for an eighth of them.
    pub probes: usize,
    /// The vectors a list's centroid is trained on (the document's
    /// SAMPLE_PER_PARTITION).
    pub sample_per_list: usize,
    /// The seed of every random choice.
    pub seed: u64,
    /// Whether a quantized index gives its candidates re-ranked.
    pub rerank: bool,
}

/// An IVF index in a pager file. Clones share it.
#[derive(Clone)]
pub struct Ivf {
    inner: Arc<Inner>,
}

/// \[DESIGN\] What a search reads of a state, fixed for the state's life
/// and shared with it, so a search takes it under the lock and reads its
/// lists' pages after letting the lock go (R-ERR-4). A state replaced by
/// training is retired: its pages are freed once the last search reading
/// them lets its shape go, never under one.
struct Shape {
    /// The centroids, `dims` each; none until trained.
    centroids: Vec<f32>,
    /// The codec, once a quantizer is trained.
    codec: Option<Arc<dyn Codec>>,
    layout: Layout,
    /// Each list's first page.
    heads: Vec<PageId>,
    /// The pages to free when it is let go, once retired.
    retired: Mutex<Option<Retired>>,
}

/// A retired state's pages.
struct Retired {
    pool: Arc<BufferPool>,
    file: FileId,
    directory: Vec<PageId>,
    centres: PageId,
}

impl Shape {
    /// A shape of no lists yet.
    fn new(centroids: Vec<f32>, codec: Option<Arc<dyn Codec>>, layout: Layout) -> Shape {
        Shape { centroids, codec, layout, heads: Vec::new(), retired: Mutex::new(None) }
    }
}

impl Drop for Shape {
    fn drop(&mut self) {
        let retired = match self.retired.get_mut() {
            Ok(retired) => retired.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        };
        if let Some(r) = retired {
            // The meta page names the state that replaced this one, and no
            // search reads it any more; a failed free leaks pages only.
            let _ = open::free_pages_of(&r.pool, r.file, (&self.heads, &r.directory, r.centres));
        }
    }
}

/// What an index holds in memory.
struct State {
    /// What searches read, shared with them.
    shape: Arc<Shape>,
    lists: Vec<List>,
    /// The directory's pages.
    directory: Vec<PageId>,
    /// The first page of the centroids and codec.
    centres: PageId,
    /// The live rows it was trained on; 0 untrained.
    trained_on: u64,
    /// The lists it was trained for: its lists, or more, set, than its
    /// rows could train.
    target: usize,
    live: u64,
    dead: u64,
    /// Each entry's list and page, by `place_key`.
    locate: HashMap<u64, u64>,
    /// Keys two entries share, or whose place the map cannot hold: their
    /// entries are found by scanning their list.
    collided: HashSet<u64>,
    /// Its changes: training, working with no lock held, puts its new
    /// state in place only if none came meanwhile.
    changes: u64,
}

/// The settings a change of an index's options makes in place.
struct Tuning {
    lists: AtomicUsize,
    probes: AtomicUsize,
    sample_per_list: AtomicUsize,
    rerank: AtomicBool,
}

struct Inner {
    pool: Arc<BufferPool>,
    file: FileId,
    meta: PageId,
    dims: usize,
    seed: u64,
    metric: Metric,
    quantizer: Option<Arc<dyn Quantizer>>,
    tuning: Tuning,
    state: RwLock<State>,
    /// The state's reservation and the bytes of it used.
    memory: Mutex<(Reservation, usize)>,
    budget: Arc<MemoryBudget>,
    /// Buffers of searches done, for the next.
    buffers: Mutex<Vec<query::Buffers>>,
}

impl Ivf {
    /// An empty index in `file`: one list, untrained.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 for no dimensions, or an entry too large for a page;
    /// the buffer pool's; -918 if the budget refuses.
    pub fn create(
        pool: Arc<BufferPool>,
        file: FileId,
        config: &Config,
        (metric, quantizer): (Metric, Option<Arc<dyn Quantizer>>),
        budget: &Arc<MemoryBudget>,
    ) -> Result<Ivf> {
        if config.dims == 0 || config.lists > MAX_LISTS {
            return Err(SqlError::new(
                Code::Api,
                format!("[DESIGN] an IVF index takes at least one dimension and at most {MAX_LISTS} lists"),
            ));
        }
        let layout = Layout::new(&pool, file, config.dims.saturating_mul(F32_BYTES))?;
        let memory = (budget.reserve(0, "an IVF index's centroids and map")?, 0);
        let mut made = Vec::new();
        let state = fresh(&pool, file, (layout, &mut made)).and_then(|state| {
            let (meta, mut page) = pool.new_page(file)?;
            *page = meta_bytes(config, (metric.name(), quantizer.as_ref().map(|q| q.name())), &state);
            Ok((meta, state))
        });
        let (meta, state) = match state {
            Ok(done) => done,
            Err(error) => {
                // Unpublished pages; a failed free leaks pages only.
                let _ = free_pages(&pool, file, &made);
                return Err(error);
            }
        };
        Ok(Ivf::with(Parts { pool, file, meta, config: *config, metric, quantizer, state, memory, budget: Arc::clone(budget) }))
    }

    fn with(p: Parts) -> Ivf {
        let c = p.config;
        Ivf {
            inner: Arc::new(Inner {
                pool: p.pool,
                file: p.file,
                meta: p.meta,
                dims: c.dims,
                seed: c.seed,
                metric: p.metric,
                quantizer: p.quantizer,
                tuning: Tuning {
                    lists: AtomicUsize::new(c.lists.min(MAX_LISTS)),
                    probes: AtomicUsize::new(c.probes),
                    sample_per_list: AtomicUsize::new(c.sample_per_list.max(1)),
                    rerank: AtomicBool::new(c.rerank),
                },
                state: RwLock::new(p.state),
                memory: Mutex::new(p.memory),
                budget: p.budget,
                buffers: Mutex::new(Vec::new()),
            }),
        }
    }

    /// The page that finds the index again (`open`).
    pub fn meta(&self) -> PageId {
        self.inner.meta
    }

    /// Each vector's dimensions.
    pub fn dims(&self) -> usize {
        self.inner.dims
    }

    /// Changes the options that change in place: the lists maintenance
    /// trains, the probes, the sample, re-ranking.
    pub fn tune(&self, config: &Config) {
        use std::sync::atomic::Ordering::Relaxed;
        let t = &self.inner.tuning;
        t.lists.store(config.lists.min(MAX_LISTS), Relaxed);
        t.probes.store(config.probes, Relaxed);
        t.sample_per_list.store(config.sample_per_list.max(1), Relaxed);
        t.rerank.store(config.rerank, Relaxed);
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
    state: State,
    memory: (Reservation, usize),
    budget: Arc<MemoryBudget>,
}

/// An untrained state: one empty list, its directory, no centroids; the
/// pages made pushed to `made`.
fn fresh(pool: &BufferPool, file: FileId, (layout, made): (Layout, &mut Vec<PageId>)) -> Result<State> {
    let list = lists::new_list(pool, file)?;
    made.push(list.head);
    let centres = write_centres(pool, file, &[], None)?;
    made.push(centres);
    let directory = lists::write_directory(pool, file, &layout, std::slice::from_ref(&list))?;
    made.extend_from_slice(&directory);
    let mut shape = Shape::new(Vec::new(), None, layout);
    shape.heads.push(list.head);
    Ok(State {
        shape: Arc::new(shape),
        lists: vec![list],
        directory,
        centres,
        trained_on: 0,
        target: 1,
        live: 0,
        dead: 0,
        locate: HashMap::new(),
        collided: HashSet::new(),
        changes: 0,
    })
}

/// Writes the centroids and codec to new pages: their first page.
fn write_centres(pool: &BufferPool, file: FileId, centroids: &[f32], codec: Option<&Arc<dyn Codec>>) -> Result<PageId> {
    let mut out = Vec::new();
    put_varint(&mut out, u64::try_from(centroids.len()).unwrap_or(u64::MAX));
    put_floats(&mut out, centroids);
    match codec {
        Some(codec) => {
            let mut bytes = Vec::new();
            codec.write(&mut bytes);
            out.push(1);
            put_varint(&mut out, u64::try_from(bytes.len()).unwrap_or(u64::MAX));
            out.extend_from_slice(&bytes);
        }
        None => out.push(0),
    }
    let mut w = ChainWriter::new(pool, file)?;
    if let Err(error) = w.put(&out) {
        // Unpublished; a failed free leaks pages only.
        let _ = w.abandon();
        return Err(error);
    }
    Ok(w.finish()?.0)
}

/// Centroids and the codec, if any.
type Centres = (Vec<f32>, Option<Arc<dyn Codec>>);

/// Reads the centroids and codec `write_centres` wrote at `head`.
fn read_centres(pool: &BufferPool, file: FileId, head: PageId, (dims, quantizer): (usize, Option<&Arc<dyn Quantizer>>)) -> Result<Centres> {
    let mut r = ChainReader::new(pool, file, head)?;
    let floats = usize::try_from(r.varint()?)
        .ok()
        .filter(|n| n.checked_rem(dims) == Some(0))
        .ok_or_else(|| corrupt("its centroids are malformed"))?;
    let mut centroids = vec![0.0_f32; floats];
    r.floats(&mut centroids)?;
    let codec = match (r.u8()?, quantizer) {
        (0, _) => None,
        (1, Some(quantizer)) => {
            let mut bytes = vec![0_u8; usize::try_from(r.varint()?).map_err(|_| corrupt("its codec is too long"))?];
            r.bytes(&mut bytes)?;
            Some(quantizer.read(&bytes, dims)?)
        }
        (_, _) => return Err(corrupt("its codec is unknown or its quantizer is missing")),
    };
    Ok((centroids, codec))
}

/// The meta page: tag, version, dimensions, seed, the metric's and
/// quantizer's names, the lists, the rows trained on, the centroids' and
/// directory's first pages.
fn meta_bytes(config: &Config, (metric, quantizer): (&str, Option<&str>), state: &State) -> Vec<u8> {
    let mut bytes = vec![META_TAG, META_VERSION];
    bytes.put_u32(u32::try_from(config.dims).unwrap_or(u32::MAX));
    bytes.put_u64(config.seed);
    bytes.put_str(metric);
    bytes.put_str(quantizer.unwrap_or_default());
    bytes.put_u32(u32::try_from(state.lists.len()).unwrap_or(u32::MAX));
    bytes.put_u64(state.trained_on);
    bytes.put_u64(state.centres.0);
    bytes.put_u64(state.directory.first().map_or(0, |p| p.0));
    bytes
}

/// The error of a change tried `TRIES` times, training replacing the
/// index's state under it each time.
fn contended() -> SqlError {
    SqlError::new(Code::Api, "[DESIGN] an IVF index kept being trained under a change of it")
}

/// A malformed index.
fn corrupt(why: &str) -> SqlError {
    SqlError::new(Code::Corrupt, format!("[DESIGN] an IVF index is malformed: {why}"))
}

/// The key of an entry of `row` and vector hash `hash` in the map.
fn place_key(row: u64, hash: u64) -> u64 {
    crate::engine::hnsw::random::mixed(row ^ crate::engine::hnsw::random::mixed(hash))
}

/// An entry's place: its list and page, packed; none if either is past
/// what a place holds.
fn place(list: usize, page: PageId) -> Option<u64> {
    let list = u64::try_from(list).ok().filter(|l| *l >> (u64::BITS - PAGE_BITS) == 0 && page.0 >> PAGE_BITS == 0)?;
    Some((list << PAGE_BITS) | page.0)
}

/// A packed place's list and page.
fn unplace(packed: u64) -> (usize, PageId) {
    (usize::try_from(packed >> PAGE_BITS).unwrap_or(usize::MAX), PageId(packed & ((1 << PAGE_BITS) - 1)))
}

/// What the meta page holds.
struct Meta {
    dims: usize,
    seed: u64,
    metric: String,
    quantizer: String,
    lists: usize,
    trained_on: u64,
    centres: PageId,
    directory: PageId,
}

/// Reads the meta page `bytes`.
fn read_meta(bytes: &[u8]) -> Result<Meta> {
    let mut r = Reader::new(bytes, "IVF metadata");
    if r.u8()? != META_TAG || r.u8()? != META_VERSION {
        return Err(r.corrupt("its tag or format version is unsupported"));
    }
    let dims = usize::try_from(r.u32()?).map_err(|_| r.corrupt("its dimensions are out of range"))?;
    let seed = r.u64()?;
    let metric = r.str()?;
    let quantizer = r.str()?;
    let lists = usize::try_from(r.u32()?)
        .ok()
        .filter(|n| (1..=MAX_LISTS).contains(n))
        .ok_or_else(|| r.corrupt("its list count is out of range"))?;
    let trained_on = r.u64()?;
    let centres = PageId(r.u64()?);
    let directory = PageId(r.u64()?);
    r.finish()?;
    Ok(Meta { dims, seed, metric, quantizer, lists, trained_on, centres, directory })
}

#[cfg(test)]
mod tests;

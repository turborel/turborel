//! \[EXTENSION\] The HNSW index method, "hnsw": an approximate
//! nearest-neighbour index of one fixed-dimension vector column, over the
//! engine in `engine/hnsw.rs` [Malkov and Yashunin, TPAMI 2020], the
//! vector design document's INMEMORY NEIGHBOR GRAPH (§10.1, §10.4). Its
//! options are in `options.rs`; what it shares with the other vector
//! methods, in `vector.rs`.
//!
//! Its graph is held in memory reserved of the database's budget
//! (RULES.md D38, the document's R10.7), and kept in the pages of the
//! method's file as an image and a log of changes since, appended as each
//! change is made, so the pages a checkpoint syncs reopen it without a
//! rebuild (R14.6). A checkpoint folds the log into a new image first.

pub mod options;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use self::options::options_of;
use super::vector::{Ask, Engine, Held, Place, VectorIndex, approximate_support, column_of, default_desc, located, plain};
use super::{Index, IndexMethod, NearestQuery, NearestSupport};
use crate::catalog::{IndexDesc, TableDesc};
use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::pager::PageId;
use crate::engine::MemoryPages;
use crate::engine::hnsw::{Hnsw, Search};
use crate::error::{Code, Result, SqlError};
use crate::memory::Host;
use crate::storage::TableOption;
use crate::value::{CompOp, DataType};
use crate::vector::metric::Metrics;
use crate::vector::quantizer::Quantizers;
use crate::vector::single;

/// The name of `HnswMethod`.
pub const HNSW_METHOD: &str = "hnsw";

/// \[EXTENSION\] The HNSW index method, in one pager file.
pub struct HnswMethod {
    place: Place<HnswEngine>,
}

impl HnswMethod {
    /// Stores its indexes in the registered file of this buffer pool.
    pub fn new(pool: Arc<BufferPool>, file: FileId) -> HnswMethod {
        HnswMethod { place: Place::new(pool, file) }
    }

    /// Creates its budgeted memory file lazily, when the first index is
    /// made. A database attaches its memory budget before using it.
    pub fn in_memory() -> HnswMethod {
        HnswMethod { place: Place::in_memory() }
    }

    pub(crate) fn sharing(memory: Arc<MemoryPages>) -> HnswMethod {
        HnswMethod { place: Place::sharing(memory) }
    }

    /// Ordering by the metrics `metrics` gives: the database's.
    #[must_use]
    pub fn with_metrics(self, metrics: Metrics) -> HnswMethod {
        HnswMethod { place: self.place.with_metrics(metrics) }
    }

    /// Compressing by the quantizers `quantizers` gives: the database's.
    #[must_use]
    pub fn with_quantizers(self, quantizers: Quantizers) -> HnswMethod {
        HnswMethod { place: self.place.with_quantizers(quantizers) }
    }

    /// An index of `columns` as `desc` describes, made anew or, of a
    /// locator, opened.
    fn index(&self, columns: &[DataType], desc: &IndexDesc, locator: Option<&[u8]>) -> Result<Box<dyn Index>> {
        let column = column_of(HNSW_METHOD, columns)?;
        plain(HNSW_METHOD, desc)?;
        let o = options_of(desc, column.width());
        let (plugins, metric) = self.place.codec_plugins((&o.metric, &o.quantizer), column, true)?;
        let (pool, file) = self.place.files()?;
        let budget = self.place.budget();
        let hnsw = match locator {
            Some(locator) => Hnsw::open(pool, (file, located(locator, column)?), &o.config, plugins, &budget)?,
            None => Hnsw::create(pool, file, &o.config, plugins, &budget)?,
        };
        if hnsw.dims() != column.width() {
            return Err(SqlError::new(Code::Corrupt, "[DESIGN] an HNSW index's dimensions differ from its column's"));
        }
        let held = Held::new(HnswEngine { hnsw, ef: AtomicUsize::new(o.config.ef_search), dims: column.width() });
        self.place.keep(&held);
        Ok(Box::new(VectorIndex::new(held, column, &metric)))
    }
}

impl IndexMethod for HnswMethod {
    fn supports_nearest(&self, query: &NearestQuery, columns: &[DataType]) -> NearestSupport {
        approximate_support(HNSW_METHOD, query, columns)
    }
    fn metric(&self, index: &IndexDesc) -> Option<String> {
        self.place.metric_name(&options_of(index, 0).metric)
    }
    fn name(&self) -> &'static str {
        HNSW_METHOD
    }
    fn attach(&self, host: &Host) -> Result<()> {
        self.place.attach(host);
        Ok(())
    }
    /// No comparison: vectors have no order (R3.11).
    fn supports(&self, _op: CompOp) -> bool {
        false
    }
    fn create(&self, columns: &[DataType]) -> Result<Box<dyn Index>> {
        self.index(columns, &default_desc(HNSW_METHOD), None)
    }
    fn open(&self, columns: &[DataType], locator: &[u8]) -> Result<Option<Box<dyn Index>>> {
        self.index(columns, &default_desc(HNSW_METHOD), Some(locator)).map(Some)
    }
    fn index_options(&self) -> Vec<TableOption> {
        options::options()
    }
    fn create_on(&self, columns: &[DataType], on: (&TableDesc, &IndexDesc)) -> Result<Box<dyn Index>> {
        self.index(columns, on.1, None)
    }
    fn open_on(&self, columns: &[DataType], locator: &[u8], on: (&TableDesc, &IndexDesc)) -> Result<Option<Box<dyn Index>>> {
        self.index(columns, on.1, Some(locator)).map(Some)
    }
    fn checkpoint(&self) -> Result<()> {
        self.place.checkpoint()
    }
}

/// An HNSW graph as a vector index's engine, with its efSearch.
pub(crate) struct HnswEngine {
    hnsw: Hnsw,
    ef: AtomicUsize,
    dims: usize,
}

impl Engine for HnswEngine {
    const METHOD: &'static str = HNSW_METHOD;
    fn meta(&self) -> PageId {
        self.hnsw.meta()
    }
    fn insert(&self, vector: &[f32], row: u64) -> Result<()> {
        self.hnsw.insert(vector, row)
    }
    fn remove(&self, vector: &[f32], row: u64) -> Result<()> {
        self.hnsw.remove(vector, row)
    }
    fn rows(&self, vector: &[f32]) -> Result<Vec<u64>> {
        self.hnsw.rows(vector)
    }
    fn load(&self, entries: &mut dyn Iterator<Item = Result<(Vec<f32>, u64)>>) -> Result<()> {
        self.hnsw.load(entries)
    }
    fn entries(&self) -> Box<dyn Iterator<Item = Result<u64>> + Send> {
        self.hnsw.entries()
    }
    fn search(&self, query: &[f32], ask: &Ask, filter: Option<&mut dyn FnMut(u64) -> bool>) -> Result<Vec<(f32, u64)>> {
        let ef = ask.list(self.ef.load(Ordering::Relaxed));
        let s = Search { ef: Some(ef), probes: None, filter, exact: None };
        match ask.radius {
            Some(radius) => self.hnsw.range(query, single(radius), s),
            None => self.hnsw.nearest(query, ask.wanted, s),
        }
    }
    fn live(&self) -> Option<u64> {
        self.hnsw.counts().ok().and_then(|(live, _)| u64::try_from(live).ok())
    }
    fn maintenance_due(&self) -> bool {
        self.hnsw.maintenance_due()
    }
    fn maintain(&self) -> Result<bool> {
        self.hnsw.maintain()
    }
    fn flush(&self) -> Result<()> {
        self.hnsw.save()
    }
    fn free(&self) -> Result<()> {
        self.hnsw.free()
    }
    fn retune(&self, desc: &IndexDesc) {
        let o = options_of(desc, self.dims);
        self.hnsw.tune(&o.config);
        self.ef.store(o.config.ef_search, Ordering::Relaxed);
    }
}

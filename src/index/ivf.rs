//! \[EXTENSION\] The IVF index method, "ivf": an approximate
//! nearest-neighbour index of one fixed-dimension vector column, its
//! vectors in lists by their nearest k-means centroid, over the engine in
//! `engine/ivf.rs` (the vector design document's NEIGHBOR PARTITIONS,
//! §10.1, §10.4). Its options are in `options.rs`; what it shares with
//! the other vector methods, in `vector.rs`.
//!
//! Its lists are pages of the method's file, written through as each
//! change is made; only its centroids, its lists' directory and a map of
//! its entries are in memory, reserved of the database's budget (RULES.md
//! D38). A checkpoint writes the directory's records, then syncs the
//! file. Not a graph, it may order by a metric that is not a true one,
//! DOT among them (R8.2 binds graph indexes alone).

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
use crate::engine::hnsw::Search;
use crate::engine::ivf::Ivf;
use crate::error::{Code, Result, SqlError};
use crate::memory::Host;
use crate::storage::TableOption;
use crate::value::{CompOp, DataType};
use crate::vector::metric::Metrics;
use crate::vector::quantizer::Quantizers;
use crate::vector::single;

/// The name of `IvfMethod`.
pub const IVF_METHOD: &str = "ivf";
/// An automatic NPROBE scans one list in this many, as the engine's.
const AUTO_PROBE_SHARE: usize = 8;

/// \[EXTENSION\] The IVF index method, in one pager file.
pub struct IvfMethod {
    place: Place<IvfEngine>,
}

impl IvfMethod {
    /// Stores its indexes in the registered file of this buffer pool.
    pub fn new(pool: Arc<BufferPool>, file: FileId) -> IvfMethod {
        IvfMethod { place: Place::new(pool, file) }
    }

    /// Creates its budgeted memory file lazily, when the first index is
    /// made. A database attaches its memory budget before using it.
    pub fn in_memory() -> IvfMethod {
        IvfMethod { place: Place::in_memory() }
    }

    pub(crate) fn sharing(memory: Arc<MemoryPages>) -> IvfMethod {
        IvfMethod { place: Place::sharing(memory) }
    }

    /// Ordering by the metrics `metrics` gives: the database's.
    #[must_use]
    pub fn with_metrics(self, metrics: Metrics) -> IvfMethod {
        IvfMethod { place: self.place.with_metrics(metrics) }
    }

    /// Compressing by the quantizers `quantizers` gives: the database's.
    #[must_use]
    pub fn with_quantizers(self, quantizers: Quantizers) -> IvfMethod {
        IvfMethod { place: self.place.with_quantizers(quantizers) }
    }

    /// An index of `columns` as `desc` describes, made anew or, of a
    /// locator, opened.
    fn index(&self, columns: &[DataType], desc: &IndexDesc, locator: Option<&[u8]>) -> Result<Box<dyn Index>> {
        let column = column_of(IVF_METHOD, columns)?;
        plain(IVF_METHOD, desc)?;
        let o = options_of(desc, column.width());
        let (plugins, metric) = self.place.codec_plugins((&o.metric, &o.quantizer), column, false)?;
        let (pool, file) = self.place.files()?;
        let budget = self.place.budget();
        let ivf = match locator {
            Some(locator) => Ivf::open(pool, (file, located(locator, column)?), &o.config, plugins, &budget)?,
            None => Ivf::create(pool, file, &o.config, plugins, &budget)?,
        };
        if ivf.dims() != column.width() {
            return Err(SqlError::new(Code::Corrupt, "[DESIGN] an IVF index's dimensions differ from its column's"));
        }
        let held = Held::new(IvfEngine { ivf, probes: AtomicUsize::new(o.config.probes), dims: column.width() });
        self.place.keep(&held);
        Ok(Box::new(VectorIndex::new(held, column, &metric)))
    }
}

impl IndexMethod for IvfMethod {
    fn supports_nearest(&self, query: &NearestQuery, columns: &[DataType]) -> NearestSupport {
        approximate_support(IVF_METHOD, query, columns)
    }
    fn metric(&self, index: &IndexDesc) -> Option<String> {
        self.place.metric_name(&options_of(index, 0).metric)
    }
    fn name(&self) -> &'static str {
        IVF_METHOD
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
        self.index(columns, &default_desc(IVF_METHOD), None)
    }
    fn open(&self, columns: &[DataType], locator: &[u8]) -> Result<Option<Box<dyn Index>>> {
        self.index(columns, &default_desc(IVF_METHOD), Some(locator)).map(Some)
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

/// An IVF index as a vector index's engine, with its NPROBE.
pub(crate) struct IvfEngine {
    ivf: Ivf,
    probes: AtomicUsize,
    dims: usize,
}

impl Engine for IvfEngine {
    const METHOD: &'static str = IVF_METHOD;
    fn meta(&self) -> PageId {
        self.ivf.meta()
    }
    fn insert(&self, vector: &[f32], row: u64) -> Result<()> {
        self.ivf.insert(vector, row)
    }
    fn remove(&self, vector: &[f32], row: u64) -> Result<()> {
        self.ivf.remove(vector, row)
    }
    fn rows(&self, vector: &[f32]) -> Result<Vec<u64>> {
        self.ivf.rows(vector)
    }
    fn load(&self, entries: &mut dyn Iterator<Item = Result<(Vec<f32>, u64)>>) -> Result<()> {
        self.ivf.load(entries)
    }
    fn entries(&self) -> Box<dyn Iterator<Item = Result<u64>> + Send> {
        self.ivf.entries()
    }
    fn search(&self, query: &[f32], ask: &Ask, filter: Option<&mut dyn FnMut(u64) -> bool>) -> Result<Vec<(f32, u64)>> {
        let (lists, _) = self.ivf.shape()?;
        let own = match self.probes.load(Ordering::Relaxed) {
            0 => lists.div_ceil(AUTO_PROBE_SHARE),
            set => set,
        };
        let s = Search { ef: None, probes: Some(ask.probes(lists, own)), filter, exact: None };
        match ask.radius {
            Some(radius) => self.ivf.range(query, single(radius), s),
            None => self.ivf.nearest(query, ask.wanted, s),
        }
    }
    fn live(&self) -> Option<u64> {
        self.ivf.counts().ok().map(|(live, _)| live)
    }
    fn maintenance_due(&self) -> bool {
        self.ivf.maintenance_due()
    }
    fn maintain(&self) -> Result<bool> {
        self.ivf.maintain()
    }
    fn flush(&self) -> Result<()> {
        self.ivf.save()
    }
    fn free(&self) -> Result<()> {
        self.ivf.free()
    }
    fn retune(&self, desc: &IndexDesc) {
        let o = options_of(desc, self.dims);
        self.ivf.tune(&o.config);
        self.probes.store(o.config.probes, Ordering::Relaxed);
    }
}

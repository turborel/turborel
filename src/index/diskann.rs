//! \[EXTENSION\] The DiskANN index method, "diskann": an approximate
//! nearest-neighbour index of one fixed-dimension vector column, over the
//! engine in `engine/diskann.rs` [DiskANN, NeurIPS 2019], the vector
//! design document's DISK NEIGHBOR GRAPH (§10.1, §10.2). Its options are
//! in `options.rs`; what it shares with the other vector methods, in
//! `vector.rs`.
//!
//! Its graph's nodes are in the pages of the method's file, its codes and
//! hot nodes in memory reserved of the database's budget (RULES.md D38).
//! A checkpoint writes each index's meta page, then syncs the file, so a
//! reopened index is as the checkpoint left it, and recovery mends it as
//! any index (D25). A bulk build (`Index::load`) runs Vamana's two passes
//! [§2.3], or the merged build past its memory [§3.1]; an index filled
//! row by row, as CREATE INDEX does while writers go on, grows by
//! FreshDiskANN's inserts, its codes trained by maintenance.

pub mod options;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use self::options::{Options, options_of};
use super::vector::{
    Ask, Column, Engine, Held, Place, VectorIndex, approximate_support, column_of, default_desc, disk_quantizer, distance, located, plain,
    serves,
};
use super::{Index, IndexMethod, NearestQuery, NearestSupport};
use crate::catalog::{IndexDesc, TableDesc};
use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::pager::PageId;
use crate::engine::MemoryPages;
use crate::engine::diskann::{DiskAnn, Layout, Plugins, Search};
use crate::error::{Code, Result, SqlError};
use crate::memory::Host;
use crate::storage::TableOption;
use crate::value::{CompOp, DataType};
use crate::vector::metric::Metrics;
use crate::vector::quantizer::Quantizers;

/// The name of `DiskAnnMethod`.
pub const DISKANN_METHOD: &str = "diskann";
/// \[DESIGN\] The most times an in-filtered search doubles its list while
/// its filter leaves fewer rows than asked, as the engine's own filter.
const MOST_ROUNDS: u32 = 16;

/// \[EXTENSION\] The DiskANN index method, in one pager file.
pub struct DiskAnnMethod {
    place: Place<DiskEngine>,
}

impl DiskAnnMethod {
    /// Stores its indexes in the registered file of this buffer pool.
    pub fn new(pool: Arc<BufferPool>, file: FileId) -> DiskAnnMethod {
        DiskAnnMethod { place: Place::new(pool, file) }
    }

    /// Creates its budgeted memory file lazily, when the first index is
    /// made. A database attaches its memory budget before using it.
    pub fn in_memory() -> DiskAnnMethod {
        DiskAnnMethod { place: Place::in_memory() }
    }

    pub(crate) fn sharing(memory: Arc<MemoryPages>) -> DiskAnnMethod {
        DiskAnnMethod { place: Place::sharing(memory) }
    }

    /// Ordering by the metrics `metrics` gives: the database's.
    #[must_use]
    pub fn with_metrics(self, metrics: Metrics) -> DiskAnnMethod {
        DiskAnnMethod { place: self.place.with_metrics(metrics) }
    }

    /// Compressing by the quantizers `quantizers` gives: the database's.
    #[must_use]
    pub fn with_quantizers(self, quantizers: Quantizers) -> DiskAnnMethod {
        DiskAnnMethod { place: self.place.with_quantizers(quantizers) }
    }

    /// The plug-ins of an index of `column` with options `o`, and the
    /// names of the metric it orders by and of the one it measures by.
    fn plugins(&self, o: &Options, column: Column) -> Result<(Plugins, (String, String))> {
        let m = self.place.metric((&o.metric, true, o.shared.normalized), column)?;
        let metric = m.metric;
        let quantizer = self.place.quantizer(&o.quantizer)?.map(|q| disk_quantizer(q, Arc::clone(&metric), o.shared.code_bytes));
        let measure = distance(&metric, column);
        let plugins = Plugins { metric: Arc::new(move |a: &[f32], b: &[f32]| measure(a, b)), quantizer };
        Ok((plugins, m.names))
    }

    /// An index of `columns` as `desc` describes, made anew or, of a
    /// locator, opened.
    fn index(&self, columns: &[DataType], desc: &IndexDesc, locator: Option<&[u8]>) -> Result<Box<dyn Index>> {
        let column = column_of(DISKANN_METHOD, columns)?;
        plain(DISKANN_METHOD, desc)?;
        let o = options_of(desc)?;
        let (plugins, metric) = self.plugins(&o, column)?;
        let (pool, file) = self.place.files()?;
        let ann = if let Some(locator) = locator {
            DiskAnn::open(pool, file, located(locator, column)?, (o.tuning, plugins), self.place.budget())?
        } else {
            let layout = Layout { dims: column.dims, degree: o.degree, element: o.element };
            DiskAnn::create(pool, file, layout, (o.tuning, plugins), self.place.budget())?
        };
        if ann.layout().dims != column.dims {
            return Err(SqlError::new(Code::Corrupt, "[DESIGN] a DiskANN index's dimensions differ from its column's"));
        }
        let held = Held::new(DiskEngine { ann, rerank: AtomicBool::new(o.rerank) });
        self.place.keep(&held);
        Ok(Box::new(VectorIndex::new(held, column, (&metric.0, &metric.1), o.shared)))
    }
}

impl IndexMethod for DiskAnnMethod {
    fn supports_nearest(&self, query: &NearestQuery, columns: &[DataType]) -> NearestSupport {
        approximate_support(DISKANN_METHOD, query, columns)
    }
    fn serves_metric(&self, index: &IndexDesc, call: &str) -> bool {
        let normalized = options_of(index).is_ok_and(|o| o.shared.normalized);
        self.metric(index).is_some_and(|m| serves(&m, normalized, call))
    }
    fn metric(&self, index: &IndexDesc) -> Option<String> {
        self.place.metric_name(&options_of(index).ok()?.metric)
    }
    fn name(&self) -> &'static str {
        DISKANN_METHOD
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
        self.index(columns, &default_desc(DISKANN_METHOD), None)
    }
    fn open(&self, columns: &[DataType], locator: &[u8]) -> Result<Option<Box<dyn Index>>> {
        self.index(columns, &default_desc(DISKANN_METHOD), Some(locator)).map(Some)
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

/// A DiskANN graph as a vector index's engine, and its RERANK.
pub(crate) struct DiskEngine {
    ann: DiskAnn,
    rerank: AtomicBool,
}

impl DiskEngine {
    /// The rows `filter` admits of a search widened until it finds
    /// `ask.wanted` of them, its list doubled each time, as the engine's
    /// own filter does; each row asked of once. The engine's filter must
    /// be `Sync`, which a filter reading the table is not, so it is asked
    /// here, after each search, with no lock held.
    fn filtered(&self, query: &[f32], (ask, list): (&Ask, usize), filter: &mut dyn FnMut(u64) -> bool) -> Result<Vec<(f32, u64)>> {
        let mut asked: HashMap<u64, bool> = HashMap::new();
        let mut list = list;
        let mut kept = Vec::new();
        for _ in 0..MOST_ROUNDS {
            let wide = u32::try_from(list).unwrap_or(u32::MAX);
            let s = Search { results: list, list: Some(wide), width: ask.beam_width, radius: ask.radius, ..Search::default() };
            let (found, _) = self.ann.search(query, &s)?;
            let complete = found.len() < list;
            kept.clear();
            for c in found {
                if ask.radius.is_none() && kept.len() >= ask.wanted {
                    break;
                }
                let admitted = if let Some(admitted) = asked.get(&c.row) {
                    *admitted
                } else {
                    let admitted = filter(c.row);
                    asked.insert(c.row, admitted);
                    admitted
                };
                if admitted {
                    kept.push((c.distance, c.row));
                }
            }
            if complete || kept.len() >= ask.wanted || ask.radius.is_some() {
                break;
            }
            list = list.saturating_mul(2);
        }
        Ok(kept)
    }
}

impl Engine for DiskEngine {
    const METHOD: &'static str = DISKANN_METHOD;
    const LIST_OPTION: &'static str = options::SEARCH_LIST;
    fn meta(&self) -> PageId {
        self.ann.meta()
    }
    fn insert(&self, vector: &[f32], row: u64) -> Result<()> {
        self.ann.insert(vector, row)
    }
    fn remove(&self, vector: &[f32], row: u64) -> Result<()> {
        self.ann.remove(vector, row)
    }
    fn rows(&self, vector: &[f32]) -> Result<Vec<u64>> {
        self.ann.rows(vector)
    }
    fn load(&self, entries: &mut dyn Iterator<Item = Result<(Vec<f32>, u64)>>) -> Result<()> {
        self.ann.load(entries)
    }
    fn entries(&self) -> Box<dyn Iterator<Item = Result<u64>> + Send> {
        self.ann.entries()
    }
    fn search(&self, query: &[f32], ask: &Ask, filter: Option<&mut dyn FnMut(u64) -> bool>) -> Result<Vec<(f32, u64)>> {
        let own = usize::try_from(self.ann.tuning()?.search_list).unwrap_or(1);
        let list = ask.list(own);
        if let Some(filter) = filter {
            return self.filtered(query, (ask, list), filter);
        }
        // RERANK ON gives the executor the whole list to order again by
        // the SQL distance; OFF the rows asked alone, in the index's order.
        let results = if self.rerank.load(Ordering::Relaxed) { list.max(ask.wanted) } else { ask.wanted };
        let s = Search {
            results,
            list: Some(u32::try_from(list).unwrap_or(u32::MAX)),
            width: ask.beam_width,
            selectivity: ask.selectivity,
            radius: ask.radius,
            filter: None,
        };
        let (found, _) = self.ann.search(query, &s)?;
        Ok(found.into_iter().map(|c| (c.distance, c.row)).collect())
    }
    fn live(&self) -> Option<u64> {
        Some(self.ann.counts().live)
    }
    fn maintenance_due(&self) -> bool {
        self.ann.maintenance_due()
    }
    fn maintain(&self) -> Result<bool> {
        self.ann.maintain()
    }
    fn flush(&self) -> Result<()> {
        self.ann.flush()
    }
    fn free(&self) -> Result<()> {
        self.ann.free()
    }
    fn retune(&self, desc: &IndexDesc) {
        if let Ok(o) = options_of(desc) {
            self.rerank.store(o.rerank, Ordering::Relaxed);
            // Reconfiguring cannot report; a refused retune leaves the
            // old tuning, which still searches correctly.
            let _ = self.ann.retune(o.tuning);
        }
    }
}

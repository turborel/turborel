//! \[EXTENSION\] The built-in sparse inverted index, "inverted", of the
//! vector design's §10.1 ("SPARSE INVERTED: posting list per dimension
//! with block-max WAND") and §10.2 (`ORGANIZATION INVERTED`,
//! `PARAMETERS (TYPE INVERTED, BLOCK_SIZE n)`). It indexes one column of
//! SPARSE vectors (R10.3: INVERTED requires SPARSE), each a §4.2 image,
//! in a sparse inverted index (`engine/sparse.rs`) in a pager file of a
//! buffer pool, as the rtree index keeps its tree.
//!
//! Its keys are projected (D85), as the dense vector indexes' are: every
//! vector to one constant key, a null to itself. The engine counts an
//! entry under the hash of its vector, not the vector, which may be large,
//! and its searches find rows, not hashes, so a reader takes each row as
//! it sees it and the executor measures it again (D-APPROX). `rows` of a
//! vector gives the rows of that vector exactly. NULL vectors are not
//! indexed (R10.6). A key of a null is refused, as no NULLS NOT DISTINCT
//! index of this method is made.
//!
//! Top-k and range searches by dot product are the engine's
//! (`SparseTree::top_k`, `SparseTree::within`). An approximate search of
//! a call of DOT reads them (`nearest.rs`).

mod nearest;
mod options;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub use options::{BLOCK_SIZE_OPTION, QUANTIZATION_OPTION, VECTOR_TYPE};

use super::{
    Entries, Index, IndexMethod, Key, KeyOrder, KeyProjection, NearestQuery, NearestSupport, NeighbourEntries, Probe, RowFilter, RowIds,
};
use crate::catalog::{IndexDesc, TableDesc};
use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::io::Durability;
use crate::engine::sparse::{HASH_BYTES, Layout, SparseTree, SparseVector};
use crate::engine::{MEMORY_POOL_BYTES, MemoryPages};
use crate::error::{Code, Result, SqlError};
use crate::memory::{Host, MemoryBudget};
use crate::spill::{Area, WorkAreas};
use crate::storage::{RowId, TableOption};
use crate::sync::lock_builtin;
use crate::value::{CompOp, DataType, Value};
use crate::vector::Element;
use crate::vector::metric::DOT;

/// The name of `SparseMethod`.
pub const SPARSE_METHOD: &str = "inverted";

fn unsupported(detail: &str) -> SqlError {
    SqlError::new(Code::Unsupported, format!("[EXTENSION] the inverted index method {detail}"))
}

/// \[EXTENSION\] The sparse inverted index method, in one pager file.
pub struct SparseMethod {
    files: Mutex<Option<(Arc<BufferPool>, FileId)>>,
    memory: Arc<MemoryPages>,
}

impl SparseMethod {
    /// Stores its indexes in the registered file of this buffer pool.
    pub fn new(pool: Arc<BufferPool>, file: FileId) -> SparseMethod {
        SparseMethod { files: Mutex::new(Some((pool, file))), memory: MemoryPages::new(0) }
    }

    /// Creates its budgeted memory file lazily, when the first index is
    /// made. A database attaches its memory budget before using it.
    pub fn in_memory() -> SparseMethod {
        SparseMethod::sharing(MemoryPages::new(MEMORY_POOL_BYTES))
    }

    /// A method keeping its indexes in `memory`, which others may share.
    pub(crate) fn sharing(memory: Arc<MemoryPages>) -> SparseMethod {
        SparseMethod { files: Mutex::new(None), memory }
    }

    fn files(&self) -> Result<(Arc<BufferPool>, FileId)> {
        let mut files = lock_builtin(&self.files);
        if let Some((pool, file)) = files.as_ref() {
            return Ok((Arc::clone(pool), *file));
        }
        let (pool, file) = self.memory.file("the pool in memory of the built-in engine and index methods")?;
        *files = Some((Arc::clone(&pool), file));
        Ok((pool, file))
    }

    /// A new index of `layout`, its column's type not checked: `create`'s
    /// work, for the method and its tests.
    pub(crate) fn make(&self, layout: Layout) -> Result<SparseIndex> {
        let (pool, file) = self.files()?;
        Ok(SparseIndex::new(SparseTree::create(pool, file, layout)?, None))
    }

    /// The index `locator` finds, of `layout`, its type not checked.
    pub(crate) fn reopen(&self, locator: &[u8], layout: Layout) -> Result<SparseIndex> {
        let meta = options::located(locator)?;
        let (pool, file) = self.files()?;
        Ok(SparseIndex::new(SparseTree::open(pool, file, meta, layout)?, None))
    }
}

/// Refuses columns that are not one VECTOR column, or one of DENSE
/// layout (R10.3); else the dimension count and format the column fixes,
/// if it fixes both and SPARSE.
fn validate(columns: &[DataType]) -> Result<Option<(u32, Element)>> {
    let refused = || unsupported("indexes one VECTOR column of SPARSE layout (R10.3)");
    let [column] = columns else { return Err(refused()) };
    let DataType::Extension(t) = column else { return Err(refused()) };
    let dense = crate::vector::shape_of(column).and_then(|s| s.layout) == Some(crate::vector::Layout::Dense);
    if !t.kind.name().eq_ignore_ascii_case(VECTOR_TYPE) || dense {
        return Err(refused());
    }
    Ok(nearest::fixed_sparse(column))
}

impl IndexMethod for SparseMethod {
    fn name(&self) -> &'static str {
        SPARSE_METHOD
    }
    fn attach(&self, host: &Host) -> Result<()> {
        self.memory.use_budget(Arc::clone(&host.memory));
        Ok(())
    }
    /// None, as vectors have no order (R3.11).
    fn supports(&self, _op: CompOp) -> bool {
        false
    }
    /// Approximately, of a call of DOT on a column of a fixed sparse
    /// shape (`nearest.rs`).
    fn supports_nearest(&self, query: &NearestQuery, columns: &[DataType]) -> NearestSupport {
        nearest::support(query, columns)
    }
    /// DOT: the rows of the greatest dot product are the nearest by it.
    fn metric(&self, _index: &IndexDesc) -> Option<String> {
        Some(DOT.to_owned())
    }
    fn create(&self, columns: &[DataType]) -> Result<Box<dyn Index>> {
        let shape = validate(columns)?;
        Ok(Box::new(self.make(Layout::default())?.of_shape(shape)))
    }
    fn open(&self, columns: &[DataType], locator: &[u8]) -> Result<Option<Box<dyn Index>>> {
        let shape = validate(columns)?;
        Ok(Some(Box::new(self.reopen(locator, Layout::default())?.of_shape(shape))))
    }
    fn index_options(&self) -> Vec<TableOption> {
        options::options()
    }
    fn create_on(&self, columns: &[DataType], on: (&TableDesc, &IndexDesc)) -> Result<Box<dyn Index>> {
        if on.1.nulls || on.1.unique {
            return Err(unsupported("makes no UNIQUE or NULLS NOT DISTINCT index"));
        }
        let shape = validate(columns)?;
        Ok(Box::new(self.make(options::layout_of(on.1)?)?.of_shape(shape)))
    }
    fn open_on(&self, columns: &[DataType], locator: &[u8], on: (&TableDesc, &IndexDesc)) -> Result<Option<Box<dyn Index>>> {
        let shape = validate(columns)?;
        Ok(Some(Box::new(self.reopen(locator, options::layout_of(on.1)?)?.of_shape(shape))))
    }
    fn checkpoint(&self) -> Result<()> {
        let (pool, file) = self.files()?;
        pool.sync(file, Durability::Full)
    }
}

/// The hash a key is counted under: of its vector; none of the projected
/// key every vector shares.
fn hash_of(key: &Key) -> Result<Option<[u8; HASH_BYTES]>> {
    match key.0.as_slice() {
        [Value::Binary(_)] => Ok(None),
        [_] => Ok(Some(vector_of(key)?.hash())),
        _ => Err(unsupported("received a key of more than one column")),
    }
}

/// The vector of a key: a VECTOR value's §4.2 image.
fn vector_of(key: &Key) -> Result<SparseVector> {
    match key.0.as_slice() {
        [Value::Extension(v)] => SparseVector::from_image(&v.bytes),
        [Value::Null] => Err(unsupported("indexes no NULL vector (R10.6)")),
        _ => Err(unsupported("received a key that is not a VECTOR value")),
    }
}

/// \[EXTENSION\] An index of the method: a sparse inverted index.
pub struct SparseIndex {
    tree: SparseTree,
    discarded: AtomicBool,
    projection: Arc<dyn KeyProjection>,
    /// The dimension count and format its column fixes, if it fixes both:
    /// of none, it answers no approximate search (`nearest.rs`).
    shape: Option<(u32, Element)>,
}

impl SparseIndex {
    fn new(tree: SparseTree, shape: Option<(u32, Element)>) -> SparseIndex {
        SparseIndex { tree, discarded: AtomicBool::new(false), projection: super::vector::projection(), shape }
    }

    /// It, of a column fixing `shape`.
    fn of_shape(mut self, shape: Option<(u32, Element)>) -> SparseIndex {
        self.shape = shape;
        self
    }

    /// Its sparse inverted index, for top-k and range searches.
    pub fn tree(&self) -> &SparseTree {
        &self.tree
    }
}

impl Drop for SparseIndex {
    fn drop(&mut self) {
        if self.discarded.load(Ordering::Acquire) {
            // The last reader has released the index. A failure leaks
            // pages only; Drop cannot report it.
            let _ = self.tree.free();
        }
    }
}

impl Index for SparseIndex {
    fn approximate(
        self: Arc<Self>,
        query: &NearestQuery,
        budget: &Arc<MemoryBudget>,
        filter: Option<&mut dyn RowFilter>,
    ) -> Result<Option<NeighbourEntries>> {
        let Some(found) = nearest::candidates(&self.tree, self.shape, (query, budget), filter)? else { return Ok(None) };
        Ok(nearest::stream(self, found, budget))
    }
    fn key_projection(&self) -> Option<Arc<dyn KeyProjection>> {
        Some(Arc::clone(&self.projection))
    }
    fn insert(&self, key: Key, row: RowId) -> Result<()> {
        self.tree.insert(&vector_of(&key)?, row)
    }
    fn insert_alone(&self, key: Key, row: RowId) -> Result<bool> {
        self.insert(key, row)?;
        // No UNIQUE index is made of this method.
        Ok(false)
    }
    fn load(&self, entries: &mut dyn Iterator<Item = Result<(Key, RowId)>>, work: (&WorkAreas, Area)) -> Result<()> {
        let mut vectors = entries.map(|entry| entry.and_then(|(key, row)| Ok((vector_of(&key)?, row))));
        self.tree.load(&mut vectors, work)
    }
    fn remove(&self, key: &Key, row: RowId) -> Result<()> {
        self.tree.remove(&vector_of(key)?, row)
    }
    /// Of a vector, the rows of that vector; of the projected key, every
    /// row, each once.
    fn rows(&self, key: &Key) -> Result<Vec<RowId>> {
        if let Some(hash) = hash_of(key)? {
            return self.tree.rows(&hash);
        }
        let mut rows = self.tree.entries().map(|entry| entry.map(|(_, row)| row)).collect::<Result<Vec<_>>>()?;
        rows.sort_unstable();
        rows.dedup();
        Ok(rows)
    }
    fn key_rows(self: Arc<Self>, key: &Key) -> RowIds {
        match self.rows(key) {
            Ok(found) => Box::new(found.into_iter().map(Ok)),
            Err(error) => Box::new(std::iter::once(Err(error))),
        }
    }
    fn estimate(&self, probe: &Probe) -> Option<usize> {
        match probe {
            Probe::All => self.tree.documents().and_then(|n| usize::try_from(n).ok()),
            Probe::Columns(_) | Probe::Compare { .. } | Probe::Prefix { .. } | Probe::Range { .. } | Probe::Predicate { .. } => None,
        }
    }
    fn distinct_keys(&self) -> Option<f64> {
        self.tree.documents().and_then(|n| u32::try_from(n).ok()).map(f64::from)
    }
    fn maintenance_due(&self) -> bool {
        self.tree.maintenance_due()
    }
    fn maintain(&self) -> Result<bool> {
        self.tree.maintain()
    }
    fn search(self: Arc<Self>, probe: &Probe, _order: KeyOrder) -> Entries {
        match probe {
            Probe::All => {
                let found = self.tree.entries();
                let key = super::vector::entry_key();
                Box::new(found.map(move |entry| {
                    let _held = &self;
                    entry.map(|(_, row)| (key.clone(), row))
                }))
            }
            Probe::Columns(_) | Probe::Compare { .. } | Probe::Prefix { .. } | Probe::Range { .. } | Probe::Predicate { .. } => {
                Box::new(std::iter::once(Err(unsupported("answers every entry and top-k searches, no comparison"))))
            }
        }
    }
    fn locator(&self) -> Vec<u8> {
        options::locator(self.tree.meta())
    }
    fn discard(&self) {
        self.discarded.store(true, Ordering::Release);
    }
    fn reconfigure(&self, on: (&TableDesc, &IndexDesc)) {
        if let Ok(layout) = options::layout_of(on.1) {
            self.tree.set_layout(layout);
        }
    }
}

#[cfg(test)]
mod tests;

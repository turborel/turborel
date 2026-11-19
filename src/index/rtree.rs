//! \[DESIGN\] The built-in Cartesian index, "rtree" (D72). Two
//! approximate-numeric columns are a point (X, Y); four are a rectangle
//! (min X, min Y, max X, max Y). The key retains every coordinate, so
//! ordinary SQL comparisons and MVCC key rechecks remain exact. A single
//! GEOMETRY(srid) column instead stores envelope projections and rechecks
//! the visible geometry and full spatial predicate (D85).

mod clustered;
mod geometry;
mod paired;
mod probe;

pub use paired::{Envelopes, KeyPairs};

use super::{
    Entries, Index, IndexMethod, Key, KeyOrder, KeyProjection, NearestQuery, NearestSupport, NeighbourEntries, Predicate, Probe,
    RTREE_METHOD, RowIds,
};
use crate::bytes::{Put, Reader};
use crate::catalog::{IndexDesc, TableDesc};
use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::io::Durability;
use crate::disk::pager::PageId;
use crate::engine::rtree::RTree;
use crate::engine::{MEMORY_POOL_BYTES, MemoryPages};
use crate::error::{Code, Result, SqlError};
use crate::memory::{Host, MemoryBudget};
use crate::spatial::{Bounds, DIMENSIONS};
use crate::spill::{Area, WorkAreas};
use crate::storage::RowId;
use crate::sync::lock_builtin;
use crate::value::{CompOp, DataType, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Four coordinates describe two lower and two upper bounds.
const RECTANGLE_COLUMNS: usize = 4;
/// Version of the method's locator, independent of the tree's pages.
const LOCATOR_VERSION: u8 = 1;
/// Geometry locators also bind the enforced reference identifier.
const GEOMETRY_LOCATOR_VERSION: u8 = 2;
/// Bound planning work even when a probe matches most of the index.
const MOST_COUNTED: usize = 1_000;

/// An R-link index method in one pager file. Its indexes use R* subtree
/// selection and splits without forced reinsertion (RULES.md D72).
pub struct RTreeMethod {
    files: Mutex<Option<(Arc<BufferPool>, FileId)>>,
    memory: Arc<MemoryPages>,
}

impl RTreeMethod {
    /// Stores its indexes in the registered file of this buffer pool.
    pub fn new(pool: Arc<BufferPool>, file: FileId) -> RTreeMethod {
        RTreeMethod { files: Mutex::new(Some((pool, file))), memory: MemoryPages::new(0) }
    }

    /// Creates its budgeted memory file lazily, when the first index is
    /// made. A database attaches its memory budget before using it.
    pub fn in_memory() -> RTreeMethod {
        Self::sharing(MemoryPages::new(MEMORY_POOL_BYTES))
    }

    pub(crate) fn sharing(memory: Arc<MemoryPages>) -> RTreeMethod {
        RTreeMethod { files: Mutex::new(None), memory }
    }

    /// Temporary instances need the same projection before their pages
    /// are made, so a prepared reader sees a fixed contract (D49, D85).
    pub(crate) fn projection(columns: &[DataType]) -> Option<Arc<dyn KeyProjection>> {
        let [column] = columns else { return None };
        crate::geometry::fixed_srid(column).map(geometry::projection)
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
}

impl IndexMethod for RTreeMethod {
    fn supports_nearest(&self, query: &NearestQuery, columns: &[DataType]) -> NearestSupport {
        let [column] = columns else { return NearestSupport::No };
        let certified = crate::geometry::fixed_srid(column).is_some() && geometry::supports_nearest(query);
        if certified { NearestSupport::Exact } else { NearestSupport::No }
    }
    fn name(&self) -> &'static str {
        RTREE_METHOD
    }
    fn attach(&self, host: &Host) -> Result<()> {
        self.memory.use_budget(Arc::clone(&host.memory));
        Ok(())
    }
    fn supports(&self, op: CompOp) -> bool {
        op != CompOp::Ne
    }
    fn supports_compare(&self, op: CompOp, columns: &[DataType]) -> bool {
        numeric_width(columns.len()) && self.supports(op)
    }
    fn supports_prefix(&self, columns: usize, equal: usize, then: Option<CompOp>) -> bool {
        numeric_width(columns)
            && equal > 0
            && equal.saturating_add(usize::from(then.is_some())) <= columns
            && then.is_none_or(|op| self.supports(op))
    }
    fn supports_range(&self, columns: usize, equal: usize) -> bool {
        numeric_width(columns) && equal < columns
    }
    fn supports_columns(&self, columns: usize, comparisons: &[(usize, CompOp)]) -> bool {
        numeric_width(columns) && !comparisons.is_empty() && comparisons.iter().all(|(at, op)| *at < columns && self.supports(*op))
    }
    fn supports_predicate(&self, predicate: &Predicate, columns: &[DataType]) -> bool {
        let [column] = columns else { return false };
        crate::geometry::fixed_srid(column).is_some() && geometry::supports(predicate)
    }
    fn create(&self, columns: &[DataType]) -> Result<Box<dyn Index>> {
        let srid = validate(columns)?;
        let (pool, file) = self.files()?;
        Ok(Box::new(RTreeIndex::new(RTree::create(pool, file)?, columns.len(), srid, self.memory.budget())))
    }
    fn create_on(&self, columns: &[DataType], on: (&TableDesc, &IndexDesc)) -> Result<Box<dyn Index>> {
        if on.1.nulls {
            return Err(unsupported("does not store NULLS NOT DISTINCT keys"));
        }
        self.create(columns)
    }
    fn open(&self, columns: &[DataType], locator: &[u8]) -> Result<Option<Box<dyn Index>>> {
        let srid = validate(columns)?;
        let mut reader = Reader::new(locator, "R-tree index locator");
        let version = reader.u8()?;
        let width = usize::from(reader.u8()?);
        let expected = if srid.is_some() { GEOMETRY_LOCATOR_VERSION } else { LOCATOR_VERSION };
        if version != expected || width != columns.len() {
            return Err(reader.corrupt("its version or column count differs"));
        }
        if let Some(srid) = srid {
            let stored = reader.u32()?;
            if i32::try_from(stored).ok() != Some(srid) {
                return Err(reader.corrupt("its geometry SRID differs from its column"));
            }
        }
        let meta = PageId(reader.u64()?);
        reader.finish()?;
        let (pool, file) = self.files()?;
        Ok(Some(Box::new(RTreeIndex::new(RTree::open(pool, file, meta)?, width, srid, self.memory.budget()))))
    }
    fn checkpoint(&self) -> Result<()> {
        let (pool, file) = self.files()?;
        pool.sync(file, Durability::Full)
    }
}

fn validate(columns: &[DataType]) -> Result<Option<i32>> {
    if let [column] = columns {
        return crate::geometry::fixed_srid(column).map(Some).ok_or_else(|| unsupported("needs GEOMETRY(srid) for a geometry key"));
    }
    if !numeric_width(columns.len()) {
        return Err(unsupported("needs GEOMETRY(srid), two point columns or four rectangle columns"));
    }
    if columns.iter().any(|t| !matches!(t, DataType::Approximate { .. })) {
        return Err(unsupported("needs REAL, FLOAT or DOUBLE PRECISION coordinate columns"));
    }
    Ok(None)
}

fn numeric_width(width: usize) -> bool {
    width == DIMENSIONS || width == RECTANGLE_COLUMNS
}

fn unsupported(detail: &str) -> SqlError {
    SqlError::new(Code::Unsupported, format!("[EXTENSION] the rtree index method {detail}"))
}

struct RTreeIndex {
    tree: RTree,
    width: usize,
    discarded: AtomicBool,
    srid: Option<i32>,
    projection: Option<Arc<dyn KeyProjection>>,
    budget: Arc<MemoryBudget>,
}

impl RTreeIndex {
    fn new(tree: RTree, width: usize, srid: Option<i32>, budget: Arc<MemoryBudget>) -> RTreeIndex {
        RTreeIndex { tree, width, discarded: AtomicBool::new(false), srid, projection: srid.map(geometry::projection), budget }
    }

    fn bounds(&self, key: &Key) -> Result<Bounds> {
        if let Some(srid) = self.srid {
            return geometry::bounds(key, srid);
        }
        if key.0.len() != self.width {
            return Err(unsupported("received an invalid coordinate count"));
        }
        match key.0.as_slice() {
            [Value::Approx(x), Value::Approx(y)] => Bounds::point([*x, *y]),
            [Value::Approx(min_x), Value::Approx(min_y), Value::Approx(max_x), Value::Approx(max_y)] => {
                Bounds::new([*min_x, *min_y], [*max_x, *max_y])
            }
            _ => Err(unsupported("received an invalid coordinate key")),
        }
    }

    fn query_key(&self, key: &Key) -> Result<()> {
        if key.0.len() != self.width || key.0.iter().any(|v| !v.is_null() && v.as_f64().is_none()) {
            return Err(unsupported("received a key of the wrong width or kind"));
        }
        Ok(())
    }

    fn matching(&self, probe: &Probe) -> Entries {
        if let Some(srid) = self.srid {
            return geometry::matching(&self.tree, probe, srid, &self.budget);
        }
        let window = probe::window(probe, self.width);
        let (tree, width, probe) = (self.tree.clone(), self.width, probe.clone());
        let scan = window.map(|window| tree.scan(Some(window)));
        Box::new(scan.into_iter().flatten().filter_map(move |entry| {
            entry.map(|(bounds, row)| probe::answers(&probe, bounds, width).then(|| (probe::key(bounds, width), row))).transpose()
        }))
    }

    fn equality(&self, key: &Key) -> Result<Entries> {
        if let Some(srid) = self.srid {
            return geometry::equal(&self.tree, key, srid);
        }
        self.query_key(key)?;
        Ok(self.matching(&Probe::Prefix { equal: key.0.clone(), then: None }))
    }
}

impl Drop for RTreeIndex {
    fn drop(&mut self) {
        if self.discarded.load(Ordering::Acquire) {
            // The last cursor has released the index. Cleanup failure
            // leaks pages only; Drop cannot report it.
            let _ = self.tree.free();
        }
    }
}

impl Index for RTreeIndex {
    fn nearest(self: Arc<Self>, query: &NearestQuery, budget: &Arc<MemoryBudget>) -> Result<Option<NeighbourEntries>> {
        let Some(srid) = self.srid else { return Ok(None) };
        let Some(found) = geometry::nearest(&self.tree, query, srid, budget)? else { return Ok(None) };
        Ok(Some(Box::new(found.inspect(move |_| {
            let _held = &self;
        }))))
    }
    fn key_projection(&self) -> Option<Arc<dyn KeyProjection>> {
        self.projection.clone()
    }
    /// Of a geometry window, the window: its search reads no other entry.
    /// A numeric index's probes are comparisons, recorded as conditions.
    fn read_region(&self, probe: &Probe) -> Option<Vec<Bounds>> {
        geometry::region(&self.tree, probe, self.srid?)
    }
    /// Of a geometry index, its tree, whose keys are envelopes.
    fn envelopes(&self) -> Option<Envelopes> {
        Some(Envelopes::new(self.tree.clone(), self.srid?))
    }
    fn insert(&self, key: Key, row: RowId) -> Result<()> {
        self.tree.insert(self.bounds(&key)?, row)
    }
    fn insert_alone(&self, key: Key, row: RowId) -> Result<bool> {
        self.insert(key, row)?;
        // Uniqueness is checked by the coordinator. A fallible search
        // after a successful insertion would break failure atomicity.
        Ok(false)
    }
    /// Packed bottom-up (`RTree::load`, RULES.md D139).
    fn load(&self, entries: &mut dyn Iterator<Item = Result<(Key, RowId)>>, work: (&WorkAreas, Area)) -> Result<()> {
        self.load_beside_writers(entries, work)
    }
    /// Packed beside the writers' entries, which the packed tree takes in
    /// before it takes the old one's place.
    fn load_beside_writers(&self, entries: &mut dyn Iterator<Item = Result<(Key, RowId)>>, work: (&WorkAreas, Area)) -> Result<()> {
        let mut bounded = entries.map(|entry| entry.and_then(|(key, row)| Ok((self.bounds(&key)?, row))));
        self.tree.load(&mut bounded, work)
    }
    fn remove(&self, key: &Key, row: RowId) -> Result<()> {
        self.tree.remove(self.bounds(key)?, row)
    }
    fn rows(&self, key: &Key) -> Result<Vec<RowId>> {
        self.equality(key)?.map(|entry| entry.map(|e| e.1)).collect()
    }
    fn key_rows(self: Arc<Self>, key: &Key) -> RowIds {
        let found = match self.equality(key) {
            Ok(found) => found,
            Err(error) => return Box::new(std::iter::once(Err(error))),
        };
        Box::new(found.map(move |entry| {
            let _held = &self;
            entry.map(|e| e.1)
        }))
    }
    fn search(self: Arc<Self>, probe: &Probe, _order: KeyOrder) -> Entries {
        if self.srid.is_none() && matches!(probe, Probe::Predicate { .. }) {
            return Box::new(std::iter::once(Err(unsupported("does not support function lookups"))));
        }
        let found = self.matching(probe);
        Box::new(found.inspect(move |_| {
            let _held = &self;
        }))
    }
    fn estimate(&self, probe: &Probe) -> Option<usize> {
        if let Some(srid) = self.srid {
            return geometry::estimate(&self.tree, probe, srid);
        }
        if matches!(probe, Probe::All | Probe::Predicate { .. }) {
            return None;
        }
        let Some(window) = probe::window(probe, self.width) else { return Some(0) };
        let mut count = 0_usize;
        for (at, entry) in self.tree.scan(Some(window)).enumerate() {
            if at >= MOST_COUNTED {
                return None;
            }
            let (bounds, _) = entry.ok()?;
            count = count.saturating_add(usize::from(probe::answers(probe, bounds, self.width)));
        }
        Some(count)
    }
    fn locator(&self) -> Vec<u8> {
        let version = if self.srid.is_some() { GEOMETRY_LOCATOR_VERSION } else { LOCATOR_VERSION };
        let mut bytes = vec![version, u8::try_from(self.width).unwrap_or_default()];
        if let Some(srid) = self.srid {
            bytes.put_u32(u32::try_from(srid).unwrap_or_default());
        }
        bytes.put_u64(self.tree.meta().0);
        bytes
    }
    fn discard(&self) {
        self.discarded.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests;

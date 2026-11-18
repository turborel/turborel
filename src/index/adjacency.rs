//! \[EXTENSION\] The built-in adjacency index, "adjacency" (RULES.md D34,
//! D107): the edges of a graph's edge table by endpoint, its
//! key the source's columns, the destination's, and properties included
//! for covering reads (`options.rs`). It is one B-link tree
//! (`engine/tree.rs`) in a pager file of a buffer pool, as "btree" keeps
//! its own, of forward entries by source, reverse ones by destination,
//! and each vertex's degree (`keys.rs`), written and taken back by
//! `write.rs`. Path search follows edges through `Adjacency`
//! (`graph/adjacency.rs`), and the planner finds an equality of the
//! source's columns in it as in any index (`read.rs`).
//!
//! Maintenance compacts the tree into an adjacency in memory (`csr.rs`,
//! `maintain.rs`), as a load does (`load.rs`), and readers merge it with
//! what was written since (`delta.rs`), so every read gives exactly the
//! tree's entries. Durability is the tree's: its pages are synced at a
//! checkpoint, and recovery mends or rebuilds it as every index (D25).

mod csr;
mod delta;
mod keys;
mod load;
mod maintain;
mod options;
mod read;
mod write;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

pub use options::{BOTH, CSR_BYTES_OPTION, DEFAULT_CSR_BYTES, DIRECTION_OPTION, IN, INCLUDE_OPTION, OUT, SOURCE_COLUMNS_OPTION};

use super::stats::KeyCounts;
use super::{Entries, Index, IndexMethod, Key, KeyOrder, Probe, RowIds};
use crate::catalog::{IndexDesc, TableDesc};
use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::io::Durability;
use crate::disk::pager::PageId;
use crate::engine::tree::BLinkTree;
use crate::engine::{MEMORY_POOL_BYTES, MemoryPages};
use crate::error::{Code, Result, SqlError};
use crate::graph::adjacency::{Adjacency, Direction, Step};
use crate::memory::{Host, MemoryBudget};
use crate::spill::{Area, WorkAreas};
use crate::storage::{RowId, TableOption};
use crate::sync::lock_builtin;
use crate::value::{CompOp, DataType};
use delta::Delta;
use options::{Kept, Layout, csr_bytes_of, layout_of, located, locator};

/// The name of `AdjacencyMethod`.
pub const ADJACENCY_METHOD: &str = "adjacency";

/// \[EXTENSION\] The built-in adjacency index method: adjacency indexes in
/// B-link trees in one pager file.
pub struct AdjacencyMethod {
    /// Where its indexes are; for a method in memory, made when the first
    /// index is.
    files: Mutex<Option<(Arc<BufferPool>, FileId)>>,
    /// Its pages in memory, for a file made in memory.
    memory: Arc<MemoryPages>,
    /// The budget its indexes' compacted adjacencies are held of: the
    /// database's once attached; none until then, when they have none.
    budget: Mutex<Option<Arc<MemoryBudget>>>,
}

impl AdjacencyMethod {
    /// A method keeping its indexes in pager file `file` of `pool`.
    pub fn new(pool: Arc<BufferPool>, file: FileId) -> AdjacencyMethod {
        AdjacencyMethod { files: Mutex::new(Some((pool, file))), memory: MemoryPages::new(0), budget: Mutex::new(None) }
    }

    /// A method keeping its indexes in memory, in a pool of its own made
    /// with its first index, of the budget of the database it is attached
    /// to.
    pub fn in_memory() -> AdjacencyMethod {
        AdjacencyMethod::sharing(MemoryPages::new(MEMORY_POOL_BYTES))
    }

    /// A method keeping its indexes in `memory`, which others may share.
    pub(crate) fn sharing(memory: Arc<MemoryPages>) -> AdjacencyMethod {
        AdjacencyMethod { files: Mutex::new(None), memory, budget: Mutex::new(None) }
    }

    /// The method, its indexes' compacted adjacencies held of `budget`, as
    /// a method not attached to a database's has none.
    #[must_use]
    pub fn with_budget(self, budget: Arc<MemoryBudget>) -> AdjacencyMethod {
        *lock_builtin(&self.budget) = Some(budget);
        self
    }

    /// Its file, made in memory if it has none yet.
    fn files(&self) -> Result<(Arc<BufferPool>, FileId)> {
        let mut files = lock_builtin(&self.files);
        if let Some((pool, file)) = files.as_ref() {
            return Ok((Arc::clone(pool), *file));
        }
        let (pool, file) = self.memory.file("the pool in memory of the built-in engine and index methods")?;
        *files = Some((Arc::clone(&pool), file));
        Ok((pool, file))
    }

    /// An index of `layout` in `tree`, its key counts `keys`, its
    /// compacted adjacency of at most `most` bytes.
    fn index(&self, tree: BLinkTree, layout: Layout, counted: (KeyCounts, usize)) -> Box<dyn Index> {
        Box::new(self.index_of(tree, layout, counted))
    }

    /// `index`, unboxed.
    fn index_of(&self, tree: BLinkTree, layout: Layout, (keys, most): (KeyCounts, usize)) -> AdjacencyIndex {
        let budget = lock_builtin(&self.budget).clone();
        AdjacencyIndex {
            tree,
            layout: Arc::new(layout),
            keys,
            delta: Delta::new(),
            budget,
            most: AtomicUsize::new(most),
            refused: Mutex::new(None),
            building: Mutex::new(()),
            discarded: AtomicBool::new(false),
        }
    }
}

/// The error of what the method does not do.
fn unsupported(detail: &str) -> SqlError {
    SqlError::new(Code::Unsupported, format!("[EXTENSION] the adjacency index method {detail}"))
}

/// Refuses the index `on` describes, of a table, unless the method keeps
/// it: of no null keys, nor sort keys, for a step gives the vertex's own
/// key; its included columns NOT NULL, as a key with a null has no entry
/// and an edge must have one.
///
/// # Errors
///
/// SQLCODE -900 if it is refused.
fn admitted((table, desc): (&TableDesc, &IndexDesc), layout: &Layout) -> Result<()> {
    if desc.nulls {
        return Err(unsupported("does not store NULLS NOT DISTINCT keys"));
    }
    if desc.is_collated() {
        return Err(unsupported(
            "gives each vertex's own key, and a column with a collation is indexed by its sort key; index a column without a COLLATE clause",
        ));
    }
    let included = desc.columns.get(layout.endpoints()..).unwrap_or_default();
    if included.iter().any(|&c| table.columns.get(c).is_none_or(|c| !c.not_null)) {
        return Err(unsupported("includes NOT NULL columns alone, as a row with a null in its key has no entry"));
    }
    Ok(())
}

impl IndexMethod for AdjacencyMethod {
    fn name(&self) -> &'static str {
        ADJACENCY_METHOD
    }
    fn attach(&self, host: &Host) -> Result<()> {
        self.memory.use_budget(Arc::clone(&host.memory));
        *lock_builtin(&self.budget) = Some(Arc::clone(&host.memory));
        Ok(())
    }
    /// Equality of the first column alone.
    fn supports(&self, op: CompOp) -> bool {
        op == CompOp::Eq
    }
    /// Equal values of one or more first columns, the next compared equal too.
    fn supports_prefix(&self, columns: usize, equal: usize, then: Option<CompOp>) -> bool {
        equal > 0 && then.is_none_or(|op| op == CompOp::Eq) && equal.saturating_add(usize::from(then.is_some())) <= columns
    }
    fn restores_keys(&self) -> bool {
        true
    }
    fn index_options(&self) -> Vec<TableOption> {
        options::options()
    }
    /// One column of the source, the rest the destination's, kept both ways.
    fn create(&self, columns: &[DataType]) -> Result<Box<dyn Index>> {
        let layout = Layout::new(columns, (1, 0), Kept::Both)?;
        let (pool, file) = self.files()?;
        Ok(self.index(BLinkTree::create(pool, file)?, layout, (KeyCounts::new(), DEFAULT_CSR_BYTES)))
    }
    fn create_on(&self, columns: &[DataType], on: (&TableDesc, &IndexDesc)) -> Result<Box<dyn Index>> {
        let layout = layout_of(columns, on.1)?;
        admitted(on, &layout)?;
        let (pool, file) = self.files()?;
        Ok(self.index(BLinkTree::create(pool, file)?, layout, (KeyCounts::new(), csr_bytes_of(on.1))))
    }
    fn open(&self, columns: &[DataType], locator: &[u8]) -> Result<Option<Box<dyn Index>>> {
        let (layout, counts) = located(locator, columns)?;
        let (meta, keys) = KeyCounts::located(counts)?;
        let (pool, file) = self.files()?;
        Ok(Some(self.index(BLinkTree::open(pool, file, PageId(meta))?, layout, (keys, DEFAULT_CSR_BYTES))))
    }
    fn open_on(&self, columns: &[DataType], locator: &[u8], on: (&TableDesc, &IndexDesc)) -> Result<Option<Box<dyn Index>>> {
        let (layout, counts) = located(locator, columns)?;
        let (meta, keys) = KeyCounts::located(counts)?;
        let (pool, file) = self.files()?;
        Ok(Some(self.index(BLinkTree::open(pool, file, PageId(meta))?, layout, (keys, csr_bytes_of(on.1)))))
    }
    /// Writes back every page and syncs the file.
    fn checkpoint(&self) -> Result<()> {
        let (pool, file) = self.files()?;
        pool.sync(file, Durability::Full)
    }
}

/// \[EXTENSION\] An adjacency index: its tree, its layout, its compacted
/// adjacency and delta, and whether it was dropped, so its pages are
/// freed when it goes.
pub struct AdjacencyIndex {
    tree: BLinkTree,
    layout: Arc<Layout>,
    /// Its distinct keys and live entries (`index/stats.rs`).
    keys: KeyCounts,
    delta: Delta,
    /// The budget its compacted adjacency is held of, if any.
    budget: Option<Arc<MemoryBudget>>,
    /// The most bytes its compacted adjacency holds (ADJACENCY.CSR_BYTES).
    most: AtomicUsize,
    /// The live entries when a build last did not fit, so the next waits
    /// for twice as many.
    refused: Mutex<Option<u64>>,
    /// Held by the build under way, so two never run at once.
    building: Mutex<()>,
    discarded: AtomicBool,
}

/// A dropped index's pages are freed when the last reference to it goes,
/// so no reader of an earlier snapshot reads a page given to another use.
/// A failure cannot be reported from here: the pages are then lost to the
/// file until it is rebuilt, which costs space only.
impl Drop for AdjacencyIndex {
    fn drop(&mut self) {
        if self.discarded.load(Ordering::Acquire) {
            let _ = self.tree.free();
        }
    }
}

impl Index for AdjacencyIndex {
    fn adjacency(self: Arc<Self>) -> Option<Arc<dyn Adjacency>> {
        Some(self)
    }
    fn insert(&self, key: Key, row: RowId) -> Result<()> {
        self.add(&key, row)
    }
    /// Not alone, never asked: no UNIQUE constraint is checked by it.
    fn insert_alone(&self, key: Key, row: RowId) -> Result<bool> {
        self.add(&key, row)?;
        Ok(false)
    }
    fn load(&self, entries: &mut dyn Iterator<Item = Result<(Key, RowId)>>, work: (&WorkAreas, Area)) -> Result<()> {
        self.load_whole(entries, work)
    }
    fn remove(&self, key: &Key, row: RowId) -> Result<()> {
        self.take_back(key, row)
    }
    fn rows(&self, key: &Key) -> Result<Vec<RowId>> {
        read::rows_of(&self.tree, &self.layout, key)?.collect()
    }
    fn key_rows(self: Arc<Self>, key: &Key) -> RowIds {
        match read::rows_of(read::Held(Arc::clone(&self)), &self.layout, key) {
            Ok(rows) => Box::new(rows),
            Err(e) => Box::new(std::iter::once(Err(e))),
        }
    }
    fn estimate(&self, probe: &Probe) -> Option<usize> {
        self.estimated(probe)
    }
    fn distinct_keys(&self) -> Option<f64> {
        self.keys.distinct()
    }
    fn maintenance_due(&self) -> bool {
        self.keys.due() || self.compaction_due()
    }
    fn maintain(&self) -> Result<bool> {
        self.maintained()
    }
    /// A descent, its levels the tree's.
    fn probe_pages(&self, _probe: &Probe) -> Option<f64> {
        self.tree.height().ok().map(f64::from)
    }
    fn search(self: Arc<Self>, probe: &Probe, _order: KeyOrder) -> Entries {
        read::search(&self, probe)
    }
    /// Its layout, its tree's meta page and its key counts.
    fn locator(&self) -> Vec<u8> {
        locator(&self.layout, &self.keys.locator(self.tree.meta().0))
    }
    fn discard(&self) {
        self.discarded.store(true, Ordering::Release);
    }
    /// ADJACENCY.CSR_BYTES changed: a compacted adjacency past it is dropped.
    fn reconfigure(&self, on: (&TableDesc, &IndexDesc)) {
        let most = csr_bytes_of(on.1);
        self.most.store(most, Ordering::Release);
        let past = self.delta.current().ok().flatten().is_some_and(|csr| csr.bytes() > most);
        if past {
            // A poisoned lock leaves the adjacency, which stays exact.
            let _ = self.delta.drop_csr();
        }
    }
}

impl Adjacency for AdjacencyIndex {
    fn steps(&self, vertex: &Key, direction: Direction, out: &mut Vec<Step>) -> Result<()> {
        self.steps_of(vertex, direction, out)
    }
    fn degree(&self, vertex: &Key, direction: Direction) -> Result<u64> {
        self.degree_of(vertex, direction)
    }
    fn edges(&self) -> u64 {
        self.keys.live()
    }
}

// Unit tests: entries counted by version, steps against a scan of every
// edge, the compacted adjacency and its delta against the tree, loads,
// budgets, locators and refusals (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;

//! \[EXTENSION\] The built-in reachability index, "reachability"
//! (RULES.md D34, D150): which vertices an edge table's edges lead to,
//! by pruned landmark labelling (`landmarks.rs`), so a GRAPH_TABLE's
//! search drops the starts that reach no vertex its end could bind
//! (`exec/graph_table/reach.rs`). Its key is the source's columns, then
//! the destination's, of one type each (REACHABILITY.SOURCE_COLUMNS
//! splits them), so a vertex's key is the same bytes at either end.
//!
//! Its entries are kept in one B-link tree in a pager file, as
//! "adjacency" keeps its own: an edge's source, destination and row,
//! counting the versions holding it (`counted.rs`). The labels are held
//! in memory, of the memory budget (D38): made of the tree when first
//! read, kept exact as edges are added, and dropped when the last version
//! of an edge is taken back, as they would then claim a path that may be
//! gone; maintenance makes them again. Until then the index answers
//! nothing, and searches are not pruned. Durability is the tree's: its
//! pages are synced at a checkpoint, and recovery mends or rebuilds it
//! as every index (D25).

mod landmarks;
mod state;

use std::ops::{Bound, Deref};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::counted::{self, Files};
use super::{Entries, Index, IndexMethod, Key, KeyOrder, Probe};
use crate::catalog::{IndexDesc, TableDesc};
use crate::disk::buffer::{BufferPool, FileId};
use crate::engine::key::{decode_front, encode_key};
use crate::engine::tree::{BLinkTree, Direction, scan_of};
use crate::engine::{MEMORY_POOL_BYTES, MemoryPages};
use crate::error::{Code, Result, SqlError};
use crate::graph::indexes::Reachability;
use crate::memory::{Host, MemoryBudget};
use crate::settings::Kind;
use crate::storage::{Rebuild, RowId, TableOption, option_value};
use crate::sync::lock_builtin;
use crate::value::{CompOp, DataType, Decimal, Value};
use state::Labels;

/// The name of `ReachabilityMethod`.
pub const REACHABILITY_METHOD: &str = "reachability";
/// The option of how many of the key's first columns are the source.
pub const SOURCE_COLUMNS_OPTION: &str = "REACHABILITY.SOURCE_COLUMNS";
/// The most columns the source may have, as the locator keeps them.
const MOST_COLUMNS: i128 = 1 << 16;
/// Bytes of a row id.
const ROW_BYTES: usize = 8;
/// What a locator of it is read as.
const LOCATOR: &str = "reachability index locator";

/// \[EXTENSION\] The built-in reachability index method: reachability
/// indexes in B-link trees in one pager file.
pub struct ReachabilityMethod {
    files: Files,
    /// The budget its indexes' labels are held of, once attached.
    budget: Mutex<Option<Arc<MemoryBudget>>>,
}

impl ReachabilityMethod {
    /// A method keeping its indexes in pager file `file` of `pool`.
    pub fn new(pool: Arc<BufferPool>, file: FileId) -> ReachabilityMethod {
        ReachabilityMethod { files: Files::new(pool, file), budget: Mutex::new(None) }
    }

    /// A method keeping its indexes in a pool of its own in memory.
    pub fn in_memory() -> ReachabilityMethod {
        ReachabilityMethod::sharing(MemoryPages::new(MEMORY_POOL_BYTES))
    }

    /// A method keeping its indexes in `memory`, which others may share.
    pub(crate) fn sharing(memory: Arc<MemoryPages>) -> ReachabilityMethod {
        ReachabilityMethod { files: Files::sharing(memory), budget: Mutex::new(None) }
    }

    /// The method, its labels held of `budget`.
    #[must_use]
    pub fn with_budget(self, budget: Arc<MemoryBudget>) -> ReachabilityMethod {
        *lock_builtin(&self.budget) = Some(budget);
        self
    }

    /// An index of key `types`, `source` of them the source's, in `tree`.
    fn index(&self, tree: BLinkTree, types: &[DataType], source: usize) -> Result<Box<dyn Index>> {
        split(types, source)?;
        let budget = lock_builtin(&self.budget).clone();
        Ok(Box::new(ReachabilityIndex {
            tree,
            types: types.to_vec(),
            source,
            labels: Mutex::new(Labels::new(budget)),
            discarded: AtomicBool::new(false),
        }))
    }
}

/// The error of what the method does not do.
fn unsupported(detail: &str) -> SqlError {
    SqlError::new(Code::Unsupported, format!("[EXTENSION] the reachability index method {detail}"))
}

/// Refuses a key of `types` whose first `source` are not the source's
/// columns, of the destination's types in order.
///
/// # Errors
///
/// SQLCODE -900 if it is refused.
fn split(types: &[DataType], source: usize) -> Result<()> {
    let (from, to) = types.split_at_checked(source).unwrap_or((types, &[]));
    if source == 0 || from != to {
        return Err(unsupported(
            "keys an edge by its source's columns, then its destination's, as many and of the same types, so a vertex is one key at either end",
        ));
    }
    Ok(())
}

/// The source's columns `desc` gives a key of `columns` columns: half, if
/// it names none.
fn source_of(desc: &IndexDesc, columns: usize) -> usize {
    match option_value(&options(), &desc.options, SOURCE_COLUMNS_OPTION) {
        Some(Value::Exact(d)) => d.to_string().parse::<usize>().unwrap_or(0),
        Some(_) | None => columns.checked_div(2).unwrap_or(0),
    }
}

/// The options its indexes take (RULES.md D41).
fn options() -> Vec<TableOption> {
    vec![TableOption::new(
        (SOURCE_COLUMNS_OPTION, Kind::Integer { min: 1, max: MOST_COLUMNS }, Value::Exact(Decimal::new(1, 0))),
        Rebuild::Yes,
        "How many of the key's first columns are the edge's source; the destination's, as many, follow.",
    )]
}

impl IndexMethod for ReachabilityMethod {
    fn name(&self) -> &'static str {
        REACHABILITY_METHOD
    }
    fn attach(&self, host: &Host) -> Result<()> {
        self.files.attach(host);
        *lock_builtin(&self.budget) = Some(Arc::clone(&host.memory));
        Ok(())
    }
    /// No comparison: it answers reachability alone.
    fn supports(&self, _op: CompOp) -> bool {
        false
    }
    fn index_options(&self) -> Vec<TableOption> {
        options()
    }
    fn create(&self, columns: &[DataType]) -> Result<Box<dyn Index>> {
        self.index(self.files.tree()?, columns, columns.len().checked_div(2).unwrap_or(0))
    }
    /// Of no NULLS NOT DISTINCT keys, nor sort keys, as a vertex is its key's own bytes.
    fn create_on(&self, columns: &[DataType], (_, desc): (&TableDesc, &IndexDesc)) -> Result<Box<dyn Index>> {
        if desc.nulls || desc.is_collated() || !desc.expressions.is_empty() {
            return Err(unsupported("keys an edge by its ends' own columns, of no collation, and stores no NULLS NOT DISTINCT keys"));
        }
        self.index(self.files.tree()?, columns, source_of(desc, columns.len()))
    }
    fn open(&self, columns: &[DataType], locator: &[u8]) -> Result<Option<Box<dyn Index>>> {
        let (source, meta) = counted::located(locator, LOCATOR)?;
        let source = usize::try_from(source).unwrap_or(usize::MAX);
        Ok(Some(self.index(self.files.opened(meta)?, columns, source)?))
    }
    fn checkpoint(&self) -> Result<()> {
        self.files.checkpoint()
    }
}

/// \[EXTENSION\] A reachability index: its tree, its key's types and
/// split, its labels, and whether it was dropped, so its pages are freed
/// when it goes.
pub struct ReachabilityIndex {
    tree: BLinkTree,
    types: Vec<DataType>,
    source: usize,
    /// Held while an entry is written, so the tree and the labels change
    /// together.
    labels: Mutex<Labels>,
    discarded: AtomicBool,
}

/// A dropped index's pages are freed when the last reference to it goes.
/// A failure cannot be reported from here: the pages are then lost to the
/// file until it is rebuilt, which costs space only.
impl Drop for ReachabilityIndex {
    fn drop(&mut self) {
        if self.discarded.load(Ordering::Acquire) {
            let _ = self.tree.free();
        }
    }
}

/// The tree of an index a scan holds.
struct Held(Arc<ReachabilityIndex>);

impl Deref for Held {
    type Target = BLinkTree;
    fn deref(&self) -> &BLinkTree {
        &self.0.tree
    }
}

/// An entry's key read back: the source's and destination's bytes and values, and the row.
struct Parsed<'a> {
    source: &'a [u8],
    destination: &'a [u8],
    values: Vec<Value>,
    row: RowId,
}

impl ReachabilityIndex {
    /// The types of a vertex's key.
    fn vertex_types(&self) -> &[DataType] {
        self.types.get(..self.source).unwrap_or_default()
    }

    /// The bytes of a vertex of key `values`.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if they are not of the source's types.
    fn vertex(&self, values: &[Value]) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        encode_key(values, self.vertex_types(), &mut out)?;
        Ok(out)
    }

    /// The key of the entry of `row` under `key`.
    fn entry(&self, key: &Key, row: RowId) -> Result<Vec<u8>> {
        let (from, to) = key.0.split_at_checked(self.source).unwrap_or((&key.0, &[]));
        let mut out = self.vertex(from)?;
        out.extend(self.vertex(to)?);
        out.extend_from_slice(&row.to_be_bytes());
        Ok(out)
    }

    /// The parts of entry key `key`.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if it is not of the layout.
    fn parse<'a>(&self, key: &'a [u8]) -> Result<Parsed<'a>> {
        let mut rest = key;
        let mut values = decode_front(&mut rest, self.vertex_types())?;
        let source = key.get(..key.len().saturating_sub(rest.len())).unwrap_or_default();
        let after = rest;
        values.extend(decode_front(&mut rest, self.vertex_types())?);
        let destination = after.get(..after.len().saturating_sub(rest.len())).unwrap_or_default();
        let bytes: [u8; ROW_BYTES] = rest.try_into().map_err(|_| counted::corrupt("a reachability"))?;
        Ok(Parsed { source, destination, values, row: RowId::from_be_bytes(bytes) })
    }

    /// Every edge's source and destination bytes.
    ///
    /// # Errors
    ///
    /// The tree's.
    fn edges(&self) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let mut out = Vec::new();
        for entry in self.tree.scan(Bound::Unbounded, Bound::Unbounded, Direction::Forward) {
            let (key, _) = entry?;
            let parsed = self.parse(&key)?;
            out.push((parsed.source.to_vec(), parsed.destination.to_vec()));
        }
        Ok(out)
    }
}

impl Index for ReachabilityIndex {
    fn reachability(self: Arc<Self>) -> Option<Arc<dyn Reachability>> {
        Some(self)
    }
    fn insert(&self, key: Key, row: RowId) -> Result<()> {
        let entry = self.entry(&key, row)?;
        let mut labels = lock_builtin(&self.labels);
        counted::add(&self.tree, &entry)?;
        let parsed = self.parse(&entry)?;
        labels.added(parsed.source, parsed.destination);
        Ok(())
    }
    /// Not alone, never asked: no UNIQUE constraint is checked by it.
    fn insert_alone(&self, key: Key, row: RowId) -> Result<bool> {
        self.insert(key, row)?;
        Ok(false)
    }
    fn remove(&self, key: &Key, row: RowId) -> Result<()> {
        let entry = self.entry(key, row)?;
        let mut labels = lock_builtin(&self.labels);
        if counted::take(&self.tree, &entry)?.1 {
            labels.removed();
        }
        Ok(())
    }
    fn rows(&self, key: &Key) -> Result<Vec<RowId>> {
        let mut out = Vec::new();
        for entry in self.tree.scan(Bound::Unbounded, Bound::Unbounded, Direction::Forward) {
            let (bytes, _) = entry?;
            let parsed = self.parse(&bytes)?;
            if self.entry(key, parsed.row)? == bytes && !out.contains(&parsed.row) {
                out.push(parsed.row);
            }
        }
        Ok(out)
    }
    fn estimate(&self, _probe: &Probe) -> Option<usize> {
        None
    }
    fn maintenance_due(&self) -> bool {
        lock_builtin(&self.labels).due()
    }
    fn maintain(&self) -> Result<bool> {
        let mut labels = lock_builtin(&self.labels);
        if !labels.due() {
            return Ok(false);
        }
        labels.build(&self.edges()?);
        Ok(true)
    }
    /// Of `Probe::All`, every entry, by source; no other probe is asked of it.
    fn search(self: Arc<Self>, probe: &Probe, _order: KeyOrder) -> Entries {
        if !matches!(probe, Probe::All) {
            return Box::new(std::iter::empty());
        }
        let index = Arc::clone(&self);
        let scan = scan_of(Held(self), Bound::Unbounded, Bound::Unbounded, Direction::Forward);
        Box::new(scan.map(move |entry| {
            let (key, _) = entry?;
            let parsed = index.parse(&key)?;
            Ok((Key(parsed.values), parsed.row))
        }))
    }
    /// Its source's columns and its tree's meta page.
    fn locator(&self) -> Vec<u8> {
        counted::locator(u32::try_from(self.source).unwrap_or(u32::MAX), &self.tree)
    }
    fn discard(&self) {
        self.discarded.store(true, Ordering::Release);
    }
}

impl Reachability for ReachabilityIndex {
    fn reaching(&self, starts: &[Key], ends: &[Key], reverse: bool) -> Result<Option<Vec<bool>>> {
        // A vertex whose key is not of the index's types is one no edge
        // leads to by bytes; a start of one is kept, an end of one stops
        // the pruning, as either may still be joined by value.
        let starts: Vec<Option<Vec<u8>>> = starts.iter().map(|k| self.vertex(&k.0).ok()).collect();
        let Ok(ends) = ends.iter().map(|k| self.vertex(&k.0)).collect::<Result<Vec<_>>>() else { return Ok(None) };
        let mut labels = lock_builtin(&self.labels);
        if labels.unmade() {
            labels.build(&self.edges()?);
        }
        Ok(labels.answer(|l| l.reaching(&starts, &ends, reverse)))
    }
}

// Unit tests: labels against a search of every edge, kept exact as edges
// are added, dropped as one is taken back and made again by maintenance,
// searches, locators and refusals (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;

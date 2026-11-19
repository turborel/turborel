//! \[EXTENSION\] The built-in label index, "labels" (RULES.md D34, D115):
//! the rows of a property graph's element table, each numbered densely,
//! so a GRAPH_TABLE tests a reached element's labels by its number and
//! finds the start candidates of a label expression in bitmaps, without
//! reading the rows of tables that cannot match nor any row's columns
//! (`exec/graph_table/labels.rs`). Its key is the element table's key,
//! each of NOT NULL columns, so every row has an entry.
//!
//! Its entries are kept in one B-link tree in a pager file, as
//! "adjacency" keeps its own: a row's number, then its key, counting the
//! versions holding it (`counted.rs`). The numbering is held in memory
//! (`numbered.rs`), made of the tree when it is first read, kept as rows
//! are written, and made again by maintenance once an eighth of its
//! numbers are of rows gone, so they stay dense. Durability is the tree's:
//! its pages are synced at a checkpoint, and recovery mends or rebuilds
//! it as every index (D25); the numbering is made again of it.

mod numbered;

use std::ops::{Bound, Deref};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::counted::{self, Files};
use super::{Entries, Index, IndexMethod, Key, KeyOrder, Probe};
use crate::catalog::{IndexDesc, TableDesc};
use crate::disk::buffer::{BufferPool, FileId};
use crate::engine::tree::{BLinkTree, Direction, scan_of};
use crate::engine::{MEMORY_POOL_BYTES, MemoryPages};
use crate::error::{Code, Result, SqlError};
use crate::graph::indexes::{MemberSet, Members};
use crate::memory::{Host, MemoryBudget};
use crate::storage::RowId;
use crate::sync::lock_builtin;
use crate::value::{CompOp, DataType};
use numbered::Numbered;

/// The name of `LabelsMethod`.
pub const LABELS_METHOD: &str = "labels";
/// Bytes of a row id.
const ROW_BYTES: usize = 8;
/// What a locator of it is read as.
const LOCATOR: &str = "label index locator";

/// \[EXTENSION\] The built-in label index method: label indexes in B-link
/// trees in one pager file.
pub struct LabelsMethod {
    files: Files,
    /// The budget its indexes' numberings are held of, once attached.
    budget: Mutex<Option<Arc<MemoryBudget>>>,
}

impl LabelsMethod {
    /// A method keeping its indexes in pager file `file` of `pool`.
    pub fn new(pool: Arc<BufferPool>, file: FileId) -> LabelsMethod {
        LabelsMethod { files: Files::new(pool, file), budget: Mutex::new(None) }
    }

    /// A method keeping its indexes in a pool of its own in memory.
    pub fn in_memory() -> LabelsMethod {
        LabelsMethod::sharing(MemoryPages::new(MEMORY_POOL_BYTES))
    }

    /// A method keeping its indexes in `memory`, which others may share.
    pub(crate) fn sharing(memory: Arc<MemoryPages>) -> LabelsMethod {
        LabelsMethod { files: Files::sharing(memory), budget: Mutex::new(None) }
    }

    /// The method, its numberings held of `budget`.
    #[must_use]
    pub fn with_budget(self, budget: Arc<MemoryBudget>) -> LabelsMethod {
        *lock_builtin(&self.budget) = Some(budget);
        self
    }

    /// An index of key `types` in `tree`.
    fn index(&self, tree: BLinkTree, types: &[DataType]) -> Box<dyn Index> {
        let budget = lock_builtin(&self.budget).clone();
        Box::new(LabelsIndex {
            tree,
            types: types.to_vec(),
            numbered: Mutex::new(Numbered::new(budget)),
            discarded: AtomicBool::new(false),
        })
    }
}

/// The error of what the method does not do.
fn unsupported(detail: &str) -> SqlError {
    SqlError::new(Code::Unsupported, format!("[EXTENSION] the labels index method {detail}"))
}

/// Refuses the index `on` describes unless every row has an entry: of no
/// NULLS NOT DISTINCT keys, and every key column NOT NULL, as a key with a
/// null has no entry.
///
/// # Errors
///
/// SQLCODE -900 if it is refused.
fn admitted((table, desc): (&TableDesc, &IndexDesc)) -> Result<()> {
    if desc.nulls {
        return Err(unsupported("does not store NULLS NOT DISTINCT keys"));
    }
    if !desc.expressions.is_empty() || desc.columns.iter().any(|&c| table.columns.get(c).is_none_or(|c| !c.not_null)) {
        return Err(unsupported("keys a row by NOT NULL columns alone, as a row with a null in its key has no entry"));
    }
    Ok(())
}

impl IndexMethod for LabelsMethod {
    fn name(&self) -> &'static str {
        LABELS_METHOD
    }
    fn attach(&self, host: &Host) -> Result<()> {
        self.files.attach(host);
        *lock_builtin(&self.budget) = Some(Arc::clone(&host.memory));
        Ok(())
    }
    /// No comparison: it answers label tests alone.
    fn supports(&self, _op: CompOp) -> bool {
        false
    }
    fn create(&self, columns: &[DataType]) -> Result<Box<dyn Index>> {
        Ok(self.index(self.files.tree()?, columns))
    }
    fn create_on(&self, columns: &[DataType], on: (&TableDesc, &IndexDesc)) -> Result<Box<dyn Index>> {
        admitted(on)?;
        self.create(columns)
    }
    fn open(&self, columns: &[DataType], locator: &[u8]) -> Result<Option<Box<dyn Index>>> {
        let (_, meta) = counted::located(locator, LOCATOR)?;
        Ok(Some(self.index(self.files.opened(meta)?, columns)))
    }
    fn checkpoint(&self) -> Result<()> {
        self.files.checkpoint()
    }
}

/// \[EXTENSION\] A label index: its tree, its key's types, the numbering
/// of its rows, and whether it was dropped, so its pages are freed when
/// it goes.
pub struct LabelsIndex {
    tree: BLinkTree,
    types: Vec<DataType>,
    /// Held while an entry is written, so the tree and the numbering
    /// change together.
    numbered: Mutex<Numbered>,
    discarded: AtomicBool,
}

/// A dropped index's pages are freed when the last reference to it goes.
/// A failure cannot be reported from here: the pages are then lost to the
/// file until it is rebuilt, which costs space only.
impl Drop for LabelsIndex {
    fn drop(&mut self) {
        if self.discarded.load(Ordering::Acquire) {
            let _ = self.tree.free();
        }
    }
}

/// The tree of an index a scan holds.
struct Held(Arc<LabelsIndex>);

impl Deref for Held {
    type Target = BLinkTree;
    fn deref(&self) -> &BLinkTree {
        &self.0.tree
    }
}

/// The row an entry's key begins with.
///
/// # Errors
///
/// SQLCODE -916 if it has none.
fn row_of(entry: &[u8]) -> Result<RowId> {
    let bytes: [u8; ROW_BYTES] = entry.get(..ROW_BYTES).and_then(|b| b.try_into().ok()).ok_or_else(|| counted::corrupt("a labels"))?;
    Ok(RowId::from_be_bytes(bytes))
}

impl LabelsIndex {
    /// The key of the entry of `row` under `key`.
    fn entry(&self, key: &Key, row: RowId) -> Result<Vec<u8>> {
        let mut out = row.to_be_bytes().to_vec();
        crate::engine::key::encode_key(&key.0, &self.types, &mut out)?;
        Ok(out)
    }

    /// Every row with an entry and the versions holding its entries, in
    /// the order of the rows.
    ///
    /// # Errors
    ///
    /// The tree's.
    fn counts(&self) -> Result<Vec<(RowId, u32)>> {
        let mut out: Vec<(RowId, u32)> = Vec::new();
        for entry in self.tree.scan(Bound::Unbounded, Bound::Unbounded, Direction::Forward) {
            let (key, value) = entry?;
            let (row, n) = (row_of(&key)?, counted::counted(&value)?);
            if let Some((_, total)) = out.last_mut().filter(|(last, _)| *last == row) {
                *total = total.saturating_add(n);
            } else {
                out.push((row, n));
            }
        }
        Ok(out)
    }
}

impl Index for LabelsIndex {
    fn members(self: Arc<Self>) -> Option<Arc<dyn Members>> {
        Some(self)
    }
    fn insert(&self, key: Key, row: RowId) -> Result<()> {
        let entry = self.entry(&key, row)?;
        let mut numbered = lock_builtin(&self.numbered);
        counted::add(&self.tree, &entry)?;
        numbered.added(row);
        Ok(())
    }
    /// Not alone, never asked: no UNIQUE constraint is checked by it.
    fn insert_alone(&self, key: Key, row: RowId) -> Result<bool> {
        self.insert(key, row)?;
        Ok(false)
    }
    fn remove(&self, key: &Key, row: RowId) -> Result<()> {
        let entry = self.entry(key, row)?;
        let mut numbered = lock_builtin(&self.numbered);
        if counted::take(&self.tree, &entry)?.0 {
            numbered.removed(row);
        }
        Ok(())
    }
    fn rows(&self, key: &Key) -> Result<Vec<RowId>> {
        let mut wanted = Vec::new();
        crate::engine::key::encode_key(&key.0, &self.types, &mut wanted)?;
        let mut out = Vec::new();
        for entry in self.tree.scan(Bound::Unbounded, Bound::Unbounded, Direction::Forward) {
            let (bytes, _) = entry?;
            let row = row_of(&bytes)?;
            if bytes.get(ROW_BYTES..) == Some(wanted.as_slice()) && !out.contains(&row) {
                out.push(row);
            }
        }
        Ok(out)
    }
    fn estimate(&self, _probe: &Probe) -> Option<usize> {
        None
    }
    fn maintenance_due(&self) -> bool {
        lock_builtin(&self.numbered).due()
    }
    fn maintain(&self) -> Result<bool> {
        let mut numbered = lock_builtin(&self.numbered);
        if !numbered.due() {
            return Ok(false);
        }
        numbered.build(self.counts()?);
        Ok(true)
    }
    /// Of `Probe::All`, every entry, in the order of the rows; no other
    /// probe is asked of it.
    fn search(self: Arc<Self>, probe: &Probe, _order: KeyOrder) -> Entries {
        if !matches!(probe, Probe::All) {
            return Box::new(std::iter::empty());
        }
        let types = self.types.clone();
        let scan = scan_of(Held(self), Bound::Unbounded, Bound::Unbounded, Direction::Forward);
        Box::new(scan.map(move |entry| {
            let (key, _) = entry?;
            let row = row_of(&key)?;
            let values = crate::engine::key::decode_key(key.get(ROW_BYTES..).unwrap_or_default(), &types)?;
            Ok((Key(values), row))
        }))
    }
    /// Its tree's meta page.
    fn locator(&self) -> Vec<u8> {
        counted::locator(0, &self.tree)
    }
    fn discard(&self) {
        self.discarded.store(true, Ordering::Release);
    }
}

impl Members for LabelsIndex {
    fn members(&self) -> Result<Option<Arc<MemberSet>>> {
        let mut numbered = lock_builtin(&self.numbered);
        if numbered.unmade() {
            numbered.build(self.counts()?);
        }
        Ok(numbered.snapshot())
    }
}

// Unit tests: rows numbered as written and taken back, the numbering made
// of the tree and made dense again, searches, locators and refusals
// (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;

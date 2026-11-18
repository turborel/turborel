//! \[EXTENSION\] The built-in json index, "json" (RULES.md D34): an
//! inverted index of the paths and values of the JSON documents of one
//! column, a character string's or the JSON type's, kept in one B-link tree (`engine/tree.rs`) in a
//! pager file of a buffer pool, as "btree" keeps its own. It finds the
//! rows of JSON_EXISTS of a member chain and its filters, of JSON_VALUE
//! of a chain compared equal, and of JSON_CONTAINS (`need.rs`).
//!
//! What a document posts is in `terms.rs`; how it is read and hashed in
//! `document.rs`; the tree's layout in `keys.rs`; its entries written and
//! taken back in `write.rs`; read in `search.rs`; built whole in
//! `load.rs`; its options and locator in `options.rs`.
//!
//! Its keys are projected (D85): an entry is counted under the hash of its
//! document, so readers recheck each row's document by its hash, and
//! every row found is checked by the whole WHERE condition (D34). A
//! document it cannot read is found by every probe, so that check raises
//! of it where a scan would (D15).

mod document;
mod keys;
mod load;
mod need;
mod options;
mod search;
mod terms;
mod write;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

pub use options::{MAX_DEPTH_OPTION, PATHS_ONLY_OPTION};
pub use terms::Shape;

use super::{Entries, Index, IndexMethod, Key, KeyOrder, KeyProjection, Predicate, Probe, RowIds};
use crate::catalog::{IndexDesc, TableDesc};
use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::io::Durability;
use crate::engine::tree::BLinkTree;
use crate::engine::{MEMORY_POOL_BYTES, MemoryPages};
use crate::error::{Code, Result, SqlError};
use crate::memory::Host;
use crate::spill::{Area, WorkAreas};
use crate::storage::{RowId, TableOption};
use crate::sync::lock_builtin;
use crate::value::{CompOp, DataType};
use options::{located, locator, shape_of, whole};

/// The name of `JsonMethod`.
pub const JSON_METHOD: &str = "json";

/// \[DESIGN\] Stripes of row locks, each serializing the changes of the
/// rows whose ids it takes modulo their count, as the full-text index's
/// do: fixed, whatever the index's size.
const ROW_LOCKS: u64 = 64;

/// \[EXTENSION\] The built-in json index method: inverted indexes of JSON
/// documents in B-link trees in one pager file.
pub struct JsonMethod {
    /// Where its indexes are; for a method in memory, made when the first
    /// index is.
    files: Mutex<Option<(Arc<BufferPool>, FileId)>>,
    /// Its pages in memory, for a file made in memory.
    memory: Arc<MemoryPages>,
}

impl JsonMethod {
    /// A method keeping its indexes in pager file `file` of `pool`.
    pub fn new(pool: Arc<BufferPool>, file: FileId) -> JsonMethod {
        JsonMethod { files: Mutex::new(Some((pool, file))), memory: MemoryPages::new(0) }
    }

    /// A method keeping its indexes in memory, in a pool of its own made
    /// with its first index, of the budget of the database it is attached
    /// to.
    pub fn in_memory() -> JsonMethod {
        JsonMethod::sharing(MemoryPages::new(MEMORY_POOL_BYTES))
    }

    /// A method keeping its indexes in `memory`, which others may share.
    pub(crate) fn sharing(memory: Arc<MemoryPages>) -> JsonMethod {
        JsonMethod { files: Mutex::new(None), memory }
    }

    /// \[EXTENSION\] The projection of every json index's keys (D85).
    pub fn projection() -> Arc<dyn KeyProjection> {
        Arc::new(document::Projection)
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
}

/// The error of what the method does not do.
fn unsupported(detail: &str) -> SqlError {
    SqlError::new(Code::Unsupported, format!("[EXTENSION] the json index method {detail}"))
}

/// Whether `columns` is one column of JSON documents, a character string's
/// or the JSON type's, as the method takes.
pub(crate) fn one_text_column(columns: &[DataType]) -> bool {
    let [column] = columns else { return false };
    column.is_character() || *column == DataType::Json
}

/// Refuses `columns` unless the method takes them.
///
/// # Errors
///
/// SQLCODE -900 if they are not one character string column.
fn validate(columns: &[DataType]) -> Result<()> {
    if one_text_column(columns) {
        Ok(())
    } else {
        Err(unsupported("indexes one column of JSON documents, of a character string type or JSON"))
    }
}

impl IndexMethod for JsonMethod {
    fn name(&self) -> &'static str {
        JSON_METHOD
    }
    fn attach(&self, host: &Host) -> Result<()> {
        self.memory.use_budget(Arc::clone(&host.memory));
        Ok(())
    }
    /// No comparison: a document's hash is all its key keeps.
    fn supports(&self, _op: CompOp) -> bool {
        false
    }
    /// A call of an SQL/JSON operator or JSON_CONTAINS whose first
    /// argument is the indexed column (`ScalarFunction::json_probe`).
    fn supports_predicate(&self, predicate: &Predicate, columns: &[DataType]) -> bool {
        let call = predicate.function.0.json_probe();
        call.is_some() && predicate.argument.is_none_or(|a| a == 0) && predicate.column == 0 && one_text_column(columns)
    }
    fn index_options(&self) -> Vec<TableOption> {
        options::options()
    }
    fn create(&self, columns: &[DataType]) -> Result<Box<dyn Index>> {
        validate(columns)?;
        let (pool, file) = self.files()?;
        Ok(Box::new(JsonIndex::with(BLinkTree::create(pool, file)?, whole())))
    }
    fn create_on(&self, columns: &[DataType], on: (&TableDesc, &IndexDesc)) -> Result<Box<dyn Index>> {
        if on.1.nulls {
            return Err(unsupported("does not store NULLS NOT DISTINCT keys"));
        }
        if on.1.collations.iter().any(Option::is_some) {
            return Err(unsupported(
                "reads a document's text, and a column with a collation is indexed by its sort key; index a column without a COLLATE clause",
            ));
        }
        validate(columns)?;
        let (pool, file) = self.files()?;
        Ok(Box::new(JsonIndex::with(BLinkTree::create(pool, file)?, shape_of(on.1))))
    }
    fn open(&self, columns: &[DataType], locator: &[u8]) -> Result<Option<Box<dyn Index>>> {
        validate(columns)?;
        let (meta, shape) = located(locator)?;
        let (pool, file) = self.files()?;
        Ok(Some(Box::new(JsonIndex::with(BLinkTree::open(pool, file, meta)?, shape))))
    }
    /// Writes back every page and syncs the file.
    fn checkpoint(&self) -> Result<()> {
        let (pool, file) = self.files()?;
        pool.sync(file, Durability::Full)
    }
}

/// \[EXTENSION\] A json index: its tree, its shape, and whether it was
/// dropped, so its pages are freed when it goes.
pub struct JsonIndex {
    tree: BLinkTree,
    /// What it posts of a document, fixed for its life.
    shape: Shape,
    /// Its row-lock stripes (`write.rs`).
    rows: Vec<Mutex<()>>,
    discarded: AtomicBool,
}

impl JsonIndex {
    /// The index of `tree`, posting what `shape` says.
    fn with(tree: BLinkTree, shape: Shape) -> JsonIndex {
        JsonIndex { tree, shape, rows: (0..ROW_LOCKS).map(|_| Mutex::new(())).collect(), discarded: AtomicBool::new(false) }
    }

    /// What it posts of a document.
    pub fn shape(&self) -> Shape {
        self.shape
    }

    /// The lock of `row`'s stripe.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if the stripe is missing, which it never is.
    fn row_lock(&self, row: RowId) -> Result<MutexGuard<'_, ()>> {
        let stripe = usize::try_from(row.checked_rem(ROW_LOCKS).unwrap_or_default()).unwrap_or_default();
        let lock = self.rows.get(stripe).ok_or_else(keys::corrupt)?;
        Ok(lock_builtin(lock))
    }
}

/// A dropped index's pages are freed when the last reference to it goes,
/// so no reader of an earlier snapshot reads a page given to another use.
/// A failure cannot be reported from here: the pages are then lost to the
/// file until it is rebuilt, which costs space only.
impl Drop for JsonIndex {
    fn drop(&mut self) {
        if self.discarded.load(Ordering::Acquire) {
            let _ = self.tree.free();
        }
    }
}

impl Index for JsonIndex {
    fn key_projection(&self) -> Option<Arc<dyn KeyProjection>> {
        Some(JsonMethod::projection())
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
        search::rows_of(self, key)
    }
    fn key_rows(self: Arc<Self>, key: &Key) -> RowIds {
        search::key_rows(&self, key)
    }
    fn estimate(&self, probe: &Probe) -> Option<usize> {
        search::estimated(self, probe)
    }
    /// A descent for each term looked up, its levels the tree's.
    fn probe_pages(&self, _probe: &Probe) -> Option<f64> {
        self.tree.height().ok().map(f64::from)
    }
    fn search(self: Arc<Self>, probe: &Probe, _order: KeyOrder) -> Entries {
        search::search(&self, probe)
    }
    /// The format, its tree's meta page and its shape.
    fn locator(&self) -> Vec<u8> {
        locator(self.tree.meta(), self.shape)
    }
    fn discard(&self) {
        self.discarded.store(true, Ordering::Release);
    }
}

// Unit tests: entries counted by version, postings written and taken
// back, searches against a scan, bulk loads, locators (RULES.md R-TEST-1,
// R-TEST-2).
#[cfg(test)]
mod tests;

//! \[EXTENSION\] RULES.md D86: the built-in XML index methods, after Pal et
//! al.'s XML indexes: "xml_path", keyed by each node's path then value
//! (PATH_VALUE, §3.2.1); "xml_value", by value then path (VALUE, §3.2.3);
//! "xml_property", by row, then path and value (PROPERTY, §3.2.2); and
//! "xml_primary", the rows of the Infoset, by row and ORDPATH (§3.1.1).
//! Each indexes one XML column, and holds its own keys: none needs another.
//!
//! An index is a B-link tree (`engine/tree.rs`) in the data file, beside
//! the B-link trees of `btree`, so it is checkpointed and reopened as
//! theirs are; a database kept nowhere else keeps it in memory. Its keys
//! are derived from a value's stored form in one pass over its events
//! (`derive.rs`): each element and attribute below an item of the value,
//! its path's id from the index's dictionary (`dictionary.rs`), its value
//! as a string and as a number (`keys.rs`), and its ORDPATH. The values
//! held, and how their entries are taken back unread, are `documents.rs`'s;
//! lookups (`Index::xml_rows`, `xml_cost`) are `search.rs`'s, the
//! primary's answers (`Index::xml_answers`) `primary.rs`'s, and a value's
//! own answer, which a lookup's rows are checked by, `check.rs`'s.
//!
//! No comparison of SQL probes an XML index: XML values are not comparable
//! [ISO/IEC 9075-14:2006 draft §4.2.4], so the planner never asks one for
//! a comparison, a range or a function call (RULES.md D34).

mod check;
mod derive;
mod dictionary;
mod documents;
mod keys;
mod lookup;
mod primary;
mod search;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub use derive::MAX_KEYED_NODES;
pub use lookup::{
    Axis, Comparison, Literal, MAX_STEPS, PathPattern, Step, Target, XmlAnswer, XmlAnswers, XmlCost, XmlFilter, XmlLookup, XmlQuery,
};

use super::{Entries, Index, IndexMethod, Key, KeyOrder, Probe, RowIds};
use crate::catalog::{IndexDesc, TableDesc};
use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::io::Durability;
use crate::disk::pager::PageId;
use crate::engine::tree::{BLinkTree, Direction};
use crate::engine::{MEMORY_POOL_BYTES, MemoryPages};
use crate::error::{Code, Result, SqlError};
use crate::memory::{Host, MemoryBudget};
use crate::spill::WorkAreas;
use crate::storage::RowId;
use crate::sync::lock_builtin;
use crate::value::{CompOp, DataType, Value};

/// The name of the method keyed by path, then value.
pub const XML_PATH_METHOD: &str = "xml_path";
/// The name of the method keyed by value, then path.
pub const XML_VALUE_METHOD: &str = "xml_value";
/// The name of the method keyed by row, then path and value.
pub const XML_PROPERTY_METHOD: &str = "xml_property";
/// The name of the method of the Infoset's rows, keyed by row and
/// ORDPATH.
pub const XML_PRIMARY_METHOD: &str = "xml_primary";

/// The built-in XML methods, as a database registers them.
pub const METHODS: [Method; 4] = [Method::Path, Method::Value, Method::Property, Method::Primary];

/// \[DESIGN\] How many octets of a node's string value an XML index keys
/// (RULES.md D86): a value longer is keyed by its first, marked cut, so an
/// entry is bounded; a lookup takes every value its first octets may
/// begin, and its rows are checked. A number is keyed only of a value this
/// short, which casts whole. The value is no measurement: the names,
/// codes, dates and numbers a path is compared with are shorter.
pub const VALUE_PREFIX_BYTES: usize = 64;

/// \[DESIGN\] How many octets of a node's string value the primary keys
/// (RULES.md D86): a value of no more is keyed whole, so a lookup answers
/// of it from the index alone; one longer is keyed by its first, cut, and
/// a row whose answer needs it is checked. The value is no measurement:
/// it leaves an entry, its ORDPATH of a deep node too, within the longest
/// key of the smallest page.
pub const PRIMARY_VALUE_BYTES: usize = 512;

/// How many locks the values held as large objects are taken under, each
/// object's by its id.
const STRIPES: usize = 16;
/// Bytes of a locator: its tree's meta page.
const LOCATOR: usize = 8;

/// \[EXTENSION\] What an XML index is keyed by first (RULES.md D86).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// "xml_path": path, value, row, ORDPATH.
    Path,
    /// "xml_value": value, path, row, ORDPATH.
    Value,
    /// "xml_property": row, path, value, ORDPATH.
    Property,
    /// "xml_primary": row, ORDPATH, then kind, path and value, the rows
    /// of the Infoset [Pal et al. §3.1.1].
    Primary,
}

impl Method {
    /// How many octets of a node's string value its indexes key.
    #[must_use]
    pub fn prefix(self) -> usize {
        match self {
            Method::Path | Method::Value | Method::Property => VALUE_PREFIX_BYTES,
            Method::Primary => PRIMARY_VALUE_BYTES,
        }
    }

    /// Its name, as CREATE INDEX's USING gives it.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Method::Path => XML_PATH_METHOD,
            Method::Value => XML_VALUE_METHOD,
            Method::Property => XML_PROPERTY_METHOD,
            Method::Primary => XML_PRIMARY_METHOD,
        }
    }
}

/// The error of a key of `n` octets, past `most`, the tree's longest.
pub(super) fn too_long(n: usize, most: usize) -> SqlError {
    SqlError::new(
        Code::Unsupported,
        format!(
            "[DESIGN] the XML value is beyond turborel's limits: an XML index's key of it has {n} octets, past the {most} this database's page size holds (RULES.md D18, D86)"
        ),
    )
}

/// The database's work areas, once the method is attached to it.
type Work = Arc<Mutex<Option<Arc<WorkAreas>>>>;

/// \[EXTENSION\] A built-in XML index method (RULES.md D86): B-link trees
/// in one pager file of a buffer pool.
pub struct XmlMethod {
    method: Method,
    /// Where its indexes are; for a method in memory, made when the first
    /// index is.
    files: Mutex<Option<(Arc<BufferPool>, FileId)>>,
    /// Its pages in memory, for a file made in memory.
    memory: Arc<MemoryPages>,
    /// The work areas of the database it is attached to.
    work: Work,
}

impl XmlMethod {
    /// The method `method`, keeping its indexes in pager file `file` of
    /// `pool`.
    pub fn new(method: Method, pool: Arc<BufferPool>, file: FileId) -> XmlMethod {
        XmlMethod { method, files: Mutex::new(Some((pool, file))), memory: MemoryPages::new(0), work: Work::default() }
    }

    /// \[DESIGN\] The method `method`, keeping its indexes in memory, in a
    /// file system of its own, made with its first index.
    pub fn in_memory(method: Method) -> XmlMethod {
        XmlMethod::sharing(method, MemoryPages::new(MEMORY_POOL_BYTES))
    }

    /// The method `method`, keeping its indexes in `memory`, which others
    /// may share.
    pub(crate) fn sharing(method: Method, memory: Arc<MemoryPages>) -> XmlMethod {
        XmlMethod { method, files: Mutex::new(None), memory, work: Work::default() }
    }

    /// Makes `budget` the one its pool in memory draws on, if made after.
    pub(crate) fn use_budget(&self, budget: Arc<MemoryBudget>) {
        self.memory.use_budget(budget);
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

    /// An index of the method in `tree`.
    fn made(&self, tree: BLinkTree) -> XmlIndex {
        XmlIndex {
            method: self.method,
            tree,
            discarded: AtomicBool::new(false),
            lock: Mutex::new(()),
            stripes: std::array::from_fn(|_| Mutex::new(())),
            work: Arc::clone(&self.work),
        }
    }
}

impl IndexMethod for XmlMethod {
    fn name(&self) -> &'static str {
        self.method.name()
    }
    fn attach(&self, host: &Host) -> Result<()> {
        self.use_budget(Arc::clone(&host.memory));
        *lock_builtin(&self.work) = Some(Arc::clone(&host.work));
        Ok(())
    }
    /// None: XML values are not comparable.
    fn supports(&self, _op: CompOp) -> bool {
        false
    }
    /// One column, of an XML type.
    fn check_columns(&self, columns: &[DataType], what: &str) -> Result<()> {
        if let [DataType::Xml(_)] = columns {
            return Ok(());
        }
        Err(SqlError::new(
            Code::SyntaxRule,
            format!("[EXTENSION] {what} of method {} keys one column, of an XML type (RULES.md D86)", self.name()),
        ))
    }
    fn create(&self, _columns: &[DataType]) -> Result<Box<dyn Index>> {
        let (pool, file) = self.files()?;
        Ok(Box::new(self.made(BLinkTree::create(pool, file)?)))
    }
    /// The index whose tree's meta page the locator names.
    fn open(&self, _columns: &[DataType], locator: &[u8]) -> Result<Option<Box<dyn Index>>> {
        let meta = <[u8; LOCATOR]>::try_from(locator)
            .map(u64::from_le_bytes)
            .map_err(|_| SqlError::new(Code::Corrupt, "[DESIGN] an XML index's locator is malformed (RULES.md D86)"))?;
        let (pool, file) = self.files()?;
        Ok(Some(Box::new(self.made(BLinkTree::open(pool, file, PageId(meta))?))))
    }
    fn create_on(&self, columns: &[DataType], _on: (&TableDesc, &IndexDesc)) -> Result<Box<dyn Index>> {
        self.create(columns)
    }
    /// Writes back every page and syncs the file.
    fn checkpoint(&self) -> Result<()> {
        let (pool, file) = self.files()?;
        pool.sync(file, Durability::Full)
    }
}

/// An index of an XML method: its tree, which holds its dictionary, the
/// values it holds and its entries; the lock its dictionary's ids are
/// given under; the locks of values held as large objects.
pub(super) struct XmlIndex {
    method: Method,
    tree: BLinkTree,
    discarded: AtomicBool,
    lock: Mutex<()>,
    stripes: [Mutex<()>; STRIPES],
    /// Where the rows a lookup has given spill.
    work: Work,
}

/// A dropped index's pages are freed when the last reference to it goes,
/// so no reader of an earlier snapshot reads a page given to another use.
/// A failure cannot be reported from here: the pages are then lost to the
/// file until it is rebuilt, which costs space only.
impl Drop for XmlIndex {
    fn drop(&mut self) {
        if self.discarded.load(Ordering::Acquire) {
            let _ = self.tree.free();
        }
    }
}

/// The value of a key of one column.
fn value_of(key: &Key) -> Result<&Value> {
    key.0.first().ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] an XML index is given a key of no column (RULES.md D86)"))
}

impl Index for XmlIndex {
    fn insert(&self, key: Key, row: RowId) -> Result<()> {
        self.add(value_of(&key)?, row)
    }

    fn remove(&self, key: &Key, row: RowId) -> Result<()> {
        self.take_back(value_of(key)?, row)
    }

    fn rows(&self, key: &Key) -> Result<Vec<RowId>> {
        self.rows_of(value_of(key)?)
    }

    /// Not known: no comparison probes it.
    fn estimate(&self, _probe: &Probe) -> Option<usize> {
        None
    }

    /// Of every entry, `Probe::All`, each (value, row) pair held, its key
    /// null, as an index of keys derived holds no value to give; no other
    /// probe, as no comparison probes it.
    fn search(self: Arc<Self>, probe: &Probe, _order: KeyOrder) -> Entries {
        if !matches!(probe, Probe::All) {
            return Box::new(std::iter::once(Err(SqlError::new(
                Code::Unsupported,
                "[EXTENSION] an XML index answers no comparison (RULES.md D86)",
            ))));
        }
        let held = Held(Arc::clone(&self));
        let marks = crate::engine::tree::scan_of(
            held,
            std::ops::Bound::Included(&[keys::MARKS][..]),
            std::ops::Bound::Excluded(&[keys::DOCUMENTS][..]),
            Direction::Forward,
        );
        Box::new(marks.map(|found| {
            let key = found?.0;
            let row = keys::Cursor { bytes: &key, at: key.len().saturating_sub(keys::ROW) }.row()?;
            Ok((Key(vec![Value::Null]), row))
        }))
    }

    fn locator(&self) -> Vec<u8> {
        self.tree.meta().0.to_le_bytes().to_vec()
    }

    fn discard(&self) {
        self.discarded.store(true, Ordering::Release);
    }

    fn xml_rows(self: Arc<Self>, lookup: &XmlLookup) -> Option<RowIds> {
        Some(self.lookup_rows(lookup))
    }

    /// Of the primary, the rows it answers of from its entries alone;
    /// none of a secondary method, whose rows are each checked.
    fn xml_answers(self: Arc<Self>, query: &XmlQuery) -> Option<XmlAnswers> {
        (self.method == Method::Primary).then(|| self.primary_answers(query))
    }

    fn xml_cost(self: Arc<Self>, lookup: &XmlLookup) -> Option<XmlCost> {
        Some(self.lookup_cost(lookup))
    }
}

/// An index's tree reached through the index it holds, so the index
/// outlives what reads it as it is taken.
pub(super) struct Held(Arc<XmlIndex>);

impl std::ops::Deref for Held {
    type Target = BLinkTree;
    fn deref(&self) -> &BLinkTree {
        &self.0.tree
    }
}

// Unit tests: entries added and taken back, by value and by object,
// lookups of each method, bounds (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;

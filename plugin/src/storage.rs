//! \[EXTENSION\] The traits of storage engines and index methods a plug-in
//! serves (RULES.md D224 to D227): the calls their payloads say
//! (`turborel::plugin::payload::engine`, `index`), each a method.

use turborel::storage::{Row, RowId};
use turborel::value::Value;

use super::{Answer, IndexAnswer, Near, Opened, Probed, TableAnswer, Write};

/// Entries of an index, each a row's id and its key.
pub type Entries = Vec<(RowId, Vec<Value>)>;

/// What SEARCH asks.
#[derive(Debug, Clone)]
pub struct Search {
    /// The entries selected.
    pub probe: Probed,
    /// Whether in descending order of key.
    pub descending: bool,
    /// Where to go on from: empty at first, else what the last page gave.
    pub cursor: Vec<u8>,
    /// The most entries of the page.
    pub max: u32,
}

/// Kind 16, a storage engine: tables kept outside the process, each named
/// by the locator `create` gives. `apply` makes its changes all or none,
/// and applying them twice is as once.
pub trait Engine {
    /// A new table `table` of `columns`, its locator.
    ///
    /// # Errors
    ///
    /// The engine's.
    fn create(&mut self, table: &str, columns: &[String]) -> Answer<Vec<u8>>;
    /// The table of `locator` opened again.
    ///
    /// # Errors
    ///
    /// The engine's.
    fn open(&mut self, locator: &[u8]) -> Answer<Opened>;
    /// At most `max` rows of ids above `after`, in order, and whether more
    /// follow.
    ///
    /// # Errors
    ///
    /// The engine's.
    fn scan(&mut self, locator: &[u8], after: RowId, max: u32) -> Answer<(Vec<(RowId, Row)>, bool)>;
    /// The rows of `ids`, none of an id it holds no row of.
    ///
    /// # Errors
    ///
    /// The engine's.
    fn get(&mut self, locator: &[u8], ids: &[RowId]) -> Answer<Vec<Option<Row>>>;
    /// Each row made to hold its change, all or none: what each held before.
    ///
    /// # Errors
    ///
    /// The engine's, nothing changed.
    fn apply(&mut self, locator: &[u8], changes: &[(RowId, Option<Row>)]) -> Answer<Vec<Option<Row>>>;
    /// What it applied made durable, and `through` kept with it.
    ///
    /// # Errors
    ///
    /// The engine's.
    fn checkpoint(&mut self, locator: &[u8], through: u64) -> Answer<()>;
    /// The table dropped.
    ///
    /// # Errors
    ///
    /// The engine's.
    fn drop_table(&mut self, locator: &[u8]) -> Answer<()>;
    /// What it knows of the table's rows, for the planner, each column's
    /// in order or none, and `refresh`, the rows written after which the
    /// database asks again (RULES.md D278). Asked once, then as the answer
    /// goes stale or the table is analyzed. By default it knows nothing, and
    /// the planner takes its defaults.
    ///
    /// # Errors
    ///
    /// The engine's, taken as nothing known.
    fn statistics(&mut self, _locator: &[u8]) -> Answer<Option<TableAnswer>> {
        Ok(None)
    }
}

/// Kind 17, an index method: indexes of counted entries kept outside the
/// process, each named by the locator `create` gives.
pub trait IndexMethod {
    /// A new index of keys of `types`, their names; its locator.
    ///
    /// # Errors
    ///
    /// The method's.
    fn create(&mut self, types: &[String]) -> Answer<Vec<u8>>;
    /// Whether the index of `locator` is found, and clean.
    ///
    /// # Errors
    ///
    /// The method's.
    fn open(&mut self, locator: &[u8]) -> Answer<(bool, bool)>;
    /// `writes` made, in order; the index not clean from before.
    ///
    /// # Errors
    ///
    /// The method's.
    fn write(&mut self, locator: &[u8], writes: &[Write]) -> Answer<()>;
    /// At most `max` entries `probe` selects after `cursor`, empty at
    /// first, in order of key; and the cursor to go on from, empty after
    /// the last.
    ///
    /// # Errors
    ///
    /// The method's.
    fn search(&mut self, locator: &[u8], search: &Search) -> Answer<(Entries, Vec<u8>)>;
    /// The ids of the entries of `key`, a null equal to a null.
    ///
    /// # Errors
    ///
    /// The method's.
    fn rows(&mut self, locator: &[u8], key: &[Value]) -> Answer<Vec<RowId>>;
    /// The nearest entries, nearest first. Default: none.
    ///
    /// # Errors
    ///
    /// The method's.
    fn nearest(&mut self, _locator: &[u8], _near: &Near) -> Answer<Vec<(f64, RowId, Vec<Value>)>> {
        Ok(Vec::new())
    }
    /// The index made durable and clean.
    ///
    /// # Errors
    ///
    /// The method's.
    fn checkpoint(&mut self, locator: &[u8]) -> Answer<()>;
    /// The index dropped.
    ///
    /// # Errors
    ///
    /// The method's.
    fn drop_index(&mut self, locator: &[u8]) -> Answer<()>;
    /// What it knows of the index's entries, for the planner, as an
    /// engine's `statistics`. By default it knows nothing.
    ///
    /// # Errors
    ///
    /// The method's, taken as nothing known.
    fn statistics(&mut self, _locator: &[u8]) -> Answer<Option<IndexAnswer>> {
        Ok(None)
    }
}

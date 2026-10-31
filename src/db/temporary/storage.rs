//! \[DESIGN\] What an instance of a temporary table stores (RULES.md D49):
//! its rows, in a row store in memory made when a transaction first reads
//! or writes them, and its entries of each index its session's definitions
//! give the table, each made complete or not at all. Split from
//! db/temporary.rs (RULES.md R-SIZE-2).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::Uses;
use crate::catalog::{IndexDesc, TableDesc};
use crate::db::api;
use crate::engine::rows::{RowStore, TableOptions};
use crate::error::{Code, Result, SqlError};
use crate::index::{self, BTreeMethod, HASH_METHOD, HashMethod, Index, IndexMethod, Key, KeyOrder, Probe};
use crate::log::Lsn;
use crate::memory::MemoryBudget;
use crate::spill::{Area, WorkAreas};
use crate::stats::Statistics;
use crate::storage::{Condition, Conjunction, Newest, Row, RowId, Rows, StorageEngine, TableStorage, project};
use crate::sync::lock_builtin;
use crate::txn::{Concurrency, Persistence, ReadMode, Timestamp, TxCapabilities, TxId, Undo};

/// The capabilities of an instance: the row store's, but never recovered,
/// so its changes are not logged.
const SCRATCH: TxCapabilities =
    TxCapabilities { undo: Undo::Native, concurrency: Concurrency::Engine { serializable: true }, persistence: Persistence::None };

/// \[DESIGN\] An instance's rows: none until a transaction first reads or
/// writes them (`TableStorage::begin`), then a table of a row store in
/// memory of its own; the transactions that began on them and have not
/// ended; and its entries of each index, by name.
pub(super) struct Rows_ {
    pub(super) desc: TableDesc,
    budget: Arc<MemoryBudget>,
    /// Which transaction may write it (`Uses::may_write`).
    uses: Arc<Uses>,
    made: Mutex<Option<Arc<dyn TableStorage>>>,
    pub(super) open: Mutex<Vec<TxId>>,
    indexes: Mutex<HashMap<String, Arc<Entries>>>,
}

impl Rows_ {
    /// None yet of `desc`, made of `budget`, written as `uses` lets.
    pub(super) fn new(desc: &TableDesc, budget: &Arc<MemoryBudget>, uses: &Arc<Uses>) -> Rows_ {
        Rows_ {
            desc: desc.clone(),
            budget: Arc::clone(budget),
            uses: Arc::clone(uses),
            made: Mutex::default(),
            open: Mutex::default(),
            indexes: Mutex::default(),
        }
    }

    pub(super) fn made(&self) -> Option<Arc<dyn TableStorage>> {
        lock_builtin(&self.made).clone()
    }

    /// Whether a transaction other than `tx` began on its rows and has not
    /// ended.
    pub(super) fn open_to_other(&self, tx: TxId) -> bool {
        lock_builtin(&self.open).iter().any(|&t| t != tx)
    }

    /// The rows, made if they are not, for transaction `tx`, which has
    /// begun on them, to write.
    fn make(&self, tx: TxId) -> Result<Arc<dyn TableStorage>> {
        self.uses.may_write(self, tx)?;
        let mut made = lock_builtin(&self.made);
        if let Some(rows) = made.as_ref() {
            return Ok(Arc::clone(rows));
        }
        let engine = RowStore::in_memory(TableOptions::default());
        engine.use_budget(Arc::clone(&self.budget));
        let rows: Arc<dyn TableStorage> = Arc::from(engine.create_table(&self.desc)?);
        *made = Some(Arc::clone(&rows));
        Ok(rows)
    }

    /// Transaction `tx` ended.
    fn ended(&self, tx: TxId) {
        lock_builtin(&self.open).retain(|&t| t != tx);
    }

    /// Empty again, each index's entries too.
    pub(super) fn emptied(&self) {
        *lock_builtin(&self.made) = None;
        lock_builtin(&self.indexes).values().for_each(|e| e.emptied());
    }

    /// Its entries of each index of `indexes`, by name, its entries of any
    /// other forgotten: those it has of the same columns and method; else
    /// made now, if they can be made complete: empty if it has no rows, or
    /// of every version of its rows if no transaction has them open. Else
    /// entries that refuse every use, so no statement reads or writes
    /// through entries that miss a row (RULES.md D49).
    pub(super) fn entries(&self, indexes: &[IndexDesc]) -> Vec<(String, Arc<Entries>)> {
        let mut kept = lock_builtin(&self.indexes);
        kept.retain(|name, _| indexes.iter().any(|ix| ix.name == *name));
        let mut out = Vec::new();
        for ix in indexes {
            let same = kept.get(&ix.name).filter(|e| same_index(&e.index, ix)).cloned();
            let entries = if let Some(entries) = same {
                entries
            } else {
                let made = Arc::new(Entries::new(&self.desc, ix, &self.budget));
                if self.complete(&made).is_ok() {
                    kept.insert(ix.name.clone(), Arc::clone(&made));
                    made
                } else {
                    Arc::new(Entries::refusing(&self.desc, ix))
                }
            };
            out.push((ix.name.clone(), entries));
        }
        out
    }

    /// Its entries of `indexes`, empty, for those it has none of: its
    /// transaction's ALTER TABLE added them, while no instance of the table
    /// holds a row (`Uses::alter`).
    pub(super) fn add_empty(&self, indexes: &[IndexDesc]) {
        let mut kept = lock_builtin(&self.indexes);
        for ix in indexes {
            kept.entry(ix.name.clone()).or_insert_with(|| Arc::new(Entries::new(&self.desc, ix, &self.budget)));
        }
    }

    /// Each of its entries.
    pub(super) fn all_entries(&self) -> Vec<Arc<Entries>> {
        lock_builtin(&self.indexes).values().cloned().collect()
    }

    /// `entries`, new, given an entry of each version of its rows.
    ///
    /// # Errors
    ///
    /// SQLCODE -911 if a transaction has its rows open, whose versions it
    /// has not committed; the index's, making entries.
    fn complete(&self, entries: &Entries) -> Result<()> {
        let Some(rows) = self.made() else { return Ok(()) };
        if !lock_builtin(&self.open).is_empty() {
            return Err(SqlError::new(Code::Conflict, "[DESIGN] the rows are open to a transaction".to_owned()));
        }
        let columns = &entries.index.columns;
        for read in rows.versions(Timestamp::MAX, Timestamp::MAX, columns) {
            let (id, row) = read?;
            let key = project(&row, columns).ok_or_else(|| api(format!("[DESIGN] a row is shorter than index {}", entries.index.name)))?;
            if !key.iter().any(crate::value::Value::is_null) {
                entries.insert(Key(key), id)?;
            }
        }
        Ok(())
    }
}

/// Whether `a` and `b` index the same columns, by the same method.
fn same_index(a: &IndexDesc, b: &IndexDesc) -> bool {
    a.columns == b.columns && a.method == b.method && a.unique == b.unique
}

/// The rows of an instance not made: none.
fn no_rows() -> Rows {
    Box::new(std::iter::empty())
}

impl TableStorage for Rows_ {
    fn insert(&self, tx: TxId, row: Row) -> Result<RowId> {
        self.make(tx)?.insert(tx, row)
    }
    fn update(&self, tx: TxId, id: RowId, row: Row) -> Result<()> {
        self.make(tx)?.update(tx, id, row)
    }
    fn delete(&self, tx: TxId, id: RowId) -> Result<()> {
        self.make(tx)?.delete(tx, id)
    }
    fn restore(&self, tx: TxId, id: RowId, row: Row) -> Result<()> {
        self.make(tx)?.restore(tx, id, row)
    }
    fn get(&self, tx: TxId, id: RowId) -> Result<Option<Row>> {
        self.made().map_or(Ok(None), |rows| rows.get(tx, id))
    }
    fn scan(self: Arc<Self>, tx: TxId, columns: &[usize]) -> Rows {
        self.made().map_or_else(no_rows, |rows| rows.scan(tx, columns))
    }
    fn scan_where(self: Arc<Self>, tx: TxId, columns: &[usize], condition: &Conjunction) -> Rows {
        self.made().map_or_else(no_rows, |rows| rows.scan_where(tx, columns, condition))
    }
    fn row_count(&self) -> usize {
        self.made().map_or(0, |rows| rows.row_count())
    }
    fn read_by_condition(&self, tx: TxId, condition: &Condition) -> Result<()> {
        self.made().map_or(Ok(()), |rows| rows.read_by_condition(tx, condition))
    }
    fn newest(&self, tx: TxId, id: RowId) -> Result<Newest> {
        self.made().map_or(Ok(Newest { row: None, pending: None }), |rows| rows.newest(tx, id))
    }
    fn statistics(&self) -> Option<Arc<dyn Statistics>> {
        self.made().and_then(|rows| rows.statistics())
    }
    fn analyze(&self) -> Result<()> {
        self.made().map_or(Ok(()), |rows| rows.analyze())
    }
    fn capabilities(&self) -> TxCapabilities {
        SCRATCH
    }
    fn begin(&self, tx: TxId, snapshot: Timestamp) -> Result<()> {
        // Joined only once begun: a failed begin is never ended.
        self.make(tx)?.begin(tx, snapshot).inspect_err(|_| self.ended(tx))
    }
    fn read_mode(&self, tx: TxId, mode: ReadMode) -> Result<()> {
        self.made().map_or(Ok(()), |rows| rows.read_mode(tx, mode))
    }
    fn savepoint(&self, tx: TxId) -> u64 {
        self.made().map_or(0, |rows| rows.savepoint(tx))
    }
    fn rollback_to(&self, tx: TxId, savepoint: u64) -> Result<()> {
        self.made().map_or(Ok(()), |rows| rows.rollback_to(tx, savepoint))
    }
    fn validate(&self, tx: TxId, at: Timestamp) -> Result<()> {
        self.made().map_or(Ok(()), |rows| rows.validate(tx, at))
    }
    fn commit(&self, tx: TxId, at: Timestamp) {
        if let Some(rows) = self.made() {
            rows.commit(tx, at);
        }
        self.ended(tx);
    }
    fn durable_through(&self) -> Lsn {
        Lsn(u64::MAX)
    }
    fn gc(&self, horizon: Timestamp, removed: &mut dyn FnMut(RowId, Row)) -> Result<()> {
        self.made().map_or(Ok(()), |rows| rows.gc(horizon, removed))
    }
    fn uncollected(&self) -> bool {
        self.made().is_some_and(|rows| rows.uncollected())
    }
    fn rollback(&self, tx: TxId) -> Result<()> {
        let undone = self.made().map_or(Ok(()), |rows| rows.rollback(tx));
        self.ended(tx);
        undone
    }
}

/// \[DESIGN\] An instance's entries of one of its table's indexes: none
/// until first written, then an index of its method, built in, in memory
/// of its own; or, if `refusing`, entries that could not be made complete,
/// which refuse every use.
pub(super) struct Entries {
    desc: TableDesc,
    pub(super) index: IndexDesc,
    budget: Option<Arc<MemoryBudget>>,
    made: Mutex<Option<Arc<dyn Index>>>,
}

impl Entries {
    pub(super) fn new(desc: &TableDesc, index: &IndexDesc, budget: &Arc<MemoryBudget>) -> Entries {
        Entries { desc: desc.clone(), index: index.clone(), budget: Some(Arc::clone(budget)), made: Mutex::default() }
    }

    /// Entries that refuse every use.
    pub(super) fn refusing(desc: &TableDesc, index: &IndexDesc) -> Entries {
        Entries { desc: desc.clone(), index: index.clone(), budget: None, made: Mutex::default() }
    }

    fn made(&self) -> Option<Arc<dyn Index>> {
        lock_builtin(&self.made).clone()
    }

    /// The refusal of a use of refusing entries.
    fn refused(&self) -> SqlError {
        SqlError::new(
            Code::Conflict,
            format!(
                "[DESIGN] index {} of temporary table {} was made while a transaction of this session had its rows open: end the transaction, and run the statement again (RULES.md D49)",
                self.index.name, self.desc.name
            ),
        )
    }

    /// The entries made, if they are not: of a hash index if its index is
    /// one, else of a B-link tree.
    ///
    /// # Errors
    ///
    /// SQLCODE -911 if they refuse every use; the method's, making them.
    fn make(&self) -> Result<Arc<dyn Index>> {
        let Some(budget) = &self.budget else { return Err(self.refused()) };
        let mut made = lock_builtin(&self.made);
        if let Some(entries) = made.as_ref() {
            return Ok(Arc::clone(entries));
        }
        let types =
            self.desc.types(&self.index.columns).ok_or_else(|| api(format!("[DESIGN] index {} names no column", self.index.name)))?;
        let on = (&self.desc, &self.index);
        let entries: Arc<dyn Index> = if self.index.method == HASH_METHOD {
            let method = HashMethod::in_memory();
            method.use_budget(Arc::clone(budget));
            Arc::from(method.create_on(&types, on)?)
        } else {
            let method = BTreeMethod::in_memory();
            method.use_budget(Arc::clone(budget));
            Arc::from(method.create_on(&types, on)?)
        };
        *made = Some(Arc::clone(&entries));
        Ok(entries)
    }

    /// The entries, if made; for refusing entries, the refusal.
    fn read(&self) -> Result<Option<Arc<dyn Index>>> {
        match &self.budget {
            Some(_) => Ok(self.made()),
            None => Err(self.refused()),
        }
    }

    /// Empty again.
    fn emptied(&self) {
        *lock_builtin(&self.made) = None;
    }
}

impl Index for Entries {
    fn insert(&self, key: Key, row: RowId) -> Result<()> {
        self.make()?.insert(key, row)
    }
    fn load(&self, entries: &mut dyn Iterator<Item = Result<(Key, RowId)>>, work: (&WorkAreas, Area)) -> Result<()> {
        self.make()?.load(entries, work)
    }
    fn remove(&self, key: &Key, row: RowId) -> Result<()> {
        self.read()?.map_or(Ok(()), |entries| entries.remove(key, row))
    }
    fn rows(&self, key: &Key) -> Result<Vec<RowId>> {
        self.read()?.map_or(Ok(Vec::new()), |entries| entries.rows(key))
    }
    fn estimate(&self, probe: &Probe) -> Option<usize> {
        self.made().map_or(Some(0), |entries| entries.estimate(probe))
    }
    fn distinct_keys(&self) -> Option<f64> {
        self.made().and_then(|entries| entries.distinct_keys())
    }
    fn search(self: Arc<Self>, probe: &Probe, order: KeyOrder) -> index::Entries {
        match self.read() {
            Ok(Some(entries)) => entries.search(probe, order),
            Ok(None) => Box::new(std::iter::empty()),
            Err(e) => Box::new(std::iter::once(Err(e))),
        }
    }
}

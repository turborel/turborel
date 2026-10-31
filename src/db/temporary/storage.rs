//! \[DESIGN\] What an instance of a temporary table stores (RULES.md D49):
//! its rows, in a row store made when a transaction first reads or writes
//! them, and its entries of each index its session's definitions give the
//! table, each made complete or not at all; and the older versions its
//! transactions keep (`db/temporary/undo.rs`); all in the instance's pages
//! (`db/temporary/pages.rs`), which spill to a scratch file. Split from
//! db/temporary.rs (RULES.md R-SIZE-2).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use super::Uses;
use super::pages::{Pages, Place};
use super::undo::PagedLog;
use crate::catalog::{IndexDesc, TableDesc};
use crate::db::api;
use crate::engine::rows::{RowStore, TableFiles, TableOptions, UndoStore};
use crate::error::{Code, Result, SqlError};
use crate::index::{self, BTreeMethod, HASH_METHOD, HashMethod, Index, IndexMethod, Key, KeyOrder, Probe};
use crate::log::Lsn;
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
/// writes them (`TableStorage::begin`), then a table of a row store in its
/// pages; the transactions that began on them and have not ended, those
/// that wrote them, and when the last that wrote them committed; and its
/// entries of each index, by name, in its pages too.
pub(super) struct Rows_ {
    pub(super) desc: TableDesc,
    /// Which transaction may write it (`Uses::may_write`).
    uses: Arc<Uses>,
    made: Mutex<Option<Arc<dyn TableStorage>>>,
    pub(super) open: Mutex<Vec<TxId>>,
    /// Those of `open` that wrote its rows.
    writers: Mutex<Vec<TxId>>,
    /// When the last transaction that wrote its rows committed; 0 if none
    /// did.
    written_at: AtomicU64,
    indexes: Mutex<HashMap<String, Arc<Entries>>>,
    /// Last, so dropped after the rows and entries made in them.
    pages: Arc<Pages>,
}

impl Rows_ {
    /// None yet of `desc`, its pages made at `place`, written as `uses`
    /// lets.
    pub(super) fn new(desc: &TableDesc, place: &Place, uses: &Arc<Uses>) -> Rows_ {
        Rows_ {
            desc: desc.clone(),
            uses: Arc::clone(uses),
            made: Mutex::default(),
            open: Mutex::default(),
            writers: Mutex::default(),
            written_at: AtomicU64::new(0),
            indexes: Mutex::default(),
            pages: Arc::new(Pages::new(place, &desc.name.to_string())),
        }
    }

    /// Its pages' pool made of `bytes`, if made after (`Pages::resize`).
    pub(super) fn resize_pages(&self, bytes: usize) {
        self.pages.resize(bytes);
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
        let (pool, file) = self.pages.file()?;
        // Older versions go to the pages too, a batch of them to a page
        // (RULES.md D49).
        let batch = pool.capacity(file)?;
        let undo = UndoStore::fitted(Arc::new(PagedLog::new(Arc::clone(&pool), file)), batch);
        let engine = RowStore::new(TableFiles::over(pool, file, undo), TableOptions::default());
        engine.use_work(Arc::clone(&self.pages.place.work));
        let rows: Arc<dyn TableStorage> = Arc::from(engine.create_table(&self.desc)?);
        *made = Some(Arc::clone(&rows));
        Ok(rows)
    }

    /// Whether a transaction that wrote its rows committed after
    /// `snapshot`, so that a transaction reading at it does not see them.
    pub(super) fn written_after(&self, snapshot: Timestamp) -> bool {
        self.written_at.load(Ordering::Acquire) > snapshot
    }

    /// The rows, made if they are not, for transaction `tx` to write.
    fn write(&self, tx: TxId) -> Result<Arc<dyn TableStorage>> {
        let rows = self.make(tx)?;
        let mut writers = lock_builtin(&self.writers);
        if !writers.contains(&tx) {
            writers.push(tx);
        }
        Ok(rows)
    }

    /// Transaction `tx` ended.
    fn ended(&self, tx: TxId) {
        lock_builtin(&self.writers).retain(|&t| t != tx);
        lock_builtin(&self.open).retain(|&t| t != tx);
    }

    /// Empty again, each index's entries too, its pages' file removed.
    pub(super) fn emptied(&self) {
        *lock_builtin(&self.made) = None;
        lock_builtin(&self.indexes).values().for_each(|e| e.emptied());
        self.pages.emptied();
    }

    /// Its entries of each index of `indexes`, by name, its entries of any
    /// other forgotten: those it has of the same columns and method; else
    /// made now, if they can be made complete: empty if it has no rows, or
    /// of every version of its rows if no transaction has them open. Else
    /// entries that refuse every use, so no statement reads or writes
    /// through entries that miss a row (RULES.md D49).
    pub(super) fn entries(&self, indexes: &[IndexDesc]) -> Vec<(String, Arc<Entries>)> {
        let mut kept = lock_builtin(&self.indexes);
        kept.retain(|name, entries| {
            let still = indexes.iter().any(|ix| ix.name == *name);
            if !still {
                entries.discard();
            }
            still
        });
        let mut out = Vec::new();
        for ix in indexes {
            let same = kept.get(&ix.name).filter(|e| same_index(&e.index, ix)).cloned();
            if same.is_none() {
                kept.get(&ix.name).inspect(|e| e.discard());
            }
            let entries = if let Some(entries) = same {
                entries
            } else {
                let made = Arc::new(Entries::new(&self.desc, ix, &self.pages));
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

    /// Its entries of `indexes`, empty, for those it has none of: it is a
    /// successor, of no row yet (`db/temporary/reshape.rs`).
    pub(super) fn add_empty(&self, indexes: &[IndexDesc]) {
        let mut kept = lock_builtin(&self.indexes);
        for ix in indexes {
            kept.entry(ix.name.clone()).or_insert_with(|| Arc::new(Entries::new(&self.desc, ix, &self.pages)));
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
        entries.add(rows.versions(Timestamp::MAX, Timestamp::MAX, &entries.index.columns))
    }

    /// Its entries of each index of `indexes`, by name, for transaction
    /// `tx`, of another session or its own, to check its rows against
    /// ALTER TABLE's constraints (`db/temporary/reshape.rs`): those it has
    /// of the same columns and method; else made now, and kept unless it
    /// has others of the name. Those of an index of `added`, which `tx`
    /// adds, so which goes if it rolls back, of the rows `tx` sees if it
    /// has begun on them (`Rows_::fill`); any other's as `entries` makes
    /// them. Else entries that refuse every use.
    ///
    /// # Errors
    ///
    /// The rows' and the index's, making entries of an index added.
    pub(super) fn entries_for(&self, indexes: &[IndexDesc], (added, tx): (&[String], TxId)) -> Result<Vec<(String, Arc<Entries>)>> {
        let mut kept = lock_builtin(&self.indexes);
        let mut out = Vec::new();
        for ix in indexes {
            let held = kept.get(&ix.name).cloned();
            if let Some(entries) = held.as_ref().filter(|e| same_index(&e.index, ix)) {
                out.push((ix.name.clone(), Arc::clone(entries)));
                continue;
            }
            let made = Arc::new(Entries::new(&self.desc, ix, &self.pages));
            let complete = if added.contains(&ix.name) { self.fill(&made, tx)? } else { self.complete(&made).is_ok() };
            if !complete {
                out.push((ix.name.clone(), Arc::new(Entries::refusing(&self.desc, ix))));
                continue;
            }
            if held.is_none() {
                kept.insert(ix.name.clone(), Arc::clone(&made));
            }
            out.push((ix.name.clone(), made));
        }
        Ok(out)
    }

    /// `entries`, new, of an index transaction `tx` adds, given an entry of
    /// each of its rows `tx` sees, if `tx` has begun on them, as no other
    /// transaction then has (`Uses::alter`): those every later transaction
    /// of its session sees if `tx` commits; else of each version of them.
    /// Whether it could: not while another transaction has them open.
    ///
    /// # Errors
    ///
    /// The rows' and the index's, making entries.
    fn fill(&self, entries: &Entries, tx: TxId) -> Result<bool> {
        let Some(rows) = self.made() else { return Ok(true) };
        let open = lock_builtin(&self.open).clone();
        if open.iter().any(|&t| t != tx) {
            return Ok(false);
        }
        let columns = &entries.index.columns;
        let read = if open.contains(&tx) { rows.scan(tx, columns) } else { rows.versions(Timestamp::MAX, Timestamp::MAX, columns) };
        entries.add(read)?;
        Ok(true)
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
        self.write(tx)?.insert(tx, row)
    }
    fn update(&self, tx: TxId, id: RowId, row: Row) -> Result<()> {
        self.write(tx)?.update(tx, id, row)
    }
    fn delete(&self, tx: TxId, id: RowId) -> Result<()> {
        self.write(tx)?.delete(tx, id)
    }
    fn restore(&self, tx: TxId, id: RowId, row: Row) -> Result<()> {
        self.write(tx)?.restore(tx, id, row)
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
        // Noted before it ends, so `Uses::alter` sees it open or written.
        if lock_builtin(&self.writers).contains(&tx) {
            self.written_at.fetch_max(at, Ordering::AcqRel);
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
/// until first written, then an index of its method, built in, in its
/// instance's pages; or, if `refusing`, entries that could not be made
/// complete, which refuse every use.
pub(super) struct Entries {
    desc: TableDesc,
    pub(super) index: IndexDesc,
    pages: Option<Arc<Pages>>,
    made: Mutex<Option<Arc<dyn Index>>>,
}

impl Entries {
    pub(super) fn new(desc: &TableDesc, index: &IndexDesc, pages: &Arc<Pages>) -> Entries {
        Entries { desc: desc.clone(), index: index.clone(), pages: Some(Arc::clone(pages)), made: Mutex::default() }
    }

    /// Entries that refuse every use.
    pub(super) fn refusing(desc: &TableDesc, index: &IndexDesc) -> Entries {
        Entries { desc: desc.clone(), index: index.clone(), pages: None, made: Mutex::default() }
    }

    fn made(&self) -> Option<Arc<dyn Index>> {
        lock_builtin(&self.made).clone()
    }

    /// An entry of each row `read` gives whose key has no null.
    ///
    /// # Errors
    ///
    /// The read's, and the index's.
    fn add(&self, read: Rows) -> Result<()> {
        let columns = &self.index.columns;
        for each in read {
            let (id, row) = each?;
            let key = project(&row, columns).ok_or_else(|| api(format!("[DESIGN] a row is shorter than index {}", self.index.name)))?;
            if !key.iter().any(crate::value::Value::is_null) {
                self.insert(Key(key), id)?;
            }
        }
        Ok(())
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

    /// The entries made, if they are not, in the instance's pages: of a
    /// hash index if its index is one, else of a B-link tree.
    ///
    /// # Errors
    ///
    /// SQLCODE -911 if they refuse every use; the pages', and the
    /// method's, making them.
    fn make(&self) -> Result<Arc<dyn Index>> {
        let Some(pages) = &self.pages else { return Err(self.refused()) };
        let mut made = lock_builtin(&self.made);
        if let Some(entries) = made.as_ref() {
            return Ok(Arc::clone(entries));
        }
        let types =
            self.desc.types(&self.index.columns).ok_or_else(|| api(format!("[DESIGN] index {} names no column", self.index.name)))?;
        let on = (&self.desc, &self.index);
        let (pool, file) = pages.file()?;
        let entries: Arc<dyn Index> = if self.index.method == HASH_METHOD {
            Arc::from(HashMethod::new(pool, file).create_on(&types, on)?)
        } else {
            Arc::from(BTreeMethod::new(pool, file).create_on(&types, on)?)
        };
        *made = Some(Arc::clone(&entries));
        Ok(entries)
    }

    /// The entries, if made; for refusing entries, the refusal.
    fn read(&self) -> Result<Option<Arc<dyn Index>>> {
        match &self.pages {
            Some(_) => Ok(self.made()),
            None => Err(self.refused()),
        }
    }

    /// Empty again.
    fn emptied(&self) {
        *lock_builtin(&self.made) = None;
    }

    /// Forgotten, as its index is dropped or replaced: its pages freed in
    /// the instance's file once no statement reads them.
    fn discard(&self) {
        if let Some(entries) = lock_builtin(&self.made).take() {
            entries.discard();
        }
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

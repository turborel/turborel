//! \[EXTENSION\] Temporary tables [ISO/IEC 9075:1992 §4.9, §11.3]: each
//! session's own instance of each.
//!
//! A temporary table is defined in a schema, as a persistent one is, and
//! its definition is logged and kept by checkpoints; but its rows are each
//! session's, "effectively empty at the start of an SQL-session" and gone
//! at its end [§4.9]. So the definitions a session's statements see
//! (`Session::with_temporaries`) give each temporary table the session's
//! own instance: its rows, its indexes and its lock, in place of those of
//! the definition, which stay empty. An instance's rows are made in
//! memory, of the database's budget, when a transaction of the session
//! first reads or writes it, its indexes' entries when first written, and
//! no change to it is logged: none is recovered [§4.9]. A table of ON COMMIT DELETE
//! ROWS is emptied as each transaction ends [§14.3 GR3]: committed, it is
//! as the DELETE GR3 runs leaves it; rolled back, as it was before the
//! transaction, empty too. One of ON COMMIT PRESERVE ROWS is collected
//! then, by its session (`Instance::collect`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use super::{Session, State, api, rule};
use crate::catalog::{IndexDesc, QualifiedName, TableDesc, TemporaryKind};
use crate::engine::rows::{RowStore, TableOptions};
use crate::error::{Code, Result, SqlError};
use crate::index::{self, BTreeMethod, HASH_METHOD, HashMethod, Index, IndexMethod, Key, KeyOrder, Probe};
use crate::log::Lsn;
use crate::memory::MemoryBudget;
use crate::spill::{Area, WorkAreas};
use crate::stats::Statistics;
use crate::storage::{Condition, Conjunction, Newest, Row, RowId, Rows, StorageEngine, TableStorage};
use crate::sync::lock_builtin;
use crate::txn::{Concurrency, Persistence, ReadMode, TableLock, Timestamp, TxCapabilities, TxId, Undo, remove_entries};

/// The capabilities of an instance: the row store's, but never recovered,
/// so its changes are not logged.
const SCRATCH: TxCapabilities =
    TxCapabilities { undo: Undo::Native, concurrency: Concurrency::Engine { serializable: true }, persistence: Persistence::None };

/// A session's instances of the temporary tables, each by its table and
/// the generation of its definition; and its declared local temporary
/// tables, each with its indexes (`db/declared.rs`).
#[derive(Default)]
pub(super) struct Temporaries {
    made: Mutex<HashMap<(QualifiedName, u32), Instance>>,
    pub(super) declared: Mutex<Vec<(TableDesc, Vec<IndexDesc>)>>,
}

/// \[DESIGN\] The database's instances of each temporary table, of every
/// session, and the transaction altering it, if one is (RULES.md D49): an
/// ALTER TABLE of one is made only while no instance holds a row, and no
/// other transaction writes one until it ends, so none has rows the new
/// definition does not describe.
#[derive(Default)]
pub(crate) struct Uses(Mutex<HashMap<QualifiedName, Use>>);

/// A temporary table's instances, and its altering transaction.
#[derive(Default)]
struct Use {
    instances: Vec<Weak<Rows_>>,
    altering: Option<TxId>,
}

impl Uses {
    /// Notes `rows`, an instance of temporary table `name`.
    fn register(&self, name: &QualifiedName, rows: &Arc<Rows_>) {
        let mut tables = lock_builtin(&self.0);
        let used = tables.entry(name.clone()).or_default();
        used.instances.retain(|w| w.strong_count() > 0);
        used.instances.push(Arc::downgrade(rows));
    }

    /// That transaction `tx` may write an instance of `name`: no other
    /// transaction is altering it.
    ///
    /// # Errors
    ///
    /// SQLCODE -911 if one is.
    fn may_write(&self, name: &QualifiedName, tx: TxId) -> Result<()> {
        let altering = lock_builtin(&self.0).get(name).and_then(|u| u.altering);
        if altering.is_some_and(|other| other != tx) {
            let msg =
                format!("[DESIGN] temporary table {name} is being altered: its rows are written once that transaction ends (RULES.md D49)");
            return Err(SqlError::new(Code::Conflict, msg));
        }
        Ok(())
    }

    /// [ISO/IEC 9075:1992 §11.10] Transaction `tx` alters temporary table
    /// `name`, which no instance of any session holds a row of: no other
    /// writes one until it ends.
    ///
    /// # Errors
    ///
    /// SQLCODE -911 if an instance holds a row.
    pub(super) fn alter(&self, name: &QualifiedName, tx: TxId) -> Result<()> {
        let mut tables = lock_builtin(&self.0);
        let used = tables.entry(name.clone()).or_default();
        used.instances.retain(|w| w.strong_count() > 0);
        if used.instances.iter().filter_map(Weak::upgrade).any(|rows| rows.row_count() > 0) {
            let msg = format!(
                "[DESIGN] temporary table {name} has rows in a session's instance: it is altered only while none does (RULES.md D49)"
            );
            return Err(SqlError::new(Code::Conflict, msg));
        }
        used.altering = Some(tx);
        Ok(())
    }

    /// Transaction `tx` ended: what it altered may be written again.
    fn ended(&self, tx: TxId) {
        for used in lock_builtin(&self.0).values_mut() {
            if used.altering == Some(tx) {
                used.altering = None;
            }
        }
    }
}

/// One instance: its rows, its lock, its indexes, and whether a commit
/// empties it.
struct Instance {
    rows: Arc<Rows_>,
    lock: Arc<TableLock>,
    indexes: HashMap<String, Arc<Entries>>,
    delete_rows: bool,
}

impl Session {
    /// [ISO/IEC 9075:1992 §4.9] `published`, with the session's declared
    /// local temporary tables (`db/declared.rs`), each temporary table in it
    /// this session's own instance: one made for it the first time it is
    /// seen, and forgotten once its definition is gone or replaced.
    pub(super) fn with_temporaries(&self, published: Arc<State>) -> Arc<State> {
        let st = self.with_declared(published);
        let temporary: Vec<TableDesc> = st.catalog.tables().into_iter().filter(|t| t.temporary.is_some()).cloned().collect();
        let mut made = lock_builtin(&self.temporaries.made);
        made.retain(|(name, generation), _| temporary.iter().any(|t| t.name == *name && t.generation == *generation));
        if temporary.is_empty() {
            return st;
        }
        let mut copy = State::clone(&st);
        for desc in temporary {
            let indexes: Vec<IndexDesc> = st.catalog.indexes(&desc.name).into_iter().cloned().collect();
            let key = (desc.name.clone(), desc.generation);
            let instance = made.entry(key).or_insert_with(|| Instance::of(&desc, &indexes, (&st.memory, &st.temporaries)));
            let rows: Arc<dyn TableStorage> = Arc::<Rows_>::clone(&instance.rows);
            copy.storage.insert(desc.name.clone(), rows);
            copy.locks.insert(desc.name.clone(), Arc::clone(&instance.lock));
            for (name, entries) in &instance.indexes {
                let entries: Arc<dyn Index> = Arc::<Entries>::clone(entries);
                copy.indexes.insert(name.clone(), entries);
            }
        }
        Arc::new(copy)
    }

    /// [ISO/IEC 9075:1992 §14.3 GR3] Its transaction, `tx` if it had one,
    /// ended: each instance of ON COMMIT DELETE ROWS emptied, each other
    /// collected (`Instance::collect`), and what it altered of `st`'s
    /// temporary tables written again.
    pub(super) fn temporaries_ended(&self, st: &State, tx: Option<TxId>) {
        if let Some(tx) = tx {
            st.temporaries.ended(tx);
        }
        for instance in lock_builtin(&self.temporaries.made).values() {
            if instance.delete_rows {
                instance.rows.emptied();
                instance.indexes.values().for_each(|e| e.emptied());
            } else {
                instance.collect();
            }
        }
    }
}

/// [ISO/IEC 9075:1992 §11.8 SR4, SR5; §11.9 SR3, SR4] That `from` may
/// refer to `to`, as a foreign key or a CHECK does, the Syntax Rules
/// `kinds` and `rows` cited: a persistent table to persistent ones alone;
/// a global temporary table to global ones; a created local one to those,
/// or created local ones; a declared one to any temporary one; and one
/// whose rows a commit keeps to none whose rows it deletes.
///
/// # Errors
///
/// SQLCODE -103 if it may not.
pub(super) fn may_refer(from: &TableDesc, to: &TableDesc, (kinds, rows): (&str, &str)) -> Result<()> {
    let (of, at) = (from.temporary.map(|t| t.kind), to.temporary.map(|t| t.kind));
    let kind_fits = match of {
        None => at.is_none(),
        Some(TemporaryKind::Global) => at == Some(TemporaryKind::Global),
        Some(TemporaryKind::CreatedLocal) => matches!(at, Some(TemporaryKind::Global | TemporaryKind::CreatedLocal)),
        Some(TemporaryKind::Declared) => at.is_some(),
    };
    if !kind_fits {
        return Err(rule(format!(
            "[ISO/IEC 9075:1992 {kinds}] {} may not refer to {}: a {} table, to a {} one",
            from.name,
            to.name,
            kind_of(of),
            kind_of(at)
        )));
    }
    let deletes = |t: &TableDesc| t.temporary.is_some_and(|t| t.delete_rows);
    if deletes(to) && !deletes(from) {
        return Err(rule(format!(
            "[ISO/IEC 9075:1992 {rows}] {} keeps its rows at commit, so may not refer to {}, whose rows a commit deletes",
            from.name, to.name
        )));
    }
    Ok(())
}

/// How a table of `kind` (none: persistent) is called.
fn kind_of(kind: Option<TemporaryKind>) -> &'static str {
    match kind {
        None => "persistent base",
        Some(TemporaryKind::Global) => "global temporary",
        Some(TemporaryKind::CreatedLocal) => "created local temporary",
        Some(TemporaryKind::Declared) => "declared local temporary",
    }
}

impl Instance {
    /// \[DESIGN\] Removes every version of its rows superseded or deleted,
    /// their index entries with them, as its session's transaction has
    /// ended: no transaction of another session reads an instance, and none
    /// of its own is open, so none can read them, whatever the database's
    /// horizon (RULES.md D49). A failure leaves them for its next end.
    fn collect(&self) {
        let Some(rows) = self.rows.made() else { return };
        let indexes: Vec<(&dyn Index, &[usize])> = self
            .indexes
            .values()
            .map(|entries| {
                let index: &dyn Index = &**entries;
                (index, entries.index.columns.as_slice())
            })
            .collect();
        let _ = rows.gc(Timestamp::MAX, &mut |id, row| remove_entries(&indexes, id, &row));
    }

    /// An instance of `desc`, with `indexes`, its rows and entries made as
    /// they are first used, of `budget`, noted in `uses`.
    fn of(desc: &TableDesc, indexes: &[IndexDesc], (budget, uses): (&Arc<MemoryBudget>, &Arc<Uses>)) -> Instance {
        let rows = Arc::new(Rows_ { desc: desc.clone(), budget: Arc::clone(budget), uses: Arc::clone(uses), made: Mutex::default() });
        uses.register(&desc.name, &rows);
        let entries = |ix: &IndexDesc| {
            let made = Entries { desc: desc.clone(), index: ix.clone(), budget: Arc::clone(budget), made: Mutex::default() };
            (ix.name.clone(), Arc::new(made))
        };
        let delete_rows = desc.temporary.is_some_and(|t| t.delete_rows);
        Instance { rows, lock: Arc::default(), indexes: indexes.iter().map(entries).collect(), delete_rows }
    }
}

/// \[DESIGN\] An instance's rows: none until a transaction first reads or
/// writes them (`TableStorage::begin`), then a table of a row store in
/// memory of its own.
struct Rows_ {
    desc: TableDesc,
    budget: Arc<MemoryBudget>,
    /// Which transaction may write it (`Uses::may_write`).
    uses: Arc<Uses>,
    made: Mutex<Option<Arc<dyn TableStorage>>>,
}

impl Rows_ {
    fn made(&self) -> Option<Arc<dyn TableStorage>> {
        lock_builtin(&self.made).clone()
    }

    /// The rows, made if they are not, for transaction `tx` to write.
    fn make(&self, tx: TxId) -> Result<Arc<dyn TableStorage>> {
        self.uses.may_write(&self.desc.name, tx)?;
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

    /// Empty again.
    fn emptied(&self) {
        *lock_builtin(&self.made) = None;
    }
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
        self.make(tx)?.begin(tx, snapshot)
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
        self.made().map_or(Ok(()), |rows| rows.rollback(tx))
    }
}

/// \[DESIGN\] An instance's entries of one of its table's indexes: none
/// until first written, then an index of its method, built in, in memory
/// of its own.
struct Entries {
    desc: TableDesc,
    index: IndexDesc,
    budget: Arc<MemoryBudget>,
    made: Mutex<Option<Arc<dyn Index>>>,
}

impl Entries {
    fn made(&self) -> Option<Arc<dyn Index>> {
        lock_builtin(&self.made).clone()
    }

    /// The entries, made if they are not: of a hash index if its index is
    /// one, else of a B-link tree.
    fn make(&self) -> Result<Arc<dyn Index>> {
        let mut made = lock_builtin(&self.made);
        if let Some(entries) = made.as_ref() {
            return Ok(Arc::clone(entries));
        }
        let types =
            self.desc.types(&self.index.columns).ok_or_else(|| api(format!("[DESIGN] index {} names no column", self.index.name)))?;
        let on = (&self.desc, &self.index);
        let entries: Arc<dyn Index> = if self.index.method == HASH_METHOD {
            let method = HashMethod::in_memory();
            method.use_budget(Arc::clone(&self.budget));
            Arc::from(method.create_on(&types, on)?)
        } else {
            let method = BTreeMethod::in_memory();
            method.use_budget(Arc::clone(&self.budget));
            Arc::from(method.create_on(&types, on)?)
        };
        *made = Some(Arc::clone(&entries));
        Ok(entries)
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
        self.made().map_or(Ok(()), |entries| entries.remove(key, row))
    }
    fn rows(&self, key: &Key) -> Result<Vec<RowId>> {
        self.made().map_or(Ok(Vec::new()), |entries| entries.rows(key))
    }
    fn estimate(&self, probe: &Probe) -> Option<usize> {
        self.made().map_or(Some(0), |entries| entries.estimate(probe))
    }
    fn distinct_keys(&self) -> Option<f64> {
        self.made().and_then(|entries| entries.distinct_keys())
    }
    fn search(self: Arc<Self>, probe: &Probe, order: KeyOrder) -> index::Entries {
        match self.made() {
            Some(entries) => entries.search(probe, order),
            None => Box::new(std::iter::empty()),
        }
    }
}

// Unit tests: an instance's collection (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;

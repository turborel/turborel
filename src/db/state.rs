//! \[DESIGN\] The database state: how it is built, from the built-in engines, methods and
//! rules and any tables a catalog already holds, and the lookups that rely on
//! the invariant documented on `State`. Split from db.rs (RULES.md R-SIZE-2).

use crate::memory::{DEFAULT_BUDGET, MemoryBudget};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::api;
use super::files::Reopening;
use super::sealing::DATABASE_OPTIONS;
use super::users::Users;
use crate::catalog::{Catalog, QualifiedName, TableDesc};
use crate::disk::encoding::Encodings;
use crate::engine::MemoryPages;
use crate::engine::rows::{COLUMN_STORE, ROW_STORE, RowStore, TableOptions};
use crate::error::Result;
use crate::functions::Registry;
use crate::hash::HashFunction;
use crate::index::{BTreeMethod, HashMethod, Index, IndexMethod};
use crate::log::Log;
use crate::optimizer::Optimizer;
use crate::physical::CostModel;
use crate::settings::{
    self, DEFAULT_ENGINE, INDEX_DESCENT_COST, LOCK_WAIT, SORT_COMPARE_COST, SPILL_BATCH, Settings, UNIQUE_INDEX_METHOD, WORK_MEMORY,
    batch_bytes, work_bytes,
};
use crate::spill::WorkAreas;
use crate::stats::Statistics;
use crate::storage::{StorageEngine, TableOption, TableStorage};
use crate::system::SystemTables;
use crate::txn::{Beginning, LockWait, Policy, Reading, TableLock, Transactions, Txn, UndoSpace};
use crate::value::types::TypeRegistry;

/// \[DESIGN\] A snapshot of the database's definitions: its catalog, the
/// handles to every table's storage and every index, and the registered
/// engines, methods, functions, rules and cost model (RULES.md D27).
///
/// A snapshot is never changed once published. A statement takes the current
/// one (`Database::snapshot`) and keeps it to the end, so it sees one set of
/// definitions throughout; a change of definitions builds a new snapshot and
/// publishes it (`Database::change`). Snapshots share everything unchanged,
/// and only point down to definitions and handles, never back, so reference
/// counting frees each when its last user finishes. The data behind the
/// handles, each table's lock, and the transaction counter are shared by
/// every snapshot and guard themselves.
///
/// Invariant: every table in `catalog` has its entries in `storage` and
/// `locks`, and every index its entry in `indexes`. `schema`, `create_index` and `with_catalog`
/// keep it, and `storage_of` and `index_of` rely on it.
#[derive(Clone)]
pub struct State {
    /// Counts the snapshots published: each change of definitions adds one.
    pub(crate) generation: u64,
    pub(crate) catalog: Arc<dyn Catalog>,
    pub(crate) engines: HashMap<String, Arc<dyn StorageEngine>>,
    /// \[EXTENSION\] The options plug-ins declared for every table, each with
    /// the plug-in's name, in order of declaration (RULES.md D41).
    pub(crate) table_options: Vec<(String, TableOption)>,
    /// The pipelines tables' ENCODINGS name, by the names, made once
    /// (`db/sealing.rs`); shared by every snapshot.
    pub(crate) seals: Arc<std::sync::Mutex<HashMap<String, Arc<crate::disk::encoding::Pipeline>>>>,
    /// \[EXTENSION\] The options plug-ins declared for every index, as
    /// `table_options` (RULES.md D41).
    pub(crate) index_options: Vec<(String, TableOption)>,
    pub(crate) methods: HashMap<String, Arc<dyn IndexMethod>>,
    /// Engines chosen for particular tables, overriding `DEFAULT_ENGINE`.
    pub(super) placement: HashMap<QualifiedName, String>,
    /// The hash functions registered: a file is read with the one it
    /// names, and new files checksummed with STORAGE.CHECKSUM's.
    pub(crate) hashes: Vec<Arc<dyn HashFunction>>,
    /// The encodings registered: a file is read through those it names,
    /// and new files encoded through STORAGE.ENCODINGS'; shared with the
    /// engines, for their tables' (`Host::encodings`).
    pub(crate) encodings: Arc<Encodings>,
    /// Whether the database is kept in a directory, its settings fixed at
    /// creation fixed (`settings::Effect::Creation`).
    pub(super) created: bool,
    /// The generation of the storage each table dropped had last, so a
    /// table defined again under its name takes a later one, and the log's
    /// changes of the one dropped are never taken for its (`db/recovery.rs`).
    pub(super) retired: HashMap<QualifiedName, u32>,
    /// The registry of settings and the database's values (`settings.rs`).
    pub(crate) settings: Settings,
    /// The users and the owner (`db/users.rs`).
    pub(crate) users: Users,
    /// The tables of INFORMATION_SCHEMA whose rows it makes (`system.rs`).
    pub(crate) system: SystemTables,
    pub(crate) storage: HashMap<QualifiedName, Arc<dyn TableStorage>>,
    /// Each table's lock, shared by every snapshot that knows the table.
    pub(crate) locks: HashMap<QualifiedName, Arc<TableLock>>,
    pub(crate) indexes: HashMap<String, Arc<dyn Index>>,
    /// Indexes being built: kept up to date by every write, but not read
    /// until built (`Database::create_index`).
    pub(crate) building: HashSet<String>,
    pub(crate) functions: Registry,
    /// The data types beyond the standard registered (RULES.md D53).
    pub(crate) types: TypeRegistry,
    /// The clock the datetime value functions read (`clock.rs`).
    pub(crate) clock: Arc<dyn crate::clock::Clock>,
    pub(crate) optimizer: Optimizer,
    /// The transaction counter and the graph of waits, shared by every snapshot.
    pub(crate) tx: Arc<Transactions>,
    /// The memory budget its caches draw on, shared by every snapshot
    /// (`memory.rs`).
    pub(crate) memory: Arc<MemoryBudget>,
    /// The pages of its built-in engine and index methods, if it is kept
    /// in memory: their pool made when first needed, sized by the setting
    /// STORAGE.POOL_BYTES then (`engine.rs`).
    pub(crate) pages: Arc<MemoryPages>,
    /// The sessions' instances of its temporary tables, shared by every
    /// snapshot (`db/temporary.rs`).
    pub(crate) temporaries: Arc<super::temporary::Uses>,
    /// Where its operators spill, shared by every snapshot (`spill.rs`).
    pub(crate) work: Arc<WorkAreas>,
    /// Its background work's jobs, shared by every snapshot (`threads.rs`,
    /// RULES.md D55).
    pub(crate) jobs: Arc<crate::threads::Jobs>,
    /// The files it holds, shared by every snapshot (`files.rs`, RULES.md
    /// D56).
    pub(crate) files: Arc<crate::files::Registry>,
    /// Where its plug-ins keep files of their own (RULES.md D56).
    pub(crate) plugins: Arc<crate::files::plugins::PluginFiles>,
    /// The log, the commit point of transactions begun with this snapshot
    /// (RULES.md D25); `None`, the default, writes none, as a database whose
    /// engines keep nothing between runs needs none.
    pub(crate) log: Option<Arc<dyn Log>>,
    /// While the database is opened from its files, the locators of its
    /// tables and indexes, and what could not be reopened (`db/files.rs`).
    pub(super) reopening: Option<Reopening>,
}

impl State {
    /// The state over `catalog`, with the built-in engines, index methods,
    /// set functions and optimizer rules, and nothing attached yet.
    pub(crate) fn with_builtins(catalog: Box<dyn Catalog>) -> State {
        let memory = MemoryBudget::new(DEFAULT_BUDGET);
        let mut st = State {
            generation: 0,
            catalog: Arc::from(catalog),
            engines: HashMap::new(),
            table_options: vec![(DATABASE_OPTIONS.to_owned(), crate::storage::encodings_option())],
            seals: Arc::default(),
            index_options: Vec::new(),
            methods: HashMap::new(),
            placement: HashMap::new(),
            hashes: crate::hash::crc::builtin(),
            encodings: Arc::default(),
            created: false,
            retired: HashMap::new(),
            settings: Settings::default(),
            users: Users::default(),
            system: crate::system::builtin(),
            storage: HashMap::new(),
            locks: HashMap::new(),
            indexes: HashMap::new(),
            functions: Registry::with_set_functions(),
            types: TypeRegistry::default(),
            clock: Arc::new(crate::clock::SystemClock),
            optimizer: Optimizer::with_builtin_rules(),
            tx: Arc::new(Transactions::default()),
            memory: Arc::clone(&memory),
            pages: MemoryPages::new(crate::db::POOL_BYTES),
            temporaries: Arc::default(),
            work: Arc::new(WorkAreas::new(Arc::clone(&memory))),
            jobs: Arc::default(),
            files: Arc::default(),
            plugins: Arc::new(crate::files::plugins::PluginFiles::in_memory(Arc::clone(&memory))),
            log: None,
            reopening: None,
            building: HashSet::new(),
        };
        st.attach_builtins(&memory);
        st
    }

    /// The built-in engines and index methods, given the budget `memory`
    /// as `attach` gives a plug-in it, sharing their pages in memory,
    /// sized by STORAGE.POOL_BYTES.
    fn attach_builtins(&mut self, memory: &Arc<MemoryBudget>) {
        let st = self;
        st.pages.use_budget(Arc::clone(memory));
        let engine = RowStore::sharing(TableOptions::default(), Arc::clone(&st.pages));
        engine.use_work(Arc::clone(&st.work));
        engine.use_encodings(Arc::clone(&st.encodings));
        let columns = RowStore::columns_sharing(TableOptions::default(), Arc::clone(&st.pages));
        columns.use_work(Arc::clone(&st.work));
        columns.use_encodings(Arc::clone(&st.encodings));
        st.engines.insert(COLUMN_STORE.to_owned(), Arc::new(columns));
        let btree = BTreeMethod::sharing(Arc::clone(&st.pages));
        st.engines.insert(ROW_STORE.to_owned(), Arc::new(engine));
        let hash = HashMethod::sharing(Arc::clone(&st.pages));
        let methods: Vec<Arc<dyn IndexMethod>> = vec![Arc::new(btree), Arc::new(hash)];
        for m in methods {
            st.methods.insert(m.name().to_string(), m);
        }
    }

    /// Gives each table and index already in the catalog its storage and index
    /// structure, establishing the invariant documented on `State`.
    pub(super) fn attach_catalog_objects(&mut self) -> Result<()> {
        let tables: Vec<TableDesc> = self.catalog.tables().into_iter().cloned().collect();
        for desc in &tables {
            let engine = self.engines.get(&desc.engine).ok_or_else(|| {
                api(format!("[DESIGN] the catalog places {} in engine {}, which is not built in", desc.name, desc.engine))
            })?;
            self.storage.insert(desc.name.clone(), Arc::from(engine.create_table(desc)?));
            self.locks.insert(desc.name.clone(), Arc::default());
            for ix in self.catalog.indexes(&desc.name) {
                let method = self.methods.get(&ix.method).ok_or_else(|| {
                    api(format!("[DESIGN] the catalog's index {} uses method {}, which is not built in", ix.name, ix.method))
                })?;
                let types = desc
                    .types(&ix.columns)
                    .ok_or_else(|| api(format!("[DESIGN] the catalog's index {} is on a column its table lacks", ix.name)))?;
                self.indexes.insert(ix.name.clone(), Arc::from(method.create(&types)?));
            }
        }
        Ok(())
    }
}

impl State {
    /// [X3.135-1986 §7.3 GR2a] a transaction is effectively initiated,
    /// writing to this snapshot's log.
    pub(crate) fn begin(&self) -> Txn {
        self.begin_waiting((self.lock_wait(), Reading::Serializable, Arc::default()))
    }

    /// A transaction, as `begin`, that waits for locks as `beginning` says,
    /// its session's own setting, if it has one, reads as it says, and is
    /// cancelled by its session's stop.
    pub(crate) fn begin_waiting(&self, beginning: Beginning) -> Txn {
        self.tx.begin(self.log.as_ref().map(Arc::clone), beginning, self.list_space())
    }

    /// \[DESIGN\] Where a list of this snapshot's spills (RULES.md D38): as
    /// many bytes held as WORK_MEMORY says, in batches of SPILL_BATCH's, in
    /// its work areas.
    pub(crate) fn list_space(&self) -> UndoSpace {
        let (limit, batch) = (work_bytes(self.settings.get(WORK_MEMORY)), batch_bytes(self.settings.get(SPILL_BATCH)));
        UndoSpace { limit, batch, areas: Some(Arc::clone(&self.work)), types: self.types.clone() }
    }

    /// The engine of tables a schema creates, but those placed
    /// (`DEFAULT_ENGINE`).
    pub(crate) fn table_engine(&self) -> &str {
        self.settings.word(DEFAULT_ENGINE)
    }

    /// The index method behind UNIQUE constraints (`UNIQUE_INDEX_METHOD`).
    pub(crate) fn constraint_method(&self) -> &str {
        self.settings.word(UNIQUE_INDEX_METHOD)
    }

    /// What to do with an engine that cannot give the guarantees of
    /// [X3.135-1986 §4.16] (`TRANSACTION_POLICY`).
    pub(crate) fn policy(&self) -> Policy {
        self.settings.policy()
    }

    /// Whether a transaction waits for a lock another holds, but for a
    /// session's own setting (`LOCK_WAIT`).
    pub(crate) fn lock_wait(&self) -> LockWait {
        settings::lock_wait(self.settings.get(LOCK_WAIT))
    }

    /// The records the catalog file holds before it is compacted
    /// (CATALOG.COMPACT_AFTER).
    pub(crate) fn compact_after(&self) -> usize {
        self.settings.count(settings::CATALOG_COMPACT_AFTER)
    }

    /// The weights and guesses of the physical planner (`PLANNER.*`).
    pub(crate) fn cost(&self) -> CostModel {
        let s = &self.settings;
        CostModel {
            index_descent: s.number(INDEX_DESCENT_COST),
            sort_compare: s.number(SORT_COMPARE_COST),
            equal_selectivity: s.number(settings::EQUAL_SELECTIVITY),
            range_selectivity: s.number(settings::RANGE_SELECTIVITY),
            null_fraction: s.number(settings::NULL_FRACTION),
            reorder_gain: s.number(settings::REORDER_GAIN),
        }
    }
}

/// Read access for plug-ins outside this crate, such as optimizer rules,
/// which receive the state through `RuleContext`.
impl State {
    /// The catalog: every schema, table and index.
    pub fn catalog(&self) -> &dyn Catalog {
        self.catalog.as_ref()
    }

    /// Rows currently stored in a table: a statistic for cost estimates.
    pub fn row_count(&self, table: &QualifiedName) -> Option<usize> {
        self.storage.get(table).map(|s| s.row_count())
    }

    /// What `table`'s engine knows of its rows, if anything (`stats.rs`).
    /// If nothing, and asking made its gathering due, the table is noted
    /// for the database's maintenance (`db/maintenance.rs`), which gathers
    /// them: planning never waits for a pass over a table (RULES.md D35).
    pub fn statistics(&self, table: &QualifiedName) -> Option<Arc<dyn Statistics>> {
        let storage = self.storage.get(table)?;
        let stats = storage.statistics();
        if stats.is_none() && storage.maintenance_due() {
            self.tx.note_maintenance(table);
        }
        stats
    }

    /// The distinct keys `index`, of `table`, knows it holds, if it knows
    /// (`Index::distinct_keys`). If it does not, and asking made a count
    /// due, the table is noted for the database's maintenance
    /// (`db/maintenance.rs`), which counts them: planning never waits for a
    /// pass over an index (RULES.md D35).
    pub(crate) fn distinct_keys(&self, table: &QualifiedName, index: &dyn Index) -> Option<f64> {
        let keys = index.distinct_keys();
        if keys.is_none() && index.maintenance_due() {
            self.tx.note_maintenance(table);
        }
        keys
    }
}

/// Lookups that rely on the invariant documented on `State`. They return an
/// error rather than panic, so a broken invariant is reported (R-ERR-1).
impl State {
    /// The storage of `table`.
    pub(crate) fn storage_of(&self, table: &QualifiedName) -> Result<&Arc<dyn TableStorage>> {
        self.storage.get(table).ok_or_else(|| api(format!("[DESIGN] {table} has no storage")))
    }

    /// The lock of `table`.
    pub(crate) fn lock_of(&self, table: &QualifiedName) -> Result<&Arc<TableLock>> {
        self.locks.get(table).ok_or_else(|| api(format!("[DESIGN] {table} has no lock")))
    }

    /// The generation of `table`'s storage, which its row changes are
    /// logged with (`TableDesc::generation`); 0 for a table not known.
    pub(crate) fn generation_of(&self, table: &QualifiedName) -> u32 {
        self.catalog.table(table).map_or(0, |t| t.generation)
    }

    /// The generation of the table of the index named `name`.
    pub(crate) fn index_generation(&self, name: &str) -> u32 {
        self.catalog.index(name).map_or(0, |ix| self.generation_of(&ix.table))
    }

    /// The index named `name`.
    pub(crate) fn index_of(&self, name: &str) -> Result<&Arc<dyn Index>> {
        self.indexes.get(name).ok_or_else(|| api(format!("[DESIGN] the index {name} does not exist")))
    }
}

// Unit tests: the lookups report a broken invariant instead of panicking
// (RULES.md R-ERR-1, R-TEST-2 robustness).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{ColumnDesc, IndexDesc, MemoryCatalog};
    use crate::error::Code;
    use crate::value::Spelled;
    use crate::value::{DataType, Decimal, Value};

    fn table(engine: &str) -> TableDesc {
        let column = ColumnDesc {
            name: "K".into(),
            data_type: DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Numeric },
            not_null: true,
            default: None,
            domain: None,
        };
        TableDesc {
            name: QualifiedName { auth: "A".into(), ident: "T".into() },
            columns: vec![column],
            constraints: vec![],
            engine: engine.into(),
            options: Vec::new(),
            generation: 0,
            temporary: None,
        }
    }

    /// Verifies: [DESIGN] `storage_of` and `index_of` return SQLCODE -910 for a
    /// table without storage and an index that does not exist.
    #[test]
    fn lookups_report_missing_objects() {
        let mut catalog = MemoryCatalog::default();
        catalog.create_schema("A", "A", vec![table(ROW_STORE)]);
        let st = State::with_builtins(Box::new(catalog)); // nothing attached: the invariant is broken
        let name = QualifiedName { auth: "A".into(), ident: "T".into() };
        assert_eq!(st.storage_of(&name).err().map(|e| e.code), Some(Code::Api));
        assert_eq!(st.index_of("NOPE").err().map(|e| e.code), Some(Code::Api));
    }

    /// A database with table A.T (K NOT NULL UNIQUE), one row inserted by an
    /// open transaction, which is returned.
    fn with_open_insert() -> (State, Txn, QualifiedName) {
        let mut st = State::with_builtins(Box::new(MemoryCatalog::default()));
        st.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INT NOT NULL UNIQUE)", (None, 0), None).unwrap();
        let name = QualifiedName { auth: "A".into(), ident: "T".into() };
        let tx = st.begin();
        st.insert_row(&tx, &name, &vec![Value::Exact(Decimal::new(1, 0))]).unwrap();
        (st, tx, name)
    }

    /// Verifies: [DESIGN] RULES.md D33: ROLLBACK undoes a change in the
    /// storage and index it was made in, even when the definitions no
    /// longer have them (as after a DROP TABLE): the row and its UNIQUE
    /// entry are gone there, nothing is damaged, and the transaction ends.
    #[test]
    fn rollback_undoes_where_the_changes_were_made() {
        let (mut st, tx, name) = with_open_insert();
        let storage = Arc::clone(st.storage_of(&name).unwrap());
        let index = st.indexes.values().next().map(Arc::clone).unwrap();
        st.storage.remove(&name);
        st.indexes.clear();
        st.rollback(&tx).unwrap();
        drop(tx);
        assert_eq!((st.tx.active(), st.damaged_tables()), (0, vec![]));
        assert_eq!(Arc::clone(&storage).scan(crate::txn::NO_TRANSACTION, &[0]).count(), 0, "the row undone");
        assert_eq!(index.search(&crate::index::Probe::All, crate::index::KeyOrder::Ascending).count(), 0, "its entry undone");
    }

    /// Verifies: [DESIGN] attaching a catalog whose index names an index method
    /// that is not built in fails with SQLCODE -910.
    #[test]
    fn attaching_an_unknown_index_method_fails() {
        let mut catalog = MemoryCatalog::default();
        catalog.create_schema("A", "A", vec![table(ROW_STORE)]);
        let name = QualifiedName { auth: "A".into(), ident: "T".into() };
        catalog.add_index(IndexDesc {
            name: "I".into(),
            table: name,
            columns: vec![0],
            method: "nope".into(),
            unique: false,
            options: Vec::new(),
        });
        let mut st = State::with_builtins(Box::new(catalog));
        assert_eq!(st.attach_catalog_objects().err().map(|e| e.code), Some(Code::Api));
    }

    /// Verifies: [DESIGN] RULES.md D27: a statement whose snapshot predates a
    /// change to its table's definitions (an index added) may not write the
    /// table, which would skip the index: SQLCODE -911.
    #[test]
    fn a_writer_with_a_stale_snapshot_is_refused() {
        let (st, tx, name) = with_open_insert();
        st.lock_of(&name).unwrap().definitions_changed(st.generation.saturating_add(1));
        let stale = st.insert_row(&tx, &name, &vec![Value::Exact(Decimal::new(2, 0))]);
        assert_eq!(stale.err().map(|e| e.code), Some(Code::Conflict));
    }
}

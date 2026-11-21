//! \[DESIGN\] The database state: how it is built, from the built-in engines,
//! methods and rules and any tables a catalog already holds, and the lookups
//! that rely on the invariant documented on `State`.

use crate::memory::{DEFAULT_BUDGET, MemoryBudget};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

mod bare;
mod uses;

use super::api;
use super::files::Reopening;
use super::users::Users;
use crate::catalog::{Catalog, QualifiedName, TableDesc};
use crate::disk::encoding::Encodings;
use crate::engine::MemoryPages;
use crate::engine::rows::{COLUMN_STORE, ROW_STORE, RowStore, TableOptions};
use crate::error::Result;
use crate::functions::Registry;
use crate::hash::HashFunction;
use crate::hash::xxhash::Xxh3Map;
use crate::index::vector::hybrid::HybridMethod;
use crate::index::{
    AdjacencyMethod, BTreeMethod, DiskAnnMethod, FullTextMethod, HashMethod, HnswMethod, Index, IndexMethod, IvfMethod, JsonMethod,
    LabelsMethod, RTreeMethod, ReachabilityMethod, SparseMethod, XmlMethod, xml,
};
use crate::log::Log;
use crate::optimizer::Optimizer;
use crate::physical::CostModel;
use crate::settings::{
    self, INDEX_DESCENT_COST, LOCKS_WAIT, MEMORY_WORK_BYTES, SORT_COMPARE_COST, SPILL_BATCH_BYTES, Settings, TABLES_ENGINE,
    TABLES_UNIQUE_INDEX_METHOD, batch_bytes, held_bytes,
};
use crate::spill::WorkAreas;
use crate::stats::Statistics;
use crate::stats::keyed::{KeyIndex, Keyed};
use crate::storage::{StorageEngine, TableOption, TableStorage};
use crate::system::SystemTables;
use crate::txn::{Beginning, LockWait, Policy, Reading, TableLock, Transactions, Txn, UndoSpace};
use crate::value::collation::Collation;
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
    /// Engines chosen for particular tables, overriding `TABLES.ENGINE`.
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
    /// \[DESIGN\] The CURRENT_TIMESTAMP of the definition being made, as its
    /// statement ran and its record logged it (`State::define`,
    /// `State::schema`): the timestamps of each descriptor it makes
    /// [ISO/IEC 9075-2:2023 §11.60 GR5], the same when the record is
    /// replayed, as no clock is read again.
    pub(super) stamp: crate::value::datetime::Now,
    /// The registry of settings and the database's values (`settings.rs`).
    pub(crate) settings: Settings,
    /// The users and the owner (`db/users.rs`).
    pub(crate) users: Users,
    /// The tables of INFORMATION_SCHEMA whose rows it makes (`system.rs`).
    pub(crate) system: SystemTables,
    /// Each table's storage, by its name, which is shared: a transaction
    /// the table joins holds it, so it copies no name (`State::join`).
    pub(crate) storage: Xxh3Map<Arc<QualifiedName>, Arc<dyn TableStorage>>,
    /// Each table's lock, shared by every snapshot that knows the table.
    pub(crate) locks: Xxh3Map<QualifiedName, Arc<TableLock>>,
    pub(crate) indexes: HashMap<String, Arc<dyn Index>>,
    /// Each index's counts of searches, by its name, shared by every
    /// snapshot that knows the index: kept in step with `indexes` as each
    /// snapshot is published (`State::published`, RULES.md D522).
    pub(crate) index_uses: Xxh3Map<String, Arc<crate::index::Uses>>,
    /// Indexes being built: kept up to date by every write, but not read
    /// until built (`Database::create_index`).
    pub(crate) building: HashSet<String>,
    pub(crate) functions: Registry,
    /// \[EXTENSION\] The functions a query of XQuery calls by name: the
    /// built-in ones and those registered (`xquery::Library`).
    pub(crate) xquery_functions: crate::xquery::Library,
    /// The data types beyond the standard registered (RULES.md D53).
    pub(crate) types: TypeRegistry,
    /// \[EXTENSION\] The rankings of full-text search (RULES.md D87),
    /// shared by every state.
    pub(crate) rankings: super::rankings::SharedRankings,
    /// \[EXTENSION\] The partition strategies (RULES.md D154), shared
    /// by every state.
    pub(crate) strategies: super::partitions::SharedStrategies,
    /// \[EXTENSION\] The metrics and quantizers of VECTOR (RULES.md D93),
    /// shared by every state.
    pub(crate) vectors: super::vectors::SharedVectors,
    /// \[EXTENSION\] The model plug-ins registered and the models of the
    /// catalog bound to them, shared by every state (`db/embedding.rs`).
    pub(crate) embedders: Arc<crate::vector::embed::Embedders>,
    /// \[EXTENSION\] The extensions created, by name, each with its
    /// objects and processes (RULES.md D169).
    pub(crate) extensions: Arc<BTreeMap<String, Arc<crate::plugin::Installed>>>,
    /// \[DESIGN\] What extensions are given, shared by every state.
    pub(crate) extension_host: Arc<crate::plugin::Host>,
    /// The collations there are, by name (RULES.md D60).
    pub(crate) collations: BTreeMap<String, Arc<dyn Collation>>,
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
    /// The counters of its sequence generators, shared by every snapshot
    /// (`db/sequences/counters.rs`, RULES.md D62).
    pub(crate) sequences: Arc<super::sequences::Counters>,
    /// Where its operators spill, shared by every snapshot (`spill.rs`).
    pub(crate) work: Arc<WorkAreas>,
    /// Its temporary large objects, kept in a scratch file of `work`'s,
    /// shared by every snapshot (`spill/lobs.rs`, RULES.md D79).
    pub(crate) temp_lobs: Arc<crate::spill::TempLobStore>,
    /// Its background work's jobs, shared by every snapshot (`threads.rs`,
    /// RULES.md D55).
    pub(crate) jobs: Arc<crate::threads::Jobs>,
    /// What ran since it was opened, its graph algorithms' latest runs,
    /// its statements' tallies and the last version 7 UUID it made, shared
    /// by every snapshot (`system.rs`).
    pub(crate) ran: Arc<crate::system::Ran>,
    /// What reads ahead for its pools on its read-ahead job, shared by
    /// every snapshot (`disk/buffer/ahead.rs`, RULES.md D30).
    pub(crate) ahead: Arc<crate::disk::buffer::AheadReader>,
    /// The sweep writing its files again in the formats its settings name
    /// (`db/rekey.rs`).
    pub(crate) rekey: Arc<super::rekey::Rekey>,
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

/// The default memory budget and the work areas within it.
fn budgets() -> (Arc<MemoryBudget>, Arc<WorkAreas>) {
    let memory = MemoryBudget::new(DEFAULT_BUDGET);
    let work = Arc::new(WorkAreas::new(Arc::clone(&memory)));
    (memory, work)
}

impl State {
    /// The state over `catalog`, with the built-in engines, index methods,
    /// set functions and optimizer rules, and nothing attached yet.
    pub(crate) fn with_builtins(catalog: Box<dyn Catalog>) -> State {
        let (memory, work) = budgets();
        let mut st = State::bare(catalog, (&memory, work));
        st.attach_builtins(&memory);
        st
    }

    /// The built-in engines' and rules' settings declared. Of short names
    /// none declared yet, they cannot be refused; a rule without one would
    /// be on, an engine without one at its default.
    fn declare_builtins(&mut self) {
        let engines: Vec<Arc<dyn StorageEngine>> = self.engines.values().cloned().collect();
        for engine in engines {
            let _ = self.declare_plugin(engine.name(), engine.configurable());
        }
        let _ = self.switch_rules();
        let ran = Arc::clone(&self.ran);
        let _ = self.declare_plugin(crate::system::STATEMENTS_GROUP, Some(&ran.statements));
    }

    /// The built-in engines and index methods, given the budget `memory`
    /// as `attach` gives a plug-in it, sharing their pages in memory,
    /// sized by STORAGE.POOL_BYTES; then their settings declared.
    fn attach_builtins(&mut self, memory: &Arc<MemoryBudget>) {
        let st = self;
        // EMBED, CHUNK_TEXT and EMBED_VERSION of the models registered, of no name taken yet.
        for f in crate::vector::embed::routines(&st.embedders) {
            let _ = st.functions.add_scalar(Arc::from(f));
        }
        st.pages.use_budget(Arc::clone(memory));
        st.pages.use_reader(Arc::clone(&st.ahead));
        let engine = RowStore::sharing(TableOptions::default(), Arc::clone(&st.pages));
        engine.use_work(Arc::clone(&st.work));
        engine.use_encodings(Arc::clone(&st.encodings));
        let columns = RowStore::columns_sharing(TableOptions::default(), Arc::clone(&st.pages));
        // The headers of its frozen groups held of the budget too (RULES.md D38).
        columns.use_budget(Arc::clone(memory));
        columns.use_work(Arc::clone(&st.work));
        columns.use_encodings(Arc::clone(&st.encodings));
        st.engines.insert(COLUMN_STORE.to_owned(), Arc::new(columns));
        let btree = BTreeMethod::sharing(Arc::clone(&st.pages));
        st.engines.insert(ROW_STORE.to_owned(), Arc::new(engine));
        let hash = HashMethod::sharing(Arc::clone(&st.pages));
        let spatial = RTreeMethod::sharing(Arc::clone(&st.pages));
        let words = FullTextMethod::sharing(Arc::clone(&st.pages)).with_rankings(st.rankings.closure());
        let (metrics, quantizers) = (st.vectors.metrics(), st.vectors.quantizers());
        let graph =
            DiskAnnMethod::sharing(Arc::clone(&st.pages)).with_metrics(Arc::clone(&metrics)).with_quantizers(Arc::clone(&quantizers));
        let layered =
            HnswMethod::sharing(Arc::clone(&st.pages)).with_metrics(Arc::clone(&metrics)).with_quantizers(Arc::clone(&quantizers));
        let lists = IvfMethod::sharing(Arc::clone(&st.pages)).with_metrics(Arc::clone(&metrics)).with_quantizers(Arc::clone(&quantizers));
        let sparse = SparseMethod::sharing(Arc::clone(&st.pages));
        let hybrid = HybridMethod::sharing(&st.pages).with_plugins(&(metrics, quantizers)).with_rankings(st.rankings.closure());
        let documents = JsonMethod::sharing(Arc::clone(&st.pages));
        // Its compacted adjacencies held of the budget too (RULES.md D38).
        let edges = AdjacencyMethod::sharing(Arc::clone(&st.pages)).with_budget(Arc::clone(memory));
        let methods: Vec<Arc<dyn IndexMethod>> = vec![
            Arc::new(btree),
            Arc::new(hash),
            Arc::new(spatial),
            Arc::new(words),
            Arc::new(documents),
            Arc::new(edges),
            Arc::new(LabelsMethod::sharing(Arc::clone(&st.pages)).with_budget(Arc::clone(memory))),
            Arc::new(ReachabilityMethod::sharing(Arc::clone(&st.pages)).with_budget(Arc::clone(memory))),
            Arc::new(graph),
            Arc::new(layered),
            Arc::new(lists),
            Arc::new(sparse),
            Arc::new(hybrid),
        ];
        // The XML methods (RULES.md D106), attached as the others are.
        let xml = xml::METHODS.into_iter().map(|m| -> Arc<dyn IndexMethod> { Arc::new(XmlMethod::sharing(m, Arc::clone(&st.pages))) });
        for m in methods.into_iter().chain(xml) {
            st.methods.insert(m.name().to_string(), m);
        }
        st.vector_functions();
        st.declare_builtins();
    }

    /// Gives each table and index already in the catalog its storage and index
    /// structure, establishing the invariant documented on `State`.
    pub(super) fn attach_catalog_objects(&mut self) -> Result<()> {
        let tables: Vec<TableDesc> = self.catalog.tables().into_iter().cloned().collect();
        for desc in &tables {
            let engine = self.engines.get(&desc.engine).ok_or_else(|| {
                api(format!("[DESIGN] the catalog places {} in engine {}, which is not built in", desc.name, desc.engine))
            })?;
            self.storage.insert(Arc::new(desc.name.clone()), Arc::from(engine.create_table(desc)?));
            self.locks.insert(desc.name.clone(), Arc::default());
            for ix in self.catalog.indexes(&desc.name) {
                let method = self.methods.get(&ix.method).ok_or_else(|| {
                    api(format!("[DESIGN] the catalog's index {} uses method {}, which is not built in", ix.name, ix.method))
                })?;
                let types = ix
                    .key_types(desc)
                    .ok_or_else(|| api(format!("[DESIGN] the catalog's index {} is on a column its table lacks", ix.name)))?;
                // Of its options and its table's, as CREATE INDEX made it
                // (RULES.md D41).
                self.indexes.insert(ix.name.clone(), Arc::from(method.create_on(&types, (desc, ix))?));
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
    /// many bytes held as MEMORY.WORK_BYTES says, in batches of SPILL.BATCH_BYTES's, in
    /// its work areas.
    pub(crate) fn list_space(&self) -> UndoSpace {
        let (limit, batch) = (held_bytes(self.settings.get(MEMORY_WORK_BYTES)), batch_bytes(self.settings.get(SPILL_BATCH_BYTES)));
        UndoSpace { limit, batch, areas: Some(Arc::clone(&self.work)), types: self.types.clone() }
    }

    /// The engine of tables a schema creates, but those placed
    /// (`TABLES.ENGINE`).
    pub(crate) fn table_engine(&self) -> &str {
        self.settings.word(TABLES_ENGINE)
    }

    /// The index method behind UNIQUE constraints (`TABLES.UNIQUE_INDEX_METHOD`).
    pub(crate) fn constraint_method(&self) -> &str {
        self.settings.word(TABLES_UNIQUE_INDEX_METHOD)
    }

    /// What to do with an engine that cannot give the guarantees of
    /// [X3.135-1986 §4.16] (`TRANSACTION.STRICT`).
    pub(crate) fn policy(&self) -> Policy {
        self.settings.policy()
    }

    /// Whether a transaction waits for a lock another holds, but for a
    /// session's own setting (`LOCKS.WAIT`).
    pub(crate) fn lock_wait(&self) -> LockWait {
        settings::lock_wait(self.settings.get(LOCKS_WAIT))
    }

    /// \[DESIGN\] RULES.md D36: why the setting `name`, fixed when a
    /// database is created in its directory, is taken no more, if it is
    /// not: the database was created, or opened, in its directory; or it
    /// is kept in memory, holding a schema, a table or a log, so that no
    /// directory will create it (`Database::open_files` refuses it then).
    pub(super) fn creation_refused(&self, name: &str) -> Option<String> {
        if self.created {
            return Some(format!("[DESIGN] {name} is fixed when the database is created"));
        }
        let in_memory = self.log.is_some() || !self.catalog.schemas().is_empty() || !self.catalog.tables().is_empty();
        in_memory.then(|| {
            format!("[DESIGN] {name} is fixed when a database is created in its directory: a database kept in memory does not take it")
        })
    }

    /// The records the catalog file holds before it is compacted
    /// (CATALOG.COMPACT_AFTER_IMAGES).
    pub(crate) fn compact_after(&self) -> usize {
        self.settings.count(settings::CATALOG_COMPACT_AFTER_IMAGES)
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
            estimate_margin: s.number(settings::ESTIMATE_MARGIN),
            grant_bytes: crate::physical::cost::spill::grant_bytes(self),
            degrees: crate::physical::cost::parallel::Degrees::of(self),
            runtime_filters: crate::exec::batched(self) && crate::physical::cost::runtime::wanted(self),
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

    /// Rows currently stored in a table, \[EXTENSION\] a partitioned one's
    /// in its partitions: a statistic for cost estimates.
    pub fn row_count(&self, table: &QualifiedName) -> Option<usize> {
        self.catalog.partitioning().leaves(table).iter().map(|t| self.storage.get(t).map(|s| s.row_count())).sum()
    }

    /// What `table`'s engine knows of its rows, if anything (`stats.rs`),
    /// with what its indexes count of their keys (`stats/keyed.rs`).
    /// If nothing, and asking made its gathering due, or this read made
    /// statistics changed enough wanted, the table is noted for the
    /// database's maintenance (`db/maintenance.rs`), which gathers them:
    /// planning never waits for a pass over a table (RULES.md D35, D473).
    /// A table no write commits to after the read is gathered so too.
    pub fn statistics(&self, table: &QualifiedName) -> Option<Arc<dyn Statistics>> {
        let storage = self.storage.get(table)?;
        let wanted = storage.statistics_wanted();
        let stats = storage.statistics();
        if (stats.is_none() && storage.maintenance_due()) || (!wanted && storage.statistics_wanted()) {
            self.tx.note_maintenance(table);
        }
        Some(Keyed::of(stats?, self.key_indexes(table)))
    }

    /// `table`'s indexes whose keys are its columns as they are, which
    /// count their distinct keys (RULES.md D383): none of an expression, a
    /// collation or being built.
    fn key_indexes(&self, table: &QualifiedName) -> Vec<KeyIndex> {
        let usable = |ix: &&crate::catalog::IndexDesc| {
            !ix.columns.is_empty() && ix.expressions.is_empty() && !ix.is_collated() && !self.building.contains(&ix.name)
        };
        let found = |ix: &crate::catalog::IndexDesc| Some(KeyIndex::new(&ix.columns, ix.unique, Arc::clone(self.index_of(&ix.name).ok()?)));
        self.catalog.indexes(table).into_iter().filter(usable).filter_map(found).collect()
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

    /// The storage of `table`, with its name as the state shares it.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the table has no storage.
    pub(crate) fn storage_named(&self, table: &QualifiedName) -> Result<(Arc<QualifiedName>, Arc<dyn TableStorage>)> {
        let (name, storage) = self.storage.get_key_value(table).ok_or_else(|| api(format!("[DESIGN] {table} has no storage")))?;
        Ok((Arc::clone(name), Arc::clone(storage)))
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

#[cfg(test)]
mod tests;

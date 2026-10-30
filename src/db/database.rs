//! \[DESIGN\] The `Database` handle: the current snapshot of its definitions,
//! how definitions change (RULES.md D27), and the extension points and
//! settings reached through Rust. Split from db.rs (RULES.md R-SIZE-2).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError, RwLock, Weak};
use std::thread::{self, ThreadId};

use super::background::Background;
use super::files::DatabaseFiles;
use super::stores::Storages;
use super::users::Credentials;
use super::{Session, State, qualified_name};
use crate::catalog::{Catalog, MemoryCatalog, QualifiedName};
use crate::diagnostics::{Diagnostics, Event, Extension, NoDiagnostics};
use crate::error::{Code, Result, SqlError, fail};
use crate::functions::{AggregateFunction, ScalarFunction};
use crate::index::{IndexMethod, Probe};
use crate::log::Log;
use crate::memory::Host;
use crate::optimizer::Optimizer;
use crate::physical::CostModel;
use crate::settings;
use crate::spill::Spill;
use crate::storage::StorageEngine;
use crate::sync::{lock, read, write};
use crate::txn::{Damage, LockMode, LockWait, Policy, SessionEnd};
use crate::value::Value;

/// A shared handle to a database. Cloning it is cheap, and clones may be used
/// from any thread (`Send + Sync`).
///
/// A plug-in (or a closure given to `with_optimizer` or `with_cost_model`)
/// must not call back into the database (RULES.md R-ERR-4). If one does, a
/// call into the session already running is refused with SQLCODE -910, and so
/// is a change of definitions from inside one; a call into another session
/// runs as a separate transaction, which may conflict with the caller's locks
/// (SQLCODE -911).
#[derive(Clone)]
pub struct Database(Arc<Shared>);

/// A handle to a database that does not keep it open: what a thread of the
/// database's own holds (`threads.rs`).
#[derive(Clone)]
pub(crate) struct WeakDatabase(Weak<Shared>);

/// \[DESIGN\] The sink `Database::events` gives.
struct Forward(WeakDatabase);

impl Diagnostics for Forward {
    fn event(&self, event: &Event) {
        if let Some(db) = self.0.upgrade() {
            db.emit(event);
        }
    }
}

impl WeakDatabase {
    /// The database, if it is still open somewhere.
    pub(crate) fn upgrade(&self) -> Option<Database> {
        self.0.upgrade().map(Database)
    }
}

/// What every handle to one database shares.
struct Shared {
    /// The current snapshot of the definitions. Held only to copy the `Arc`
    /// or to replace it, never while a statement runs.
    current: RwLock<Arc<State>>,
    /// The thread changing the definitions, if one is; changes take turns.
    changing: Mutex<Option<ThreadId>>,
    /// Signalled when a change ends, for a thread waiting its turn.
    turn_ended: Condvar,
    /// The files it was opened from, if any (`db/files.rs`).
    files: Mutex<Option<Arc<DatabaseFiles>>>,
    /// Set once it is closed: every call is then refused.
    closed: AtomicBool,
    /// Where its events go (RULES.md D40).
    diagnostics: RwLock<Arc<dyn Diagnostics>>,
    /// Where it keeps its catalog and its log, once it is kept in a
    /// directory (`db/stores.rs`).
    storages: Storages,
    /// What makes users' verifiers, if set (`db/users.rs`).
    credentials: RwLock<Option<Arc<dyn Credentials>>>,
    /// Its background work: checkpoints and maintenance (`db/background.rs`).
    background: Background,
}

/// One thread's turn to change the definitions. Dropping it ends the turn.
struct Turn<'a> {
    shared: &'a Shared,
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        // The slot is only ever set and cleared, so a lock poisoned by a
        // panicking thread still guards a valid slot.
        let mut changing = self.shared.changing.lock().unwrap_or_else(PoisonError::into_inner);
        *changing = None;
        self.shared.turn_ended.notify_one();
    }
}

impl Default for Database {
    fn default() -> Self {
        Self::new()
    }
}

impl Database {
    /// An empty database with the built-in catalog, engine, index methods,
    /// set functions and rewrite rules.
    pub fn new() -> Self {
        Database::over(State::with_builtins(Box::new(MemoryCatalog::default())))
    }

    fn over(state: State) -> Self {
        let background = Background::over(Arc::clone(&state.jobs));
        Database(Arc::new(Shared {
            current: RwLock::new(Arc::new(state)),
            changing: Mutex::new(None),
            turn_ended: Condvar::new(),
            files: Mutex::new(None),
            closed: AtomicBool::new(false),
            diagnostics: RwLock::new(Arc::new(NoDiagnostics)),
            storages: Storages::default(),
            credentials: RwLock::new(None),
            background,
        }))
    }

    /// \[DESIGN\] A database over a `Catalog` plug-in. Every table and index the
    /// catalog already holds gets its storage and index structure, from the
    /// built-in engines and index methods (the only ones registered yet).
    /// The built-in engines keep a database not opened from its files in
    /// memory alone, so those tables start empty.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the catalog names an engine or index method that is not built in.
    pub fn with_catalog(catalog: Box<dyn Catalog>) -> Result<Self> {
        let mut st = State::with_builtins(catalog);
        st.attach_catalog_objects()?;
        Ok(Database::over(st))
    }

    /// The current snapshot of the definitions, for one statement or cursor.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a thread panicked while publishing a snapshot, or
    /// the database is closed.
    pub(crate) fn snapshot(&self) -> Result<Arc<State>> {
        if self.0.closed.load(Ordering::Acquire) {
            return fail(Code::Api, "[DESIGN] the database is closed");
        }
        Ok(Arc::clone(&*read(&self.0.current)?))
    }

    /// The files it was opened from, if any.
    pub(super) fn files(&self) -> Result<Option<Arc<DatabaseFiles>>> {
        Ok(lock(&self.0.files)?.clone())
    }

    /// Keeps `files` as those it was opened from.
    pub(super) fn attach_files(&self, files: DatabaseFiles) -> Result<()> {
        *lock(&self.0.files)? = Some(Arc::new(files));
        Ok(())
    }

    /// Its background work.
    pub(super) fn background(&self) -> &Background {
        &self.0.background
    }

    /// A handle that does not keep the database open.
    pub(crate) fn downgrade(&self) -> WeakDatabase {
        WeakDatabase(Arc::downgrade(&self.0))
    }

    /// A sink that gives each event to the database's sink of the moment,
    /// while the database is open somewhere: for a part of it that tells
    /// of failures it goes on past, as its logs (RULES.md D40, D56).
    pub(crate) fn events(&self) -> Arc<dyn Diagnostics> {
        Arc::new(Forward(self.downgrade()))
    }

    /// Refuses every call from now on.
    pub(super) fn mark_closed(&self) {
        self.0.closed.store(true, Ordering::Release);
    }

    /// Changes the definitions: runs `change` on a copy of the current
    /// snapshot, and publishes the copy if `change` succeeds. A failed change
    /// leaves the definitions as they were. Changes take turns; statements
    /// never wait for them, nor they for statements.
    ///
    /// # Errors
    ///
    /// The error of `change`; SQLCODE -910 if this thread is already changing
    /// the definitions (a call back from a plug-in or closure), or if a lock
    /// is poisoned.
    pub(super) fn change<T>(&self, change: impl FnOnce(&mut State) -> Result<T>) -> Result<T> {
        let _turn = self.take_turn()?;
        let mut next = State::clone(&*self.snapshot()?);
        next.generation = next.generation.saturating_add(1);
        let out = change(&mut next)?;
        // What spills is encrypted while anything is (RULES.md D48).
        next.work.set_encodings(next.spill_encodings());
        *write(&self.0.current)? = Arc::new(next);
        Ok(out)
    }

    /// Waits until no other thread is changing the definitions, then takes
    /// the turn.
    fn take_turn(&self) -> Result<Turn<'_>> {
        let me = thread::current().id();
        let mut changing = lock(&self.0.changing)?;
        while let Some(owner) = *changing {
            if owner == me {
                return fail(
                    Code::Api,
                    "[DESIGN] the definitions are already being changed on this thread: a plug-in or closure called back (RULES.md R-ERR-4)",
                );
            }
            changing = self.0.turn_ended.wait(changing).map_err(|_| poisoned_turn())?;
        }
        *changing = Some(me);
        Ok(Turn { shared: &self.0 })
    }
}

/// The error for a turn whose lock a panicking thread poisoned.
fn poisoned_turn() -> SqlError {
    SqlError::new(Code::Api, "[DESIGN] a thread panicked while changing the definitions (RULES.md D27)")
}

impl Database {
    // ----- extension points -----

    /// \[DESIGN\] Sends its events to `sink` from now on (RULES.md D40).
    pub fn set_diagnostics(&self, sink: Box<dyn Diagnostics>) {
        *self.0.diagnostics.write().unwrap_or_else(PoisonError::into_inner) = Arc::from(sink);
    }

    /// \[DESIGN\] Makes the spill areas of its operators, and of its
    /// plug-ins, with `spill` from now on (RULES.md D38); an area already
    /// made stays its maker's.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the database is closed.
    pub fn set_spill(&self, spill: Box<dyn Spill>) -> Result<()> {
        self.snapshot()?.work.set_spill(Arc::from(spill));
        Ok(())
    }

    /// Where it keeps its catalog and its log (`db/stores.rs`).
    pub(super) fn storages(&self) -> &Storages {
        &self.0.storages
    }

    /// Where what makes users' verifiers is kept (`db/users.rs`).
    pub(super) fn credentials_slot(&self) -> &RwLock<Option<Arc<dyn Credentials>>> {
        &self.0.credentials
    }

    /// Gives `event` to its sink, holding no lock while the sink runs.
    pub(crate) fn emit(&self, event: &Event) {
        let sink = Arc::clone(&self.0.diagnostics.read().unwrap_or_else(PoisonError::into_inner));
        sink.event(event);
    }

    /// `result`, its failure given to the sink first if an operator must
    /// know of it (`Event::of`).
    pub(crate) fn noted<T>(&self, result: Result<T>) -> Result<T> {
        if let Some(event) = result.as_ref().err().and_then(Event::of) {
            self.emit(&event);
        }
        result
    }

    /// `result` of registering the plug-in `name` of `kind`, the event of
    /// it given to the sink if it was registered.
    pub(crate) fn registered(&self, (kind, name): (Extension, &str), result: Result<()>) -> Result<()> {
        if result.is_ok() {
            self.emit(&Event::Registered { kind, name: name.to_owned() });
        }
        result
    }

    /// What it provides the plug-ins registered with it.
    fn host(&self) -> Result<Host> {
        let st = self.snapshot()?;
        Ok(Host::new(Arc::clone(&st.memory), Arc::clone(&st.work), Arc::clone(&st.encodings), Arc::clone(&st.plugins)))
    }

    /// \[DESIGN\] Adds or replaces a storage engine, under its name, once
    /// it has taken what the database provides (`StorageEngine::attach`).
    ///
    /// # Errors
    ///
    /// The engine's, from `attach`.
    /// SQLCODE -910 if called back from a plug-in.
    pub fn register_engine(&self, engine: Box<dyn StorageEngine>) -> Result<()> {
        let name = engine.name();
        engine.attach(&self.host()?)?;
        let done = self.change(|st| {
            // [EXTENSION] RULES.md D41: a table option is one plug-in's.
            if let Some(o) = engine.table_options().iter().find(|o| st.table_options.iter().any(|(_, p)| p.name == o.name)) {
                return fail(Code::Api, format!("[DESIGN] the table option {} of engine {name} is a plug-in's", o.name));
            }
            st.engines.insert(name.to_owned(), Arc::from(engine));
            Ok(())
        });
        self.registered((Extension::Engine, name), done)
    }

    /// \[DESIGN\] Adds or replaces an index method, under its name, once it
    /// has taken what the database provides (`IndexMethod::attach`).
    ///
    /// # Errors
    ///
    /// The method's, from `attach`.
    /// SQLCODE -910 if called back from a plug-in.
    pub fn register_index_method(&self, method: Box<dyn IndexMethod>) -> Result<()> {
        let name = method.name();
        method.attach(&self.host()?)?;
        let done = self.change(|st| {
            // [EXTENSION] RULES.md D41: an index option is one plug-in's.
            if let Some(o) = method.index_options().iter().find(|o| st.index_options.iter().any(|(_, p)| p.name == o.name)) {
                return fail(Code::Api, format!("[DESIGN] the index option {} of method {name} is a plug-in's", o.name));
            }
            st.methods.insert(name.to_owned(), Arc::from(method));
            Ok(())
        });
        self.registered((Extension::IndexMethod, name), done)
    }

    /// \[EXTENSION\] Register a scalar function. Callable only from sessions
    /// that enable the function call syntax.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if an aggregate function has the same name.
    pub fn register_scalar_function(&self, f: Box<dyn ScalarFunction>) -> Result<()> {
        let name = f.name().to_owned();
        let done = self.change(|st| st.functions.add_scalar(Arc::from(f)));
        self.registered((Extension::Function, &name), done)
    }

    /// \[EXTENSION\] Register an aggregate. Registering COUNT, SUM, AVG, MAX or
    /// MIN replaces that set function's implementation [X3.135-1986 §5.8];
    /// any other name is callable only through the function call syntax.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a scalar function has the same name.
    pub fn register_aggregate_function(&self, f: Box<dyn AggregateFunction>) -> Result<()> {
        let name = f.name().to_owned();
        let done = self.change(|st| st.functions.add_aggregate(Arc::from(f)));
        self.registered((Extension::Function, &name), done)
    }

    /// \[DESIGN\] Choose the storage engine for tables in subsequent
    /// `<schema>`s: the setting `DEFAULT_ENGINE`.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if no engine has that name.
    pub fn use_engine(&self, name: &str) -> Result<()> {
        self.set_setting("DEFAULT_ENGINE", &Value::Char(name.to_owned()))
    }

    /// \[DESIGN\] Choose the storage engine for one table, "AUTH.TABLE", before
    /// the `<schema>` that defines it. Overrides `use_engine` for that table, so
    /// one schema can keep tables in different engines.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if no engine has that name, or `table` is not "AUTH.TABLE".
    pub fn place_table(&self, table: &str, engine: &str) -> Result<()> {
        let name = qualified_name(table)?;
        self.change(|st| {
            if !st.engines.contains_key(engine) {
                return fail(Code::Api, format!("[DESIGN] no storage engine named {engine}"));
            }
            st.placement.insert(name, engine.to_string());
            Ok(())
        })
    }

    /// \[DESIGN\] Runs `change` on the rule-based optimizer: add rules, enable
    /// or disable them, list them. The change is published when `change`
    /// returns; statements already running keep the rules they started with.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if called back from a plug-in or from inside such a closure.
    pub fn with_optimizer<T>(&self, change: impl FnOnce(&mut Optimizer) -> T) -> Result<T> {
        self.change(|st| Ok(change(&mut st.optimizer)))
    }

    /// \[DESIGN\] Runs `change` on the weights used by the physical planner, as
    /// `with_optimizer` runs its closure. The changed weights are kept only if
    /// every one is finite and not negative.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if called back as `with_optimizer` says, or if `change`
    /// leaves a weight NaN, infinite or negative; the weights are then unchanged.
    pub fn with_cost_model<T>(&self, change: impl FnOnce(&mut CostModel) -> T) -> Result<T> {
        self.change(|st| {
            let mut cost = st.cost();
            let out = change(&mut cost);
            cost.check()?;
            let weights = [
                ("PLANNER.INDEX_DESCENT_COST", cost.index_descent),
                ("PLANNER.SORT_COMPARE_COST", cost.sort_compare),
                ("PLANNER.EQUAL_SELECTIVITY", cost.equal_selectivity),
                ("PLANNER.RANGE_SELECTIVITY", cost.range_selectivity),
                ("PLANNER.NULL_FRACTION", cost.null_fraction),
                ("PLANNER.REORDER_GAIN", cost.reorder_gain),
            ];
            for (name, weight) in weights {
                st.define_setting(name, Some(&Value::Approx(weight)))?;
            }
            Ok(out)
        })
    }

    /// \[DESIGN\] What to do when an engine falls short of [X3.135-1986 §4.16].
    /// Like every setting, it is part of the definitions: statements already
    /// running keep the one they started with.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if called back from a plug-in, or a lock is poisoned.
    pub fn set_transaction_policy(&self, policy: Policy) -> Result<()> {
        self.set_setting("TRANSACTION_POLICY", &Value::Char(settings::policy_word(policy).to_owned()))
    }

    /// \[DESIGN\] Whether a transaction that finds a table locked against it
    /// waits (`LockWait::Wait`, for sessions on threads of their own) or is
    /// refused at once (`LockWait::NoWait`, the default).
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if called back from a plug-in, or a lock is poisoned.
    pub fn set_lock_wait(&self, wait: LockWait) -> Result<()> {
        self.set_setting("LOCK_WAIT", &Value::Char(settings::lock_wait_word(wait).to_owned()))
    }

    /// What happens to an open transaction when a session ends: the choice is
    /// implementor-defined [X3.135-1986 §7.1 GR2]. The default is Commit, so
    /// that a program that ends normally keeps its work.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if called back from a plug-in, or a lock is poisoned.
    pub fn set_session_end(&self, end: SessionEnd) -> Result<()> {
        self.set_setting("SESSION_END", &Value::Char(settings::session_end_word(end).to_owned()))
    }

    /// \[DESIGN\] The log transactions begun from now on write to (RULES.md
    /// D25): their commit point. By default there is none, and nothing is
    /// logged, as engines that keep nothing between runs need none; a
    /// `MemoryLog` keeps every record until truncated. Transactions already
    /// open keep the log they began with.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if called back from a plug-in, or a lock is poisoned.
    pub fn set_log(&self, log: Arc<dyn Log>) -> Result<()> {
        self.change(|st| {
            st.log = Some(log);
            Ok(())
        })
    }

    /// How many transactions are open, across all sessions.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a lock is poisoned.
    pub fn active_transactions(&self) -> Result<usize> {
        Ok(self.snapshot()?.tx.active())
    }

    /// \[DESIGN\] The tables a failed undo has damaged, sorted by name. Every
    /// statement on one is refused with SQLCODE -914 (RULES.md D19).
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a lock is poisoned.
    pub fn damaged_tables(&self) -> Result<Vec<QualifiedName>> {
        Ok(self.snapshot()?.damaged_tables())
    }

    /// \[DESIGN\] Repairs `table`, "AUTH.TABLE", damaged by a failed undo
    /// (RULES.md D19): if only its index entries could not be put back, its
    /// rows are as they should be, and its indexes are built from them
    /// again, under its exclusive lock, so no transaction writes it
    /// meanwhile; it then takes statements again. A table not damaged needs
    /// nothing.
    ///
    /// # Errors
    ///
    /// SQLCODE -914 if its rows could not be put back: what they should be
    /// is the last checkpoint's and the log's, which opening a database
    /// kept in a directory again recovers; one kept nowhere else has no log,
    /// and the table can only be dropped. SQLCODE -911 if a transaction
    /// holds the table (under `LockWait::NoWait`); -910 if there is no such
    /// table; an index method's errors, building.
    pub fn repair_table(&self, table: &str) -> Result<()> {
        let name = qualified_name(table)?;
        let current = self.snapshot()?;
        let lock = Arc::clone(current.lock_of(&name)?);
        match lock.damage() {
            None => Ok(()),
            Some(Damage::Rows) => {
                let how = if self.files()?.is_some() {
                    "open the database again: it is recovered from its last checkpoint and its log"
                } else {
                    "a database kept nowhere has no log to recover them from: drop the table"
                };
                Err(SqlError::new(Code::Damaged, format!("[DESIGN] the rows of {name} could not be put back; to repair it, {how}")))
            }
            Some(Damage::Indexes) => {
                let tx = current.begin();
                current.acquire(&tx, &name, LockMode::Exclusive)?;
                let _gate = current.tx.gate.enter();
                self.change(|st| st.rebuild_indexes_where(|ix| ix.table == name))?;
                lock.repaired();
                Ok(())
            }
        }
    }

    /// \[DESIGN\] Has the engine of `table`, "AUTH.TABLE", gather its
    /// statistics anew, for the planner's estimates (`stats.rs`). An engine
    /// may also gather them by itself; the row store does as the table
    /// changes. Not part of any transaction: it reads the newest committed
    /// rows.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if `table` is not "AUTH.TABLE" or has no storage; the
    /// engine's errors, the statistics then left as they were.
    pub fn analyze(&self, table: &str) -> Result<()> {
        let name = qualified_name(table)?;
        self.snapshot()?.storage_of(&name)?.analyze()
    }

    /// Start a session whose authorization identifier plays the role of the
    /// `<module authorization identifier>` [X3.135-1986 §5.4 SR2b, §5.6 GR4, §6.6 SR3].
    pub fn session(&self, auth: &str) -> Session {
        Session::new(self.clone(), auth)
    }

    /// \[DESIGN\] The entries index `name` holds, each (key, row) pair once:
    /// a diagnostic. An engine keeping versions has one per version a
    /// reader may still see.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if there is no such index; the index's errors.
    pub fn index_entries(&self, name: &str) -> Result<usize> {
        Ok(self.snapshot()?.index_of(name)?.search(&Probe::All)?.len())
    }
}

// Unit tests: what a session does when the snapshot cannot be taken
// (RULES.md R-TEST-2 robustness).
#[cfg(test)]
mod tests {
    use std::thread;

    use super::*;

    /// Verifies: [DESIGN] RULES.md D27: if a thread panicked while publishing
    /// a snapshot, every call reports SQLCODE -910 (the definitions cannot be
    /// trusted), and a failed call does not leave its session marked busy.
    #[test]
    fn a_poisoned_snapshot_is_reported() {
        let db = Database::new();
        let shared = Arc::clone(&db.0);
        let poisoned = thread::spawn(move || {
            let _held = shared.current.write();
            panic!("a thread panicked while publishing");
        });
        assert!(poisoned.join().is_err());
        let s = db.session("A");
        for attempt in ["first call", "second call"] {
            let failed = s.run("COMMIT WORK").err();
            assert!(failed.is_some_and(|e| e.code == Code::Api && e.msg.contains("panicked")), "{attempt}: not refused as busy");
        }
    }
}

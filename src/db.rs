//! The database, its extension points, and sessions.
//!
//! What SQL-86 defines is reached through SQL text:
//!   * `Database::schema` processes a `<schema>` [X3.135-1986 §6.1];
//!   * `Session::declare_cursor` processes a `<declare cursor>` [X3.135-1986 §8.3];
//!   * `Session::execute` processes any `<SQL statement>` [X3.135-1986 §7.3]:
//!     CLOSE, COMMIT WORK, DELETE (positioned or searched), FETCH, INSERT,
//!     OPEN, ROLLBACK WORK, SELECT ... INTO, UPDATE (positioned or searched).
//!
//! \[DESIGN\] A `Session` plays the part of an application program bound to a
//! `<module>`: it has the module's authorization identifier, its cursors, and
//! its current transaction. How a program is bound to a module is left to
//! the implementation [X3.135-1986 §3.4], and so is this API.
//!
//! What SQL-86 leaves to the implementation is reached through Rust:
//!
//! | trait               | built in              | used by                          |
//! |---------------------|-----------------------|----------------------------------|
//! | `Catalog`           | `MemoryCatalog`         | binder (names, types, owners)    |
//! | `StorageEngine`     | rowstore              | planner, executor, transactions  |
//! | `IndexMethod`       | btree, hash           | planner, UNIQUE \[EXTENSION\]      |
//! | `AggregateFunction` | COUNT SUM AVG MAX MIN | set functions [X3.135-1986 §5.8] |
//! | `ScalarFunction`    | SQL-92 string functions | \[EXTENSION\] call syntax       |
//! | `Rule`              | ten rewrite rules     | optimizer                        |
//! | `Diagnostics`       | none: events dropped  | the database's events (RULES.md D40) |
//! | `Spill`             | `PagedSpill`          | operators past WORK_MEMORY (RULES.md D38) |
//! | `Credentials`       | none: no password kept | CREATE and ALTER USER (RULES.md D43) |
//! | `SqlType`           | CHARACTER NUMERIC FLOAT | data types (RULES.md D53)      |
//!
//! Plus the settings (`settings.rs`, RULES.md D36): each declared once in a
//! registry, plug-ins' among them; the database's values, kept with its
//! definitions, and each session's own (`db/settings.rs`).
//!
//! \[DESIGN\] A `Database` is a cheap handle that sessions share, on any
//! number of threads (RULES.md D27). Each statement runs against a snapshot
//! of the definitions taken when it starts, holding no lock on them while it
//! runs; the data is guarded by the storage engines and indexes, and its
//! isolation by the transaction coordinator's table locks. A `Session` is
//! used by one thread at a time, but may move between threads between calls.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use crate::binder::OutColumn;
use crate::catalog::{QualifiedName, TableDesc};
use crate::error::{Code, Result, SqlError, SqlState, fail};
use crate::logical::LogicalPlan;
use crate::optimizer::RuleContext;
use crate::parser::Parser;
use crate::physical::{PhysicalPlanner, Plan};
use crate::settings::{LOCK_TIMEOUT, STATEMENT_TIMEOUT};
use crate::sync::lock_builtin;
use crate::txn::{SessionEnd, Stop, TxId, Txn, Window};

mod alter;
mod area;
mod assertions;
mod assignment;
mod catalog_file;
mod checkpoint;
mod clock;
mod completion;
mod constraints;
mod cursor;
mod database;
mod ddl;
mod declared;
mod defaults;
mod deferral;
mod definitions;
mod direct;
mod domains;
mod drop;
mod execute;
mod explain;
mod files;
mod formats;
mod image;
mod index_statements;
mod indexes;
mod integrity;
mod position;
mod prepared;
mod recovery;
mod revoke;
mod schema;
mod scroll;
mod sealing;
mod session;
mod settings;
mod slow;
mod snapshot;
mod state;
mod stores;
mod stream;
mod temporary;
mod time_zone;
mod types;
mod usage;
mod users;
mod views;
mod write;

pub use prepared::Prepared;
use scroll::{Kept, Updatable};
pub use state::State;
use stream::Stream;
pub use stream::{Cursor, Direct};
pub use users::Credentials;

use assignment::{assign_targets, candidate, check_option, check_targets, set_columns};
pub use catalog_file::{COMPACT_AFTER, CatalogFiles};
pub use completion::{Completion, SQLCODE_NO_DATA};
pub use database::Database;
pub use files::{CHECKPOINT_AFTER, CHECKPOINT_BYTES, OpenOptions, Opened, POOL_BYTES};
pub use formats::FileFormats;
pub use image::IMAGE_FORMAT;
use integrity::Deferral;
pub use recovery::RecoveryReport;
pub use session::TransactionMode;
pub use stores::{CatalogStorage, CatalogStore, FileLogs, LogStorage, StorePlace};

/// The state and transaction a statement runs in.
#[derive(Clone, Copy)]
pub(super) struct StatementContext<'a> {
    pub(super) st: &'a Arc<State>,
    pub(super) tx: &'a Txn,
}

fn api(msg: impl Into<String>) -> SqlError {
    SqlError::new(Code::Api, msg)
}

/// \[DESIGN\] The table that `table`, written "AUTH.TABLE", names. The Rust
/// API takes table names in this one form.
///
/// # Errors
///
/// SQLCODE -910 if `table` has no ".".
fn qualified_name(table: &str) -> Result<QualifiedName> {
    let (auth, ident) = table.split_once('.').ok_or_else(|| api(format!("[DESIGN] the table {table:?} must be written AUTH.TABLE")))?;
    Ok(QualifiedName { auth: auth.into(), ident: ident.into() })
}

// ===========================================================================
// Sessions
// ===========================================================================

/// [X3.135-1986 §4.12] a cursor is in the open or the closed state.
struct DeclaredCursor {
    specification: String,
    /// INSENSITIVE and SCROLL, as declared [ISO/IEC 9075:1992 §13.1].
    insensitive: bool,
    scroll: bool,
    /// Whether it is updatable [X3.135-1986 §8.3 SR4; ISO/IEC 9075:1992
    /// §13.1 SR6], once known: at DECLARE if the text parses without the
    /// opening procedure's parameters, and always at OPEN.
    updatable: Option<bool>,
    open: Option<OpenCursor>,
}

struct OpenCursor {
    columns: Vec<OutColumn>,
    rows: CursorRows,
}

/// Whether opening a cursor creates a "working base table" or a "working
/// viewed table" is implementor-defined [X3.135-1986 §4.12].
enum CursorRows {
    /// A read-only cursor's table is created when the cursor is opened
    /// [X3.135-1986 §8.8 GR3b]: a working base table, whose rows are read
    /// as the cursor is fetched from where that gives the same rows
    /// (`db/stream.rs`).
    Table(Stream),
    /// A read-only SCROLL cursor's: read as `Table`'s are, and kept as
    /// read, so FETCH may move back (`db/scroll.rs`).
    Kept(Kept),
    /// An updatable cursor's: a working viewed table (`db/scroll.rs`).
    Updatable(Box<Updatable>),
}

#[derive(Default)]
struct SessionState {
    tx: Option<Arc<Txn>>,
    cursors: HashMap<String, DeclaredCursor>,
    /// The rows of cursors opened with `open_cursor`, shared with the
    /// cursors: settled before the transaction changes a table they read,
    /// taken away when it ends.
    anonymous: Vec<Weak<Mutex<Option<Stream>>>>,
    /// The definitions its transaction changed, until it ends (`db/ddl.rs`).
    staged: Option<ddl::Staged>,
    /// Its diagnostics area (`db/area.rs`).
    area: area::Area,
}

/// \[DESIGN\] An application program bound to a `<module>`: an authorization
/// identifier, the module's cursors, and the current transaction.
pub struct Session {
    db: Database,
    /// Its authorization identifier [ISO/IEC 9075:1992 §4.30], which SET
    /// SESSION AUTHORIZATION sets [§16.4].
    auth: RefCell<String>,
    /// The authorization identifier it began with [§16.4 GR4].
    started: String,
    extensions: bool,
    inner: RefCell<SessionState>,
    /// Set while a call runs, so a plug-in calling back into this session is
    /// refused (RULES.md R-ERR-4).
    busy: Cell<bool>,
    /// The constraint modes of its transaction, and the checks of its
    /// deferred constraints waiting [ISO/IEC 9075:1992 §14.2].
    deferral: RefCell<Deferral>,
    /// Its own values of the settings a session may set (RULES.md D36).
    own: RefCell<crate::settings::Own>,
    /// The default schema of its unqualified names [ISO/IEC 9075:1992
    /// §4.30, §16.2].
    schema: RefCell<String>,
    /// The modes of its active transaction, and of the next [§14.1].
    mode: Cell<TransactionMode>,
    next: Cell<TransactionMode>,
    /// What cancels its statements (RULES.md D44).
    stop: Arc<Stop>,
    /// Its instances of the temporary tables [ISO/IEC 9075:1992 §4.9].
    temporaries: temporary::Temporaries,
    /// The cursor whose row a referential action of the statement running
    /// changed, if one did [ISO/IEC 9075:1992 §11.8 GR8].
    conflict: RefCell<Option<String>>,
    /// The instant of the statement running, and the session's time zone
    /// [ISO/IEC 9075:1992 §6.8 GR3, §4.5.1], set as it begins.
    now: Cell<crate::value::datetime::Now>,
}

/// One call on a session: the snapshot it runs against, and the session
/// marked busy until the call ends.
struct Call<'a> {
    session: &'a Session,
    snapshot: Arc<State>,
    /// The transaction this call runs on this thread, once it has one.
    running: RefCell<Option<Arc<Txn>>>,
    /// Open while the call runs: what it runs may be cancelled.
    _window: Window<'a>,
    /// The statement it runs, timed, to tell if it is slow (`db/slow.rs`).
    timed: Option<slow::Timed<&'a str>>,
}

impl Call<'_> {
    fn state(&self) -> &Arc<State> {
        &self.snapshot
    }

    /// Records that this call runs transaction `tx` on this thread, so that
    /// another transaction never waits on this thread for its locks
    /// (`txn/locks.rs`).
    fn runs(&self, tx: &Arc<Txn>) {
        tx.runs_on(Some(std::thread::current().id()));
        *self.running.borrow_mut() = Some(Arc::clone(tx));
    }
}

impl Drop for Call<'_> {
    fn drop(&mut self) {
        if let Some(tx) = self.running.borrow_mut().take() {
            tx.runs_on(None);
        }
        self.session.busy.set(false);
        if let Some(timed) = self.timed.take() {
            timed.done(&self.session.db);
        }
    }
}

fn rule(msg: impl Into<String>) -> SqlError {
    SqlError::new(Code::SyntaxRule, msg)
}

fn cursor_error<T>(name: &str, msg: impl Into<String>) -> Result<T> {
    // [ISO/IEC 9075:1992 §18.1 GR3h] about the cursor.
    Err(SqlError::new(Code::Cursor, msg).about(crate::error::Subject::cursor(name)))
}

/// Optimize a logical plan and choose its physical plan.
fn plan(st: &State, logical: LogicalPlan) -> (LogicalPlan, Vec<&'static str>, Plan) {
    let (optimized, trace) = st.optimizer.optimize(logical, &RuleContext { db: st });
    let physical = PhysicalPlanner::new(st).plan(&optimized);
    (optimized, trace, physical)
}

fn table_desc(st: &State, name: &QualifiedName) -> Result<TableDesc> {
    st.catalog.table(name).cloned().ok_or_else(|| rule(format!("[X3.135-1986 §5.4 SR5] table {name} is not defined")))
}

impl Session {
    /// Its authorization identifier, as it is now.
    pub(crate) fn auth(&self) -> std::cell::Ref<'_, String> {
        self.auth.borrow()
    }

    fn new(db: Database, auth: &str) -> Session {
        Session {
            db,
            auth: RefCell::new(auth.to_string()),
            started: auth.to_string(),
            extensions: false,
            inner: RefCell::new(SessionState::default()),
            busy: Cell::new(false),
            deferral: RefCell::new(Deferral::default()),
            own: RefCell::new(crate::settings::Own::default()),
            schema: RefCell::new(auth.to_owned()),
            mode: Cell::new(TransactionMode::default()),
            next: Cell::new(TransactionMode::default()),
            stop: Arc::default(),
            temporaries: temporary::Temporaries::default(),
            conflict: RefCell::new(None),
            now: Cell::default(),
        }
    }

    /// Starts a call: marks the session busy and takes the current snapshot.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a call on this session is already running (a plug-in
    /// called back into it), or a lock is poisoned.
    fn enter(&self) -> Result<Call<'_>> {
        let already_busy = self.busy.replace(true);
        if already_busy {
            return fail(Code::Api, "[DESIGN] the session is already running a call: a plug-in called back into it (RULES.md R-ERR-4)");
        }
        match self.db.snapshot().and_then(|published| self.definitions(published)) {
            Ok(snapshot) => {
                let snapshot = self.with_own(self.with_temporaries(snapshot));
                self.now.set(snapshot.now(&self.own.borrow()));
                let millis = |key| u64::try_from(snapshot.settings.integer(key)).unwrap_or(0);
                self.stop.set_timeout(millis(STATEMENT_TIMEOUT));
                self.stop.set_lock_timeout(millis(LOCK_TIMEOUT));
                Ok(Call { session: self, snapshot, running: RefCell::new(None), _window: self.stop.window(), timed: None })
            }
            Err(e) => {
                self.busy.set(false);
                Err(e)
            }
        }
    }

    /// \[EXTENSION\] Accept the `<identifier>`(`<args>`) function call syntax.
    /// Off by default, so a session processes only SQL-86.
    pub fn enable_extensions(&mut self, on: bool) {
        self.extensions = on;
    }

    /// The current transaction, if one is active.
    pub fn transaction(&self) -> Option<TxId> {
        self.inner.borrow().tx.as_ref().map(|tx| tx.id)
    }

    /// [X3.135-1986 §7.3 GR2a] "If no transaction is active for that agent,
    /// then a transaction is effectively initiated".
    fn ensure_transaction(&self, st: &State) -> Arc<Txn> {
        let wait = self.lock_wait(st);
        let mut inner = self.inner.borrow_mut();
        if inner.tx.is_none() {
            self.begin_mode();
        }
        let reading = self.mode.get().reading();
        Arc::clone(inner.tx.get_or_insert_with(|| Arc::new(st.begin_waiting((wait, reading, Arc::clone(&self.stop))))))
    }

    /// Take the current transaction, closing every cursor it opened
    /// [X3.135-1986 §4.12, §8.2 GR2, §8.9 GR2].
    fn end_transaction(&self) -> Option<Arc<Txn>> {
        // [ISO/IEC 9075:1992 §14.2 GR1] Its constraint modes were its own.
        *self.deferral.borrow_mut() = Deferral::default();
        let mut inner = self.inner.borrow_mut();
        for c in inner.cursors.values_mut() {
            c.open = None;
        }
        for rows in inner.anonymous.drain(..) {
            if let Some(rows) = rows.upgrade() {
                *lock_builtin(&rows) = None;
            }
        }
        inner.tx.take()
    }

    /// [X3.135-1986 §8.2] COMMIT WORK.
    ///
    /// # Errors
    ///
    /// A participant could not commit, and the transaction was rolled back;
    /// or SQLCODE -910 if called back from a plug-in.
    pub fn commit(&self) -> Result<()> {
        let call = self.enter()?;
        self.emptied(("COMMIT WORK", ""));
        self.diagnosed(self.db.noted(self.commit_in(&call)), |()| SqlState::SUCCESSFUL_COMPLETION)
    }

    fn commit_in(&self, call: &Call<'_>) -> Result<()> {
        let st = call.state();
        let open = self.inner.borrow().tx.clone();
        if let Some(tx) = open
            && let Err(e) = self.check_at_commit(call, &tx)
        {
            // [ISO/IEC 9075:1992 §14.3 GR5a] its changes canceled.
            self.rollback_in(st)?;
            return Err(e);
        }
        let staged = self.take_staged();
        let ended = match (self.end_transaction(), staged) {
            (Some(tx), None) => st.commit(&tx),
            (Some(tx), Some(staged)) => self.db.commit_staged(&tx, &staged),
            (None, _) => return Ok(()),
        };
        self.temporaries_ended();
        // The transaction has ended: a checkpoint may come due.
        self.db.checkpoint_if_due();
        ended
    }

    /// [X3.135-1986 §8.9] ROLLBACK WORK.
    ///
    /// # Errors
    ///
    /// SQLCODE -914 if a change could not be undone; the transaction still ends,
    /// and the tables concerned are damaged (RULES.md D19). SQLCODE -910 if
    /// called back from a plug-in.
    pub fn rollback(&self) -> Result<()> {
        let call = self.enter()?;
        self.emptied(("ROLLBACK WORK", ""));
        self.diagnosed(self.db.noted(self.rollback_in(call.state())), |()| SqlState::SUCCESSFUL_COMPLETION)
    }

    fn rollback_in(&self, st: &State) -> Result<()> {
        // The changes of definitions are forgotten; the published ones have
        // the storage of every table its changes are undone in.
        let ended = match (self.end_transaction(), self.take_staged()) {
            (Some(tx), None) => st.rollback(&tx),
            (Some(tx), Some(staged)) => {
                staged.discard();
                self.db.snapshot().and_then(|published| published.rollback(&tx))
            }
            (None, _) => return Ok(()),
        };
        self.temporaries_ended();
        self.db.checkpoint_if_due();
        ended
    }

    /// [X3.135-1986 §8.3] `<declare cursor>`. In SQL-86 cursors belong to the
    /// `<module>` [X3.135-1986 §7.1]; here, to the session.
    ///
    /// # Errors
    ///
    /// The declaration does not parse, or the cursor is already declared.
    pub fn declare_cursor(&self, sql: &str) -> Result<()> {
        let _call = self.enter()?;
        let d = Parser::new(sql)?.declare_cursor()?;
        let mut inner = self.inner.borrow_mut();
        if inner.cursors.contains_key(&d.name) {
            return Err(rule(format!("[X3.135-1986 §8.3 SR1] cursor {} is already declared", d.name)));
        }
        // Known now only if the text parses without the opening procedure's
        // parameters; otherwise it is parsed, and any error reported, at OPEN.
        let parsed = Parser::new(&d.specification).and_then(|mut p| p.cursor_specification()).ok();
        let updatable = parsed.map(|c| cursor::declared_updatable(&c, (d.insensitive, d.scroll))).transpose()?;
        let declared =
            DeclaredCursor { specification: d.specification, insensitive: d.insensitive, scroll: d.scroll, updatable, open: None };
        inner.cursors.insert(d.name, declared);
        Ok(())
    }
}

/// [X3.135-1986 §7.1 GR2] when the program stops calling, a COMMIT or a
/// ROLLBACK is performed; which one is implementor-defined (`SessionEnd`).
impl Drop for Session {
    fn drop(&mut self) {
        // A session dropped by a plug-in while a call on it runs cannot end
        // its transaction here.
        let Ok(call) = self.enter() else { return };
        let st = call.state();
        // Drop cannot return an error. A failed commit has rolled back, and a
        // failed undo has marked its tables damaged, so neither goes unnoticed.
        let _ = match self.session_end(st) {
            SessionEnd::Commit => self.commit_in(&call),
            SessionEnd::Rollback => self.rollback_in(st),
        };
    }
}

// Unit tests: a session refuses a call back into itself (RULES.md R-ERR-4,
// R-TEST-2 robustness).
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md R-ERR-4: a call into a session already
    /// running a call, as a plug-in calling back into it would make, is
    /// refused with SQLCODE -910; once the first call ends the session works.
    #[test]
    fn a_session_refuses_a_call_back_into_itself() {
        let db = Database::new();
        let s = db.session("A");
        let call = s.enter().unwrap();
        assert_eq!(s.enter().err().map(|e| e.code), Some(Code::Api));
        drop(call);
        assert!(s.enter().is_ok());
    }
}

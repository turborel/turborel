//! The database, its extension points, and sessions.
//!
//! What SQL-86 defines is reached through SQL text:
//!   * `Database::schema` processes a `<schema>` [X3.135-1986 §6.1];
//!   * `Session::declare_cursor` processes a `<declare cursor>` [X3.135-1986 §8.3];
//!   * `Session::execute` processes any `<SQL statement>` [X3.135-1986 §7.3]:
//!     CLOSE, COMMIT WORK, DELETE (positioned or searched), FETCH, INSERT,
//!     OPEN, ROLLBACK WORK, SELECT ... INTO, UPDATE (positioned or searched).
//!
//! \[DESIGN\] A `Session` plays an application program bound to a `<module>`, with its authorization
//! identifier, cursors and current transaction. The binding is left to the implementation [X3.135-1986 §3.4], as is this API.
//!
//! What SQL-86 leaves to the implementation is reached through Rust:
//!
//! | trait               | built in              | used by                          |
//! |---------------------|-----------------------|----------------------------------|
//! | `Catalog`           | `MemoryCatalog`         | binder (names, types, owners)    |
//! | `StorageEngine`     | rowstore, columnstore | planner, executor, transactions  |
//! | `IndexMethod`       | btree, hash           | planner, UNIQUE \[EXTENSION\]      |
//! | `AggregateFunction` | COUNT SUM AVG MAX MIN | set functions [X3.135-1986 §5.8] |
//! | `ScalarFunction`    | SQL-92 string functions | standard syntax, \[EXTENSION\] call syntax |
//! | `Rule`              | eleven rewrite rules  | optimizer                        |
//! | `Diagnostics`       | none: events dropped  | the database's events (RULES.md D40) |
//! | `Spill`             | `PagedSpill`          | operators past MEMORY.WORK_BYTES (RULES.md D38) |
//! | `Credentials`       | none: no password kept | CREATE and ALTER USER (RULES.md D43) |
//! | `ServedConnections` | none: no connection served | END and CANCEL CONNECTION (RULES.md D42) |
//! | `SqlType`           | SQL's standard types  | data types (RULES.md D53)        |
//! | `Collation`         | UCS_BASIC, UNICODE and others | comparison of strings (RULES.md D60) |
//! | `XQueryFunction`    | the core of fn:       | XQuery's function calls (RULES.md D106) |
//! | `Log`               | file log, memory log  | commit and recovery (RULES.md D25) |
//! | `LogStorage`, `CatalogStorage` | files in the directory | opening a database (RULES.md D25) |
//! | `SystemTable`       | the Information Schema's | system tables (RULES.md D37)  |
//! | `Clock`             | the system's          | datetime functions (RULES.md D54) |
//! | `BackgroundJob`     | none                  | plug-ins' jobs (RULES.md D55)    |
//! | `StateSource`       | `Database::export`    | state transfer (RULES.md D57)    |
//! | `Ranking`           | BM25                  | full-text Rank (RULES.md D87)    |
//! | `VectorMetric`, `Quantizer` | COSINE and others; PQ, SQ8, BQ | VECTOR_DISTANCE, vector indexes (RULES.md D93) |
//! | `PartitionStrategy` | RANGE, LIST, HASH     | partitioned tables (RULES.md D154) |
//! | `BackupTarget`, `TargetProvider` | a directory, memory | backups and the archive (RULES.md D166) |
//! | `Eviction`          | CLOCK                 | the data file's buffer pool (`set_eviction`, RULES.md D283) |
//! | `Kernels`           | `Portable`            | batches' primitives (`register_kernels`, RULES.md D354, D460) |
//!
//! Plus the settings (`settings.rs`, RULES.md D36), plug-ins' too, per database and session.
//! \[DESIGN\] A `Database` is a cheap handle that sessions share, on any number of threads (RULES.md D27). Each statement runs
//! against a snapshot of the definitions taken when it starts, holding no lock on them while it runs; the data is guarded by the
//! storage engines and indexes, and its isolation by the engine, or by the transaction coordinator's table locks for an engine that
//! leaves it to them (RULES.md D29). A `Session` is used by one thread at a time, and may move between threads between calls.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use self::area::commands;
use crate::binder::OutColumn;
use crate::catalog::{Authorization, QualifiedName};
use crate::error::{Code, Result, SqlError, SqlState, fail};
use crate::settings::{COMMIT_SYNC, LOCKS_TIMEOUT_MS, STATEMENT_TIMEOUT_MS};
use crate::txn::{SessionEnd, Stop, TxId, Txn, Window};

mod alter;
mod area;
mod assertions;
mod assignment;
mod background;
pub mod backup;
mod catalog_file;
mod checkpoint;
mod clock;
mod collations;
mod compiled;
mod completion;
mod configured;
mod constraints;
mod contents;
mod cursor;
mod database;
mod ddl;
mod declared;
mod defaults;
mod deferral;
mod definitions;
mod delta;
mod direct;
mod domains;
mod drop;
mod embedding;
mod escrow;
pub(crate) use embedding::model_named;
pub(crate) mod dynamic;
mod execute;
mod explain;
mod export;
mod extensions;
mod files;
mod formats;
mod functions;
mod generated;
mod graphs;
mod hosted;
mod image;
mod index_statements;
mod indexes;
mod integrity;
pub(crate) mod kernels;
mod lobs;
mod log_sync;
mod long_texts;
mod maintenance;
mod merge;
mod modules;
mod nested;
mod observed;
pub(crate) mod partitions;
mod position;
pub(crate) mod prepared;
pub(crate) mod ptf;
mod rankings;
mod recovery;
mod rekey;
mod revoke;
mod roles;
mod rollback;
mod routines;
mod running;
pub(crate) mod savepoints;
mod schema;
mod scroll;
mod sealing;
mod sequences;
mod served;
mod session;
mod settings;
mod slow;
mod snapshot;
mod staged_schema;
mod standby;
mod state;
mod stores;
mod stream;
mod structured;
mod temporary;
mod time_zone;
mod triggers;
mod truncate;
mod typed;
mod types;
mod unnamed;
mod usage;
mod user_types;
mod users;
mod vectors;
mod views;
pub(crate) mod write;

use assignment::{candidate, check_option, set_columns};
pub use catalog_file::{COMPACT_AFTER, CatalogFiles};
use compiled::{plan, plan_heard, plan_slotted, plan_written, plan_written_slotted};
pub use lobs::LOB_READ_OCTETS;
pub use prepared::Prepared;
pub use rekey::FormatsBehind;
use scroll::{Kept, Updatable};
pub use served::ServedConnections;
pub use state::State;
use stream::Stream;
pub use stream::{Cursor, Direct};
pub use users::Credentials;

/// \[DESIGN\] The default of STORAGE.REKEY_BYTES_PER_SECOND: 8 MiB a second,
/// a pace a disk bears beside the database's own work (RULES.md D48).
pub const REKEY_BYTES_PER_SECOND: u64 = 8 << 20;
pub use checkpoint::OVERDUE;
pub use completion::{Completion, SQLCODE_NO_DATA};
pub use database::{Database, EndedCounts, TransactionsEnded};
pub use export::{ExportedFile, RECORDS_FILE, StateSource, decode_records};
pub use files::{CHECKPOINT_AFTER, CHECKPOINT_BYTES, OpenOptions, Opened, POOL_BYTES};
use integrity::Deferral;
pub use log_sync::SYNC_AFTER_MS;
pub use session::TransactionMode;
pub use standby::Standby;
pub use stores::{CatalogStorage, CatalogStore, FileLogs, LOG_DIR, LogStorage, OpenedLog, StorePlace};
pub(crate) use {
    assignment::{assign_targets, assignment_type, check_targets},
    database::WeakDatabase,
    routines::run_unhosted,
    triggers::trigger_texts,
};
pub use {formats::FileFormats, image::IMAGE_FORMAT, indexes::AccuracyReport, recovery::RecoveryReport};

/// The state and transaction a statement runs in.
#[derive(Clone, Copy)]
pub(super) struct StatementContext<'a> {
    pub(super) st: &'a Arc<State>,
    pub(super) tx: &'a Txn,
}

fn api(msg: impl Into<String>) -> SqlError {
    SqlError::new(Code::Api, msg)
}

/// \[DESIGN\] The table that `table`, written "AUTH.TABLE", names. The Rust API takes table names in this one form.
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
    /// Of a dynamic cursor, the statement prepared that is its specification [ISO/IEC 9075:1992 §17.12, §17.13].
    statement: Option<dynamic::Name>,
}

struct OpenCursor {
    columns: Arc<[OutColumn]>,
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
    cursors: HashMap<dynamic::Name, DeclaredCursor>,
    /// The rows of cursors opened with `open_cursor`, shared with the cursors: settled before the transaction changes a table
    /// they read, taken away when it ends.
    anonymous: Vec<Weak<Mutex<Option<Stream>>>>,
    /// The definitions its transaction changed, until it ends (`db/ddl.rs`).
    staged: Option<ddl::Staged>,
    /// Its diagnostics area (`db/area.rs`).
    area: area::Area,
    /// Its prepared statements and SQL descriptor areas (`db/dynamic.rs`).
    dynamic: dynamic::DynamicState,
    /// [ISO/IEC 9075-2:2023 §7.6] Where the change running records its delta table's rows (`db/delta.rs`).
    delta: Option<delta::Collector>,
    /// \[EXTENSION\] The rows it gave in place of estimates (`db/explain/injected.rs`).
    injected: Option<Arc<crate::physical::Injected>>,
    /// The procedures' bodies it keeps (`db/routines/kept.rs`).
    routines: routines::KeptRoutines,
    /// What its transactions begin from, while its snapshots give the same
    /// (`txn/begin.rs`, RULES.md D844).
    origin: Option<Arc<crate::txn::Origin>>,
}

/// \[DESIGN\] An application program bound to a `<module>`: an authorization
/// identifier, the module's cursors, and the current transaction.
pub struct Session {
    db: Database,
    /// The service of its database it began in (`database/service.rs`).
    service: u64,
    /// Its current authorization identifier [ISO/IEC 9075:1992 §4.30], which
    /// SET SESSION AUTHORIZATION sets [§16.4]: of the top cell of its
    /// authorization stack, a trigger's or a procedure's owner as it runs.
    auth: RefCell<String>,
    /// [ISO/IEC 9075-2:2023 §4.42.1.2, §19.3] The current role name, which
    /// SET ROLE sets: none in a trigger's or a procedure's owner's cell.
    role: RefCell<Option<String>>,
    /// [ISO/IEC 9075-2:2023 §4.42.1.2, §19.2 GR6] The SQL-session user
    /// identifier, SESSION_USER, which no trigger or procedure changes.
    user_identifier: RefCell<String>,
    /// The authorization identifier it began with [§16.4 GR4].
    started: String,
    inner: RefCell<SessionState>,
    /// Set while a call runs, so a plug-in calling back into this session is
    /// refused (RULES.md R-ERR-4).
    busy: Cell<bool>,
    /// The constraint modes of its transaction, and the checks of its
    /// deferred constraints waiting [ISO/IEC 9075:1992 §14.2].
    deferral: RefCell<Deferral>,
    /// Its copy of the last snapshot it took (`database/published.rs`).
    published: RefCell<database::Own>,
    /// Its own values of the settings a session may set (RULES.md D36).
    own: RefCell<settings::OwnValues>,
    /// The default schema of its unqualified names [ISO/IEC 9075:1992
    /// §4.30, §16.2].
    schema: RefCell<String>,
    /// [ISO/IEC 9075-2:2023 §19.10] Its SQL-session collation, if set.
    collation: RefCell<Option<QualifiedName>>,
    /// The collation a prepared statement run now was prepared under, in
    /// place of `collation`, until it ends (`db/prepared.rs`).
    prepared_collation: RefCell<collations::Collating>,
    /// The modes of its active transaction, and those SET TRANSACTION gave the next [§14.1].
    mode: Cell<TransactionMode>,
    next: Cell<Option<TransactionMode>>,
    /// What cancels its statements (RULES.md D44), and hears the warnings
    /// they raise as they run [ISO/IEC 9075:1992 §3.3.4.1].
    stop: Arc<Stop>,
    /// Its instances of the temporary tables [ISO/IEC 9075:1992 §4.9].
    temporaries: temporary::Temporaries,
    /// The cursor whose row a referential action of the statement running
    /// changed, if one did [ISO/IEC 9075:1992 §11.8 GR8].
    conflict: RefCell<Option<String>>,
    /// The instant of the statement running, and the session's time zone
    /// [ISO/IEC 9075:1992 §6.8 GR3, §4.5.1], set as it begins.
    now: Cell<crate::value::datetime::Now>,
    /// The transaction its running call runs on this thread, once it has
    /// one (`db/running.rs`).
    running: RefCell<Option<Arc<Txn>>>,
    /// [ISO/IEC 9075-2:2023 §15.21] The triggered actions it is executing,
    /// and their plans (`db/triggers/run.rs`).
    triggering: triggers::Triggering,
    /// [ISO/IEC 9075-2:2023 §15.21 GR6, §9.18 GR6] The contexts of the
    /// triggers and procedures it is executing (`db/nested.rs`).
    nesting: nested::Nesting,
    /// [ISO/IEC 9075-2:2023 §9.24] The database and transaction of the
    /// statement running, which a PTF describe component procedure reads.
    compile: RefCell<Option<crate::binder::Compile>>,
    /// [ISO/IEC 9075-2:2023 §4.37.5] Its large object locators
    /// (`db/lobs.rs`).
    locators: RefCell<lobs::Locators>,
    /// The tables of the INSERTs it ran directly, resolved (`db/write/targets.rs`).
    targets: RefCell<write::Targets>,
}

/// One call on a session: the snapshot it runs against, and the session
/// marked busy until the call ends.
struct Call<'a> {
    session: &'a Session,
    snapshot: Arc<State>,
    /// Open while the call runs: what it runs may be cancelled.
    _window: Window<'a>,
    /// The statement it runs, timed, to tell if it is slow (`db/slow.rs`).
    timed: Option<slow::Timed<&'a str>>,
    /// The settings its calls of plug-ins are told, its session's (D282).
    _settings: Option<crate::plugin::settings::Calling>,
}

impl Call<'_> {
    fn state(&self) -> &Arc<State> {
        &self.snapshot
    }

    /// Records that this call runs transaction `tx` on this thread, so that
    /// another transaction never waits on this thread for its locks
    /// (`txn/locks.rs`).
    fn runs(&self, tx: &Arc<Txn>) {
        self.session.runs(tx);
    }
}

fn rule(msg: impl Into<String>) -> SqlError {
    SqlError::new(Code::SyntaxRule, msg)
}

fn cursor_error<T>(name: &str, msg: impl Into<String>) -> Result<T> {
    // [ISO/IEC 9075:1992 §18.1 GR3h] about the cursor.
    Err(SqlError::new(Code::Cursor, msg).about(crate::error::Subject::cursor(name)))
}

impl Session {
    /// Its authorization identifier, as it is now.
    pub(crate) fn auth(&self) -> std::cell::Ref<'_, String> {
        self.auth.borrow()
    }

    /// The top cell of its authorization stack and its SQL-session user
    /// identifier, as they are now [ISO/IEC 9075-2:2023 §4.42.1.2].
    pub(crate) fn authorization(&self) -> Authorization {
        let (auth, role, session) = (self.auth.borrow().clone(), self.role.borrow().clone(), self.user_identifier.borrow().clone());
        Authorization { auth, role, session }
    }

    fn new(db: Database, auth: &str) -> Session {
        Session {
            service: db.service(),
            db,
            auth: RefCell::new(auth.to_string()),
            role: RefCell::new(None),
            user_identifier: RefCell::new(auth.to_string()),
            started: auth.to_string(),
            inner: RefCell::new(SessionState::default()),
            busy: Cell::new(false),
            deferral: RefCell::new(Deferral::default()),
            own: RefCell::new(settings::OwnValues::default()),
            schema: RefCell::new(auth.to_owned()),
            collation: RefCell::new(None),
            prepared_collation: RefCell::new(collations::Collating::AsSet),
            mode: Cell::new(TransactionMode::default()),
            next: Cell::new(None),
            stop: Arc::default(),
            temporaries: temporary::Temporaries::default(),
            conflict: RefCell::new(None),
            now: Cell::default(),
            triggering: triggers::Triggering::default(),
            nesting: nested::Nesting::default(),
            compile: RefCell::new(None),
            locators: RefCell::default(),
            targets: RefCell::default(),
            running: RefCell::new(None),
            published: RefCell::new(None),
        }
    }

    /// Starts a call: marks the session busy and takes the current
    /// snapshot. A retired database, its replica removed from its group,
    /// serves no call (RULES.md D57).
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a call on this session is already running (a plug-in
    /// called back into it), or a lock is poisoned; -917 if the database
    /// is retired.
    fn enter(&self) -> Result<Call<'_>> {
        let already_busy = self.busy.replace(true);
        if already_busy {
            return fail(Code::Api, "[DESIGN] the session is already running a call: a plug-in called back into it (RULES.md R-ERR-4)");
        }
        match self.current_state() {
            Ok(snapshot) => {
                self.now.set(snapshot.now(&self.own.borrow()));
                let millis = |key| u64::try_from(snapshot.settings.integer(key)).unwrap_or(0);
                self.stop.set_timeout(millis(STATEMENT_TIMEOUT_MS));
                self.stop.set_lock_timeout(millis(LOCKS_TIMEOUT_MS));
                let settings = (!snapshot.extensions.is_empty()).then(|| extensions::calling(&snapshot));
                Ok(Call { session: self, snapshot, _window: self.stop.window(), timed: None, _settings: settings })
            }
            Err(e) => {
                self.busy.set(false);
                Err(e)
            }
        }
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
            self.begin_mode(st);
        }
        let reading = self.mode.get().reading();
        let SessionState { tx, origin, .. } = &mut *inner;
        Arc::clone(tx.get_or_insert_with(|| Arc::new(st.begin_waiting((wait, reading, Arc::clone(&self.stop)), origin))))
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
                *crate::sync::lock_builtin(&rows) = None;
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
        self.emptied((commands::COMMIT_WORK, commands::UNRECOGNIZED));
        self.diagnosed(self.db.noted(self.committing(call.state())), |()| SqlState::SUCCESSFUL_COMPLETION)
    }

    fn commit_in(&self, st: &Arc<State>) -> Result<()> {
        let open = self.inner.borrow().tx.clone();
        // [DESIGN] RULES.md D452: its increments reserved applied first, so
        // its constraints are checked of the rows it leaves.
        if let Some(tx) = open
            && let Err(e) = st.fold(&tx).and_then(|()| self.check_at_commit(st, &tx))
        {
            // [ISO/IEC 9075:1992 §14.3 GR5a] its changes canceled.
            self.rollback_in(st)?;
            return Err(e);
        }
        let staged = self.take_staged();
        let ending = self.end_transaction();
        let id = ending.as_ref().map(|tx| tx.id);
        let (wrote, tables) = ending.as_ref().map_or((false, Vec::new()), |tx| (tx.wrote(), tx.written_tables()));
        if let Some(tx) = &ending {
            tx.set_sync_commit(st.settings.is_on(COMMIT_SYNC));
        }
        // Whether it committed, though its definitions failed to publish.
        let committed = Cell::new(false);
        let ended = match (&ending, staged) {
            (Some(tx), None) => st.commit(tx),
            (Some(tx), Some(staged)) => self.db.commit_staged(tx, &staged, &committed),
            (None, _) => return Ok(()),
        };
        if let (Some(tx), true) = (ending, self.commit_ended(&ended, committed.get())) {
            self.db.hold_in_doubt(tx);
        }
        self.locators_ended(ended.is_ok() || committed.get());
        self.temporaries_ended(st, id);
        // The transaction has ended. The garbage collected as it did may leave maintenance due; one that wrote
        // may leave a checkpoint, a fold of kept totals, embeddings, or a sync of the log its commit did not wait for, due.
        self.db.maintain_if_due();
        if wrote {
            self.db.written(&tables);
        }
        ended
    }
}

/// [X3.135-1986 §7.1 GR2] when the program stops calling, a COMMIT or a
/// ROLLBACK is performed; which one is implementor-defined (`SessionEnd`).
impl Drop for Session {
    fn drop(&mut self) {
        // A session dropped by a plug-in while a call on it runs cannot end its transaction here.
        let Ok(call) = self.enter() else { return };
        let st = call.state();
        // Drop cannot return an error. A failed commit has rolled back, and a
        // failed undo has marked its tables damaged, so neither goes unnoticed.
        // Of a database closed, or whose files another opened since, it rolls back in memory (RULES.md D56).
        let _ = match (self.session_end(st), st.files.is_fenced()) {
            (SessionEnd::Commit, false) => self.commit_in(st),
            (SessionEnd::Commit, true) | (SessionEnd::Rollback, _) => self.rollback_in(st),
        };
    }
}

// Unit tests: a session refuses a call back into itself (RULES.md R-ERR-4, R-TEST-2 robustness).
#[cfg(test)]
mod tests;

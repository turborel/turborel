//! \[DESIGN\] Diagnostics (RULES.md D40): what the database tells whoever runs
//! it, as typed events rather than printed text, to a sink that is a
//! plug-in (`Diagnostics`). By default events are dropped; a server gives
//! a sink that writes them where its operators read them, and a test one
//! that keeps them.
//!
//! Events are what an operator must know of the database's life and
//! failures: opening and closing, checkpoints taken, and overdue while the
//! log grows (RULES.md D25), changes of settings,
//! plug-ins registered, users and the owner changed, the threads of
//! background work started, stopped or failing (RULES.md D55), failures
//! beyond a statement's own
//! making (a damaged table, corrupt data, an I/O error, a commit that
//! failed, memory refused), and statements slower than
//! DIAGNOSTICS.SLOW_STATEMENT. A failure of a statement's own SQL (a syntax
//! error, a violated constraint, a conflict) is the client's, returned to
//! it, and is no event. Events carry no row data, but for a slow
//! statement's text, and no wall-clock time: the sink stamps them as it
//! takes them. What is counted (I/O, pool hits) stays in
//! counters, read when wanted.
//!
//! A server tells what it does itself through the database it serves
//! (`Database::tell`): where it listens, that it stops, connections it
//! could not take, TLS and logins that failed. DIAGNOSTICS.LEVEL, the
//! database's, says the least level told: events below it are dropped
//! before any sink sees them.

use std::time::Duration;

use crate::db::Opened;
use crate::error::{Code, SqlError};
use crate::threads::{JobPolicy, ThreadChange};
use crate::value::Value;

/// How much an event matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// Something failed that an operator must see to.
    Error,
    /// Something went wrong, and was handled.
    Warning,
    /// The database's life: opened, checkpointed, configured.
    Info,
}

impl Level {
    /// The words DIAGNOSTICS.LEVEL takes, most severe first, in the order
    /// of the levels.
    pub const WORDS: [&'static str; 3] = ["ERROR", "WARNING", "INFO"];

    /// The level `word` names, INFO for any other.
    pub fn of(word: &str) -> Level {
        if word.eq_ignore_ascii_case("ERROR") {
            Level::Error
        } else if word.eq_ignore_ascii_case("WARNING") {
            Level::Warning
        } else {
            Level::Info
        }
    }
}

/// A kind of plug-in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Extension {
    /// A `StorageEngine`.
    Engine,
    /// An `IndexMethod`.
    IndexMethod,
    /// A `ScalarFunction` or an `AggregateFunction`.
    Function,
    /// A `SystemTable`.
    SystemTable,
    /// A setting a plug-in declared.
    Setting,
    /// A table option a plug-in declared (RULES.md D41).
    TableOption,
    /// An index option a plug-in declared (RULES.md D41).
    IndexOption,
    /// A `CatalogStorage` (`Database::set_catalog_storage`).
    CatalogStorage,
    /// A `LogStorage` (`Database::set_log_storage`).
    LogStorage,
    /// A `HashFunction` (`Database::register_hash_function`).
    HashFunction,
    /// An `Encoding` (`Database::register_encoding`).
    Encoding,
    /// \[EXTENSION\] A `Credentials` (`Database::set_credentials`).
    Credentials,
    /// \[EXTENSION\] A data type beyond the standard (`Database::register_type`).
    DataType,
    /// \[EXTENSION\] A `ServedConnections` (`Database::set_served_connections`).
    ServedConnections,
}

/// \[DESIGN\] An event of the database's.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Event {
    /// A database kept in a directory was opened: created, reopened, or
    /// recovered after a crash, with what recovery did.
    Opened(Opened),
    /// A database kept in a directory was closed cleanly.
    Closed,
    /// A checkpoint was taken: the number taken since the database opened.
    CheckpointTaken(u64),
    /// The database's value of a setting was changed: set, or, for none,
    /// back to its default.
    SettingChanged {
        /// The setting.
        name: String,
        /// Its new value, if it was set.
        value: Option<Value>,
    },
    /// \[EXTENSION\] A user was created, given a new verifier, or dropped:
    /// for an audit of who may connect.
    UserChanged {
        /// The user's authorization identifier.
        name: String,
        /// Whether it was dropped.
        dropped: bool,
    },
    /// \[EXTENSION\] The database was given an owner.
    OwnerChanged {
        /// The owner's authorization identifier.
        name: String,
    },
    /// A plug-in was registered, under its name.
    Registered {
        /// Its kind.
        kind: Extension,
        /// Its name.
        name: String,
    },
    /// A failure an operator must know of (`Event::of`).
    Failed(SqlError),
    /// \[DESIGN\] The thread of a job of background work, labelled
    /// `label`, started, stopped, failed, or went on after a panic (RULES.md
    /// D55).
    Thread {
        /// Its label.
        label: &'static str,
        /// What happened.
        change: ThreadChange,
    },
    /// A statement whose work took DIAGNOSTICS.SLOW_STATEMENT or longer: a
    /// call, or a cursor's opening and fetches (`db/slow.rs`). Its text is
    /// as its session gave it, values written in it included; none while
    /// anything of the database is encrypted (RULES.md D56).
    SlowStatement {
        /// Who ran it: its session's authorization identifier.
        user: String,
        /// Its work, in milliseconds.
        millis: u64,
        /// Its text, if it may be told.
        statement: Option<String>,
    },
    /// \[DESIGN\] More of the database's files were opened again, in a
    /// minute, than FILES.MAX_OPEN lets be open: the limit is too low for
    /// its work, each file closed only to be opened again (RULES.md D56).
    /// Told at most once a minute.
    FilesReopened {
        /// The files opened again in the minute.
        reopened: u64,
        /// FILES.MAX_OPEN.
        limit: u64,
    },
    /// \[DESIGN\] The log, or the tables file's side file, has grown past
    /// `db::OVERDUE` times what makes a checkpoint due
    /// (CHECKPOINT.AFTER_CHANGES, CHECKPOINT.AFTER_BYTES) with no
    /// checkpoint letting it be given up (RULES.md D25). Told at most once
    /// a minute.
    CheckpointOverdue {
        /// The row changes the log keeps for want of a checkpoint.
        changes: u64,
        /// Why.
        cause: Overdue,
    },
    /// \[DESIGN\] A server began to take connections (RULES.md D42).
    Listening {
        /// Where: each address, or Unix socket's path.
        addresses: Vec<String>,
    },
    /// \[DESIGN\] A server was told to stop: it takes no connection, and
    /// lets each finish its statement until SERVER.SHUTDOWN_TIMEOUT.
    Stopping {
        /// The connections it had.
        connections: u64,
    },
    /// \[DESIGN\] A server stopping went past SERVER.SHUTDOWN_TIMEOUT, or
    /// was told to stop again: the statements its sessions ran were
    /// cancelled, and its connections closed, their sessions rolled back.
    StopHurried {
        /// The sessions whose statements were cancelled.
        sessions: u64,
    },
    /// \[DESIGN\] A server's listener failed to take a connection, as
    /// when the process has no file descriptor left. Told at most once a
    /// minute.
    AcceptFailed {
        /// The failures since the last told, this one among them.
        failures: u64,
        /// What failed.
        message: String,
    },
    /// \[DESIGN\] A client's TLS handshake failed, or did not end within
    /// SERVER.LOGIN_TIMEOUT.
    TlsFailed {
        /// Who: an address, or `local`.
        peer: String,
        /// Why.
        message: String,
    },
    /// \[EXTENSION\] A client was refused as the user it named: for an
    /// audit of who tried to connect (RULES.md D43). Never with what it
    /// proved by.
    LoginFailed {
        /// The authorization identifier it named.
        user: String,
        /// Who: an address, or `local`.
        peer: String,
        /// The mechanism of authentication it asked for.
        mechanism: String,
        /// Why, as the client was told.
        message: String,
    },
    /// \[DESIGN\] Connections refused past SERVER.MAX_CONNECTIONS. Told at
    /// most once a minute.
    ConnectionsRefused {
        /// The connections refused since the last told, this one among them.
        refused: u64,
        /// SERVER.MAX_CONNECTIONS.
        limit: u64,
    },
}

/// \[DESIGN\] Why checkpoints let the log grow (`Event::CheckpointOverdue`).
#[derive(Debug, Clone)]
pub enum Overdue {
    /// Each is refused, with this error: a damaged table (RULES.md D19), a
    /// failed sync of the log (D25), or a failure of its own.
    Refused(SqlError),
    /// They are taken, but an open transaction keeps the log from its
    /// first record, logged this long ago.
    Transaction(Duration),
    /// None is taken: JOBS.POLICY, as it is, leaves them to the program
    /// (MANUAL) or to none (PAUSED), or, AUTOMATIC, its thread is behind.
    NotTaken(JobPolicy),
}

impl Event {
    /// How much the event matters.
    pub fn level(&self) -> Level {
        match self {
            Event::Opened(Opened::Recovered(_))
            | Event::SlowStatement { .. }
            | Event::FilesReopened { .. }
            | Event::CheckpointOverdue { .. }
            | Event::StopHurried { .. }
            | Event::TlsFailed { .. }
            | Event::LoginFailed { .. }
            | Event::ConnectionsRefused { .. } => Level::Warning,
            Event::AcceptFailed { .. } => Level::Error,
            Event::Failed(e) => worth(e.code).unwrap_or(Level::Warning),
            Event::Thread { change, .. } => match change {
                ThreadChange::Started | ThreadChange::Stopped => Level::Info,
                ThreadChange::Failed(_) => Level::Warning,
                ThreadChange::Restarted => Level::Error,
            },
            Event::Opened(Opened::Created | Opened::Reopened)
            | Event::Closed
            | Event::CheckpointTaken(_)
            | Event::SettingChanged { .. }
            | Event::UserChanged { .. }
            | Event::OwnerChanged { .. }
            | Event::Registered { .. }
            | Event::Listening { .. }
            | Event::Stopping { .. } => Level::Info,
        }
    }

    /// The event of failure `e`, if an operator must know of it: one of the
    /// database's making, not of the statement's.
    pub fn of(e: &SqlError) -> Option<Event> {
        worth(e.code).map(|_| Event::Failed(e.clone()))
    }
}

/// The level of a failure of `code`, if it is an event at all.
fn worth(code: Code) -> Option<Level> {
    match code {
        Code::Damaged | Code::Corrupt | Code::Io | Code::InDoubt => Some(Level::Error),
        Code::CommitFailed | Code::Resources => Some(Level::Warning),
        Code::Lexical
        | Code::Syntax
        | Code::SyntaxRule
        | Code::Privilege
        | Code::Constraint
        | Code::Data
        | Code::Cursor
        | Code::Cardinality
        | Code::Conflict
        | Code::Policy
        | Code::TransactionState
        | Code::Unsupported
        | Code::Connection
        | Code::Cancelled
        | Code::Dynamic
        | Code::Api => None,
    }
}

/// \[DESIGN\] Where the database's events go: an extension point (RULES.md
/// D40).
///
/// Contract (RULES.md R-SAFE-3):
/// * `event` may be called from several threads at once, by sessions and
///   by the checkpoint thread; it returns soon, keeping or writing the event
///   elsewhere, as a statement waits for it.
/// * It never calls back into the `Database` or its sessions (RULES.md
///   R-ERR-4), and never panics.
pub trait Diagnostics: Send + Sync {
    /// Takes `event`.
    fn event(&self, event: &Event);
}

/// The sink of a database given none: events are dropped.
pub struct NoDiagnostics;

impl Diagnostics for NoDiagnostics {
    fn event(&self, _event: &Event) {}
}

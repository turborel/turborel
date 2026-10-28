//! \[DESIGN\] Diagnostics (RULES.md D40): what the database tells whoever runs
//! it, as typed events rather than printed text, to a sink that is a
//! plug-in (`Diagnostics`). By default events are dropped; a server gives
//! a sink that writes them where its operators read them, and a test one
//! that keeps them.
//!
//! Events are what an operator must know of the database's life and
//! failures: opening and closing, checkpoints, changes of settings,
//! plug-ins registered, users changed, failures beyond a statement's own
//! making (a damaged table, corrupt data, an I/O error, a commit that
//! failed, memory refused), and statements slower than
//! DIAGNOSTICS.SLOW_STATEMENT. A failure of a statement's own SQL (a syntax
//! error, a violated constraint, a conflict) is the client's, returned to
//! it, and is no event. Events carry no row data, but for a slow
//! statement's text, and no wall-clock time: the sink stamps them as it
//! takes them. What is counted (I/O, pool hits) stays in
//! counters, read when wanted.

use crate::db::Opened;
use crate::error::{Code, SqlError};
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
    /// A statement whose work took DIAGNOSTICS.SLOW_STATEMENT or longer: a
    /// call, or a cursor's opening and fetches (`db/slow.rs`). Its text is
    /// as its session gave it, values written in it included.
    SlowStatement {
        /// Who ran it: its session's authorization identifier.
        user: String,
        /// Its work, in milliseconds.
        millis: u64,
        /// Its text.
        statement: String,
    },
}

impl Event {
    /// How much the event matters.
    pub fn level(&self) -> Level {
        match self {
            Event::Opened(Opened::Recovered(_)) | Event::SlowStatement { .. } => Level::Warning,
            Event::Failed(e) => worth(e.code).unwrap_or(Level::Warning),
            Event::Opened(Opened::Created | Opened::Reopened)
            | Event::Closed
            | Event::CheckpointTaken(_)
            | Event::SettingChanged { .. }
            | Event::UserChanged { .. }
            | Event::OwnerChanged { .. }
            | Event::Registered { .. } => Level::Info,
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
        Code::Damaged | Code::Corrupt | Code::Io => Some(Level::Error),
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

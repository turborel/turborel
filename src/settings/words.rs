//! \[DESIGN\] The words of the built-in two-way choices (RULES.md D36),
//! ON and OFF, as the transaction coordinator's types: LOCKS.WAIT,
//! SESSION.COMMIT_AT_END and TRANSACTION.STRICT; LOG.DURABILITY's, as the
//! log's; and TRANSACTION.ISOLATION's, as the parser's.

use super::builtin::{LOG_DURABILITY, READ_COMMITTED, READ_UNCOMMITTED, REPEATABLE_READ};
use super::{OFF, ON, Settings};
use crate::ast::IsolationLevel;
use crate::disk::io::Durability;
use crate::txn::{LockWait, Policy, SessionEnd};
use crate::value::Value;

/// The word or name a value is, empty if it is none.
pub(super) fn word_of(v: &Value) -> &str {
    if let Value::Char(s) = v { s } else { "" }
}

/// [X3.135-1986 §7.1 GR2] The session end a value says.
pub fn session_end(v: &Value) -> SessionEnd {
    if word_of(v) == OFF { SessionEnd::Rollback } else { SessionEnd::Commit }
}

/// The lock wait a value says.
pub fn lock_wait(v: &Value) -> LockWait {
    if word_of(v) == ON { LockWait::Wait } else { LockWait::NoWait }
}

/// The isolation level a value of `TRANSACTION.ISOLATION` says.
pub fn isolation_level(v: &Value) -> IsolationLevel {
    match word_of(v) {
        READ_UNCOMMITTED => IsolationLevel::ReadUncommitted,
        READ_COMMITTED => IsolationLevel::ReadCommitted,
        REPEATABLE_READ => IsolationLevel::RepeatableRead,
        _ => IsolationLevel::Serializable,
    }
}

/// The word of `LOCKS.WAIT` for `wait`.
pub fn lock_wait_word(wait: LockWait) -> &'static str {
    match wait {
        LockWait::NoWait => OFF,
        LockWait::Wait => ON,
    }
}

/// The word of `SESSION.COMMIT_AT_END` for `end`.
pub fn session_end_word(end: SessionEnd) -> &'static str {
    match end {
        SessionEnd::Commit => ON,
        SessionEnd::Rollback => OFF,
    }
}

/// The word of `TRANSACTION.STRICT` for `policy`.
pub fn policy_word(policy: Policy) -> &'static str {
    match policy {
        Policy::Strict => ON,
        Policy::Permissive => OFF,
    }
}

/// The durability of the log LOG.DURABILITY says, of `settings` (RULES.md
/// D25).
pub fn log_durability(settings: &Settings) -> Durability {
    Durability::of(settings.word(LOG_DURABILITY))
}

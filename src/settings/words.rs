//! \[DESIGN\] The words of the built-in two-way choices (RULES.md D36),
//! ON and OFF, as the transaction coordinator's types: LOCKS.WAIT,
//! SESSION.COMMIT_AT_END and TRANSACTION.STRICT; and LOG.DURABILITY's, as
//! the log's. Split from settings.rs (RULES.md R-SIZE-2).

use super::builtin::LOG_DURABILITY;
use super::{OFF, ON, Settings};
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

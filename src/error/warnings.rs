//! \[EXTENSION\] [ISO/IEC 9075:1992 §3.3.4.1, §4.19] The warnings a
//! statement raises as it runs: completion conditions, after which
//! "application of General Rules is not terminated", raised deep in the
//! evaluation of an expression, of a CAST [§6.10] or a set function
//! [§6.5 GR1b], and taken by the session as the statement completes.
//!
//! \[DESIGN\] A session holds one, beside what cancels its statements
//! (`txn::Stop`), so an operator reaches it through its transaction, from
//! whatever thread pulls its rows. As SQL-92 leaves it
//! implementation-dependent whether more than one condition is made
//! available [§3.3.4.1], it keeps the first raised. A warning raised once
//! one is costs a load; none raised, nothing.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use super::SqlState;
use crate::sync::lock_builtin;

/// The warnings raised since they were last taken: the first of them.
#[derive(Debug, Default)]
pub struct Warnings {
    /// Whether one was: so a warning raised after it takes no lock.
    raised: AtomicBool,
    first: Mutex<Option<SqlState>>,
}

impl Warnings {
    /// [ISO/IEC 9075:1992 §3.3.4.1] Raises the completion condition
    /// `state`, a warning: kept unless one was raised before it.
    pub(crate) fn raise(&self, state: SqlState) {
        if self.raised.load(Ordering::Acquire) {
            return;
        }
        let mut first = lock_builtin(&self.first);
        first.get_or_insert(state);
        self.raised.store(true, Ordering::Release);
    }

    /// The first warning raised since they were last taken, if any: none
    /// raised from now on.
    pub(crate) fn take(&self) -> Option<SqlState> {
        let mut first = lock_builtin(&self.first);
        self.raised.store(false, Ordering::Release);
        first.take()
    }
}

// Unit tests: the warnings kept (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §3.3.4.1] of the warnings
    /// raised, the first is kept, and taken once; none is raised before
    /// any, nor after they are taken, until another is.
    #[test]
    fn the_first_warning_is_kept_until_taken() {
        let warnings = Warnings::default();
        assert_eq!(warnings.take(), None);
        warnings.raise(SqlState::STRING_TRUNCATED);
        warnings.raise(SqlState::ZERO_BIT_PADDING);
        assert_eq!(warnings.take(), Some(SqlState::STRING_TRUNCATED));
        assert_eq!(warnings.take(), None);
        warnings.raise(SqlState::NULL_VALUE_ELIMINATED);
        assert_eq!(warnings.take(), Some(SqlState::NULL_VALUE_ELIMINATED));
    }
}

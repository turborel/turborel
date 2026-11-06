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
//! available [§3.3.4.1], it keeps the first raised, and notes whether
//! another was left out, for MORE [§18.1 GR1b]. The warning kept raised
//! again costs a load; none raised, nothing.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::SqlState;
use crate::sync::lock_builtin;

/// The warnings raised since they were last taken: the first of them.
#[derive(Debug, Default)]
pub struct Warnings {
    /// The first's `packed` value, 0 if none: so a warning raised after it
    /// takes no lock.
    kept: AtomicU64,
    /// Whether a warning other than the first was raised.
    left_out: AtomicBool,
    first: Mutex<Option<SqlState>>,
}

/// `state` as a number, not 0, to compare without the lock.
fn packed(state: SqlState) -> u64 {
    state.as_str().bytes().fold(0, |n, b| n.wrapping_shl(8) | u64::from(b))
}

impl Warnings {
    /// [ISO/IEC 9075:1992 §3.3.4.1] Raises the completion condition
    /// `state`, a warning: kept unless one was raised before it, and left
    /// out if that one is another.
    pub(crate) fn raise(&self, state: SqlState) {
        let code = packed(state);
        let kept = self.kept.load(Ordering::Acquire);
        if kept == code {
            return;
        }
        if kept != 0 {
            self.left_out.store(true, Ordering::Release);
            return;
        }
        let mut first = lock_builtin(&self.first);
        let first_raised = *first.get_or_insert(state);
        if first_raised != state {
            self.left_out.store(true, Ordering::Release);
        }
        self.kept.store(packed(first_raised), Ordering::Release);
    }

    /// Whether one was raised since they were last taken: so the statement
    /// completes with it, or one raised before it.
    pub(crate) fn raised(&self) -> bool {
        self.kept.load(Ordering::Acquire) != 0
    }

    /// The first warning raised since they were last taken, if any: none
    /// raised from now on.
    pub(crate) fn take(&self) -> Option<SqlState> {
        let mut first = lock_builtin(&self.first);
        self.kept.store(0, Ordering::Release);
        first.take()
    }

    /// Whether a warning was left out since this was last asked.
    pub(crate) fn take_left_out(&self) -> bool {
        self.left_out.swap(false, Ordering::AcqRel)
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
        assert!(!warnings.raised());
        warnings.raise(SqlState::STRING_TRUNCATED);
        assert!(warnings.raised());
        warnings.raise(SqlState::ZERO_BIT_PADDING);
        assert_eq!(warnings.take(), Some(SqlState::STRING_TRUNCATED));
        assert!(!warnings.raised(), "none raised once taken");
        assert_eq!(warnings.take(), None);
        warnings.raise(SqlState::NULL_VALUE_ELIMINATED);
        assert_eq!(warnings.take(), Some(SqlState::NULL_VALUE_ELIMINATED));
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §18.1 GR1b] a warning other
    /// than the one kept is noted as left out, once; the one kept raised
    /// again is not.
    #[test]
    fn a_warning_left_out_is_noted() {
        let warnings = Warnings::default();
        warnings.raise(SqlState::STRING_TRUNCATED);
        warnings.raise(SqlState::STRING_TRUNCATED);
        assert!(!warnings.take_left_out(), "the same condition");
        warnings.raise(SqlState::ZERO_BIT_PADDING);
        assert!(warnings.take_left_out());
        assert!(!warnings.take_left_out(), "asked once");
        assert_eq!(warnings.take(), Some(SqlState::STRING_TRUNCATED));
    }
}

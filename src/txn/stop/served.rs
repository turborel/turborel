//! \[DESIGN\] What a server tells a client of the statements a session
//! runs, beside what cancels them (RULES.md D187, D191, D194, D240): the
//! log position of the last commit, the time its statements waited for
//! locks, the trace context of the request running,
//! which a slow statement's event carries, and the notices its statements
//! raised, kept in `Stop` because the transaction that commits and the
//! program that signals reach the session through it.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::SqlState;
use crate::sync::lock_builtin;

/// \[DESIGN\] The most notices kept between two takings: a program that
/// signals in a loop holds no more, the rest dropped.
pub const MAX_NOTES: usize = 64;

/// What a server takes of a session's statements as they end.
#[derive(Debug, Default)]
pub struct Served {
    /// The log position of the last commit's record, 0 for none since it
    /// was last taken.
    committed: AtomicU64,
    /// The microseconds its statements waited for locks since this was
    /// last taken (RULES.md D240).
    lock_waits: AtomicU64,
    /// The trace context of the request running, if its client gave one.
    trace: Mutex<Option<String>>,
    /// The notices raised since they were last taken: each its SQLSTATE
    /// and message.
    notes: Mutex<Vec<(SqlState, String)>>,
}

impl Served {
    /// The commit whose record is at `position` made.
    pub(crate) fn committed_at(&self, position: u64) {
        self.committed.store(position, Ordering::Release);
    }

    /// The log position of the last commit's record, if one was made since
    /// one was last taken (`take_committed`), left as it is.
    pub(crate) fn last_committed(&self) -> Option<crate::log::Lsn> {
        Some(self.committed.load(Ordering::Acquire)).filter(|&p| p > 0).map(crate::log::Lsn)
    }

    /// The log position of the last commit since this was last asked, if
    /// one was made.
    pub fn take_committed(&self) -> Option<u64> {
        Some(self.committed.swap(0, Ordering::AcqRel)).filter(|&p| p > 0)
    }

    /// A wait of `waited` for a lock: added only once a lock was waited
    /// for, so a statement that waits for none pays nothing.
    pub(crate) fn waited(&self, waited: std::time::Duration) {
        let micros = u64::try_from(waited.as_micros()).unwrap_or(u64::MAX);
        self.lock_waits.fetch_add(micros, Ordering::Relaxed);
    }

    /// The microseconds waited for locks since this was last asked.
    pub fn take_lock_waits(&self) -> u64 {
        self.lock_waits.swap(0, Ordering::Relaxed)
    }

    /// The trace context of the requests from now on: none, none given.
    pub fn set_trace(&self, trace: Option<String>) {
        *lock_builtin(&self.trace) = trace;
    }

    /// The trace context of the request running, if it has one.
    pub fn trace(&self) -> Option<String> {
        lock_builtin(&self.trace).clone()
    }

    /// \[DESIGN\] [ISO/IEC 9075-4:2006 draft §17.2] The completion condition
    /// `state` raised with `message`, as a SIGNAL no handler handled
    /// raises it: kept, unless `MAX_NOTES` are.
    pub(crate) fn note(&self, state: SqlState, message: &str) {
        let mut notes = lock_builtin(&self.notes);
        if notes.len() < MAX_NOTES {
            notes.push((state, message.to_owned()));
        }
    }

    /// The notices raised since they were last taken, in order.
    pub fn take_notes(&self) -> Vec<(SqlState, String)> {
        std::mem::take(&mut *lock_builtin(&self.notes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D187, D194: a commit's position is taken
    /// once; the notes are taken in order, at most `MAX_NOTES` kept; the
    /// trace context lasts until set again.
    #[test]
    fn what_a_server_takes_is_taken_once() {
        let served = Served::default();
        assert_eq!(served.take_committed(), None);
        served.committed_at(7);
        assert_eq!((served.take_committed(), served.take_committed()), (Some(7), None));
        for n in 0..=MAX_NOTES {
            served.note(SqlState::STRING_TRUNCATED, &n.to_string());
        }
        let notes = served.take_notes();
        assert_eq!((notes.len(), notes[0].1.as_str()), (MAX_NOTES, "0"));
        assert_eq!(served.take_notes().len(), 0);
        served.set_trace(Some("t".into()));
        assert_eq!((served.trace(), served.trace()), (Some("t".into()), Some("t".into())));
    }
}

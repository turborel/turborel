//! \[DESIGN\] Statements told to the diagnostics as slow (RULES.md D40):
//! one whose work took DIAGNOSTICS.SLOW_STATEMENT or longer becomes an
//! `Event::SlowStatement`, with its text and who ran it; with no text while
//! anything of the database is encrypted, so no value meant for an
//! encrypted table reaches a log in the clear (RULES.md D56, D48). A
//! statement's work
//! is its call; a cursor's, its opening and every fetch, not the time
//! between, which is its client's. Nothing is timed while the setting is 0.

use std::time::{Duration, Instant};

use super::{Database, State};
use crate::diagnostics::Event;
use crate::settings::SLOW_STATEMENT;

/// A statement's work, timed.
pub(super) struct Timed<T> {
    /// Its text, unless it is not to be told.
    text: Option<T>,
    user: String,
    after: Duration,
    /// Since when it works, if it does now.
    since: Option<Instant>,
    /// Its work before that.
    busy: Duration,
}

impl<T: AsRef<str>> Timed<T> {
    /// The work of `text`, run by `user`, timed from now, if `st`'s
    /// DIAGNOSTICS.SLOW_STATEMENT asks for it.
    pub(super) fn of(st: &State, text: T, user: &str) -> Option<Timed<T>> {
        let millis = u64::try_from(st.settings.integer(SLOW_STATEMENT)).ok().filter(|&m| m > 0)?;
        let after = Duration::from_millis(millis);
        let text = st.spill_encodings().stages().is_empty().then_some(text);
        Some(Timed { text, user: user.to_owned(), after, since: Some(Instant::now()), busy: Duration::ZERO })
    }

    /// Its work resumed.
    pub(super) fn resumed(&mut self) {
        self.since = Some(Instant::now());
    }

    /// Its work paused, until resumed.
    pub(super) fn paused(&mut self) {
        if let Some(since) = self.since.take() {
            self.busy = self.busy.saturating_add(since.elapsed());
        }
    }

    /// Its work done: told to `db`'s diagnostics if it took long enough.
    pub(super) fn done(mut self, db: &Database) {
        self.paused();
        if self.busy >= self.after {
            let millis = u64::try_from(self.busy.as_millis()).unwrap_or(u64::MAX);
            let statement = self.text.as_ref().map(|t| t.as_ref().to_owned());
            db.emit(&Event::SlowStatement { user: self.user, millis, statement });
        }
    }
}

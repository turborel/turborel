//! \[DESIGN\] A statement's work, timed (RULES.md D40, D520, D521): one
//! timing that both what is told of a slow statement and the statement's
//! tally read. A statement's work is its call; a cursor's, its opening and
//! every fetch, not the time between, which is its client's.
//!
//! A statement whose work took DIAGNOSTICS.SLOW_STATEMENT_MS or longer
//! becomes an `Event::SlowStatement`, with its text and who ran it; with no
//! text while anything of the database is encrypted, so no value meant for
//! an encrypted table reaches a log in the clear (RULES.md D56, D48). It
//! carries the trace context of the client's request running it, if the
//! client gave one (RULES.md D191).
//!
//! While STATEMENTS.KEPT is not 0, its run is tallied in its session's
//! tallies as it ends (`txn/stop/activity/tallies.rs`): its time, its
//! rows, and what its session's counts of rows read and bytes spilled
//! grew by. Whatever the settings, the session's activity notes its text
//! and when it began (`Activity::began`). Nothing is timed while the
//! setting is 0 and STATEMENTS.KEPT is too.

use std::sync::Arc;
use std::time::{Duration, Instant};

use super::{Database, Session, State};
use crate::diagnostics::Event;
use crate::settings::SLOW_STATEMENT_MS;
use crate::system::Text;
use crate::txn::{Stop, Tallied};

/// What is told of a statement once it is slow.
struct Slow<T> {
    /// Its text, unless it is not to be told.
    text: Option<T>,
    user: String,
    /// The trace context of the request it runs for.
    trace: Option<String>,
    after: Duration,
}

/// A statement's work, timed: held in its call or its cursor as it is,
/// what is told of it once slow apart, as that is seldom asked for.
pub(super) struct Timed<T> {
    slow: Option<Box<Slow<T>>>,
    tallied: Option<Tallied>,
    /// Since when it works, if it does now.
    since: Option<Instant>,
    /// Its work before that.
    busy: Duration,
    /// The rows it gave, of a cursor, or changed.
    rows: u64,
}

impl Session {
    /// The work of `statement`, whose text `text` gives, timed from now if
    /// `st`'s DIAGNOSTICS.SLOW_STATEMENT_MS or STATEMENTS.KEPT asks for it:
    /// the text and the trace context are taken only for the first, so a
    /// statement not timed copies neither. The session's activity notes it
    /// begun either way.
    pub(super) fn timed<T: AsRef<str>>(&self, st: &State, text: impl FnOnce() -> T, statement: &Arc<Text>) -> Option<Timed<T>> {
        let began = self.now.get().utc;
        let activity = self.stop.activity();
        let kept = st.ran.statements.kept();
        let user = self.auth.borrow();
        let tallied = (kept > 0).then(|| {
            let (tallies, first) = activity.tallies();
            if first {
                st.ran.statements.list(tallies);
            }
            Tallied::of(activity, statement, (&user, kept), began)
        });
        activity.began(statement, began);
        let slow = slow(st, text, (&user, &self.stop)).map(Box::new);
        (slow.is_some() || tallied.is_some()).then(|| Timed { slow, tallied, since: Some(Instant::now()), busy: Duration::ZERO, rows: 0 })
    }
}

/// What is told of the statement whose text `text` gives, run by `user`
/// for a request of the trace context `stop` holds, if it is slow, if
/// `st`'s DIAGNOSTICS.SLOW_STATEMENT_MS asks for it.
fn slow<T>(st: &State, text: impl FnOnce() -> T, (user, stop): (&str, &Stop)) -> Option<Slow<T>> {
    let millis = u64::try_from(st.settings.integer(SLOW_STATEMENT_MS)).ok().filter(|&m| m > 0)?;
    let text = st.spill_encodings().stages().is_empty().then(text);
    Some(Slow { text, user: user.to_owned(), trace: stop.served().trace(), after: Duration::from_millis(millis) })
}

impl<T: AsRef<str>> Timed<T> {
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

    /// It gave or changed `rows` more rows.
    pub(super) fn rows(&mut self, rows: u64) {
        self.rows = self.rows.saturating_add(rows);
    }

    /// Its work done: tallied in `stop`'s session's tallies, and told to
    /// `db`'s diagnostics if it took long enough.
    pub(super) fn done(mut self, db: &Database, stop: &Stop) {
        self.paused();
        if let Some(tallied) = self.tallied.take() {
            tallied.done(stop.activity(), u64::try_from(self.busy.as_micros()).unwrap_or(u64::MAX), self.rows);
        }
        let Some(slow) = self.slow.take().filter(|s| self.busy >= s.after) else { return };
        let millis = u64::try_from(self.busy.as_millis()).unwrap_or(u64::MAX);
        let statement = slow.text.as_ref().map(|t| t.as_ref().to_owned());
        db.emit(&Event::SlowStatement { user: slow.user, millis, statement, trace: slow.trace });
    }
}

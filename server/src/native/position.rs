//! \[DESIGN\] Read after a commit (RULES.md D194): a Complete that
//! committed carries the log position of its commit, to a client granted
//! `commit-position`; a session that sets REPLICATION.READ_AFTER to it,
//! on a replica following the primary's log, has each statement wait for
//! the replica to have applied that position before it runs, so it reads
//! what the commit made. The wait is bounded by STATEMENT.TIMEOUT_MS, or
//! by `UNBOUNDED_WAIT` where that is 0, and fails past it; it looks again
//! every `LOOK_EVERY`, as a replica applies its log in steps of the
//! group's consensus. A Query of a SET statement never waits, so a session
//! can always change or clear the setting, however far its replica lags.

use std::time::{Duration, Instant};

use turborel::db::{Database, Session};
use turborel::error::{Code, Result, SqlError, SqlState};
use turborel::value::Value;

use crate::settings::read_after;

/// \[DESIGN\] How often a waiting statement looks at the position applied.
const LOOK_EVERY: Duration = Duration::from_millis(5);
/// \[DESIGN\] The longest wait of a session whose STATEMENT.TIMEOUT_MS is
/// 0: a statement never waits on a replica without bound.
const UNBOUNDED_WAIT: Duration = Duration::from_secs(30);
/// The setting that bounds a statement's work.
const STATEMENT_TIMEOUT_MS: &str = "STATEMENT.TIMEOUT_MS";
/// The key word of a SET statement.
const SET: &str = "SET";
/// \[DESIGN\] The SQLSTATE of a position not applied in time.
pub(super) const NOT_APPLIED: &str = "TBR01";

/// How long `session`'s statement may wait.
fn bound(session: &Session) -> Duration {
    let Ok(Value::Exact(d)) = session.setting(STATEMENT_TIMEOUT_MS) else { return UNBOUNDED_WAIT };
    match u64::try_from(d.units) {
        Ok(0) | Err(_) => UNBOUNDED_WAIT,
        Ok(millis) => Duration::from_millis(millis),
    }
}

/// Whether `sql`, a Query's text, is a SET statement, which never waits.
fn sets(sql: &str) -> bool {
    let first = turborel::lexer::lex(sql).ok().and_then(|tokens| tokens.into_iter().next());
    let Some(turborel::lexer::Token { tok: turborel::lexer::Tok::KeyWord(word), .. }) = first else { return false };
    word == SET
}

/// Waits until `db` has applied the position `session`'s
/// REPLICATION.READ_AFTER names, if it names one, before the statement of
/// the Query `sql`, if a Query's, or of an Execute.
///
/// # Errors
///
/// SQLSTATE `NOT_APPLIED` (SQLCODE -920) if it has not within the bound;
/// SQLCODE -910 if the database is closed.
pub(super) fn applied(session: &Session, db: &Database, sql: Option<&str>) -> Result<()> {
    let wanted = read_after(session);
    if wanted == 0 || sql.is_some_and(sets) {
        return Ok(());
    }
    let (began, within) = (Instant::now(), bound(session));
    loop {
        // A database with no log applies every commit as it is made.
        if db.applied_position()?.is_none_or(|at| at >= wanted) {
            return Ok(());
        }
        if began.elapsed() >= within {
            let msg = format!(
                "[DESIGN] the database had not applied log position {wanted} (REPLICATION.READ_AFTER) within {} ms",
                within.as_millis()
            );
            let state = SqlState::parse(NOT_APPLIED).unwrap_or(SqlState::CANCELLED);
            return Err(SqlError::new(Code::Cancelled, msg).with_state(state));
        }
        std::thread::sleep(LOOK_EVERY);
    }
}

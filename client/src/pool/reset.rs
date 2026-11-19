//! \[DESIGN\] A pooled connection's check and reset (RULES.md D206).
//! Granted `ping`, `answers` sends Ping, which the server answers with no
//! worker and no statement (D189). Granted `reset`, `reset` sends Reset,
//! which ends all the session holds in one request, its transaction,
//! prepared statements, cursors, temporary tables and settings, and keeps
//! its user; then the program's own statements run.
//!
//! A server that grants neither is checked and reset by statements.
//! `answers` sends ROLLBACK, which a session outside a transaction
//! completes doing nothing. `reset` gives the session back as it logged
//! in, in one pipeline: its transaction rolled back, the statements it
//! prepared closed and destroyed, its user, role, schema and zone those of
//! its login, then the program's own statements. A setting a borrower SET
//! stays, as no statement sets every setting back. Either way autocommit
//! is off after.

use std::time::Duration;

use crate::agent::Connected;
use crate::connection::{Outcome, Result, failed};
use crate::pipeline::Piped;
use turborel_protocol::capability::{PING, RESET};

/// \[DESIGN\] How long a pooled connection's check and reset wait on its
/// server, at most: as long as a step of connecting, so a server that
/// hangs holds the thread giving a connection back no longer.
pub const RESET_WITHIN: Duration = crate::timeouts::CONNECT_WITHIN;
/// \[DESIGN\] What a pooled connection is checked by, as it is handed out.
const CHECK: &str = "ROLLBACK";
/// \[DESIGN\] The most statements a borrower may leave prepared for the
/// connection to be reset rather than closed, as past it a new one costs
/// less than destroying them.
pub const RESET_MOST_STATEMENTS: u32 = 256;

/// Whether `connection` answers.
///
/// # Errors
///
/// A connection failure, if it is lost or does not answer.
pub(super) fn answers(connection: &mut Connected) -> Result<()> {
    if connection.granted().has(PING) {
        return bounded(connection, Connected::ping);
    }
    bounded(connection, |c| c.query(CHECK).map(drop))
}

/// Resets `connection` by the protocol's Reset, then runs `more`.
///
/// # Errors
///
/// A connection failure; any of `more`'s failures.
fn renewed(connection: &mut Connected, more: &[String]) -> Result<()> {
    connection.reset()?;
    // Every statement it prepared went with the session it had.
    connection.next_statement = 0;
    more.iter().try_for_each(|sql| connection.query(sql).map(drop))
}

/// `run` of `connection`, each of its waits at most `RESET_WITHIN`, or
/// the connection's own limit if shorter; the limit put back after. A
/// connection over a stream its maker bounds is run as it is bounded.
fn bounded(connection: &mut Connected, run: impl FnOnce(&mut Connected) -> Result<()>) -> Result<()> {
    let before = connection.read_timeout();
    let within = before.map_or(RESET_WITHIN, |b| b.min(RESET_WITHIN));
    let limited = connection.set_read_timeout(Some(within)).is_ok();
    let ran = run(connection);
    if limited {
        connection.set_read_timeout(before)?;
    }
    ran
}

/// `name` as a character string literal of a delimited identifier, as SET
/// SESSION AUTHORIZATION and SET SCHEMA take one, so a name of any case or
/// character is the same name.
fn delimited(name: &str) -> String {
    format!("'\"{}\"'", name.replace('"', "\"\"").replace('\'', "''"))
}

/// Gives `connection`'s session back as it logged in, and runs `more`.
///
/// # Errors
///
/// A connection failure; any statement's failure but those destroying
/// statements already gone; too many statements left prepared.
pub(super) fn reset(connection: &mut Connected, more: &[String]) -> Result<()> {
    bounded(connection, |c| reset_now(c, more))
}

/// Resets `connection` as `reset` says, unbounded.
fn reset_now(connection: &mut Connected, more: &[String]) -> Result<()> {
    if connection.is_lost() {
        return Err(failed("[DESIGN] a lost connection is not given back"));
    }
    if connection.granted().has(RESET) {
        connection.set_autocommit(false);
        return renewed(connection, more);
    }
    let prepared = connection.next_statement;
    if prepared > RESET_MOST_STATEMENTS {
        return Err(failed("[DESIGN] too many statements were left prepared to destroy"));
    }
    connection.set_autocommit(false);
    let login = connection.authorization().to_owned();
    let mut pipeline = connection.pipeline();
    pipeline.query("ROLLBACK");
    for number in 0..prepared {
        pipeline.release(number);
    }
    pipeline.query(&format!("SET SESSION AUTHORIZATION {}", delimited(&login)));
    pipeline.query(&format!("SET SCHEMA {}", delimited(&login)));
    pipeline.query("SET TIME ZONE LOCAL");
    for sql in more {
        pipeline.query(sql);
    }
    let outcomes = pipeline.run()?;
    let releases = usize::try_from(prepared).unwrap_or(usize::MAX).saturating_mul(2);
    let (first, rest) = outcomes.split_at_checked(1).unwrap_or_default();
    let kept = rest.get(releases..).unwrap_or_default();
    let failed_one = first.iter().chain(kept).find_map(|o| match o {
        Piped::Failed(f) => Some(f.clone()),
        Piped::Done(Outcome::Done(_) | Outcome::Rows { .. }) | Piped::Skipped => None,
    });
    if let Some(failure) = failed_one {
        return Err(failure);
    }
    connection.next_statement = 0;
    Ok(())
}

//! \[DESIGN\] A statement run (RULES.md D46, D212, D215, D219): echoed if
//! ECHO says, refused if SAFE_UPDATES finds it changes every row, in a
//! savepoint of its own if ROLLBACK_ON_ERROR says, its own transaction if
//! AUTOCOMMIT says, its rows written as the layout says, timed if asked;
//! then ROW_COUNT, ERROR, SQLSTATE and the last error's variables set.

use std::io::Write;
use std::time::Duration;

use turborel::error::{Code, SqlError};
use turborel::storage::Row;
use turborel_client::connection::{Sink, Streamed};
use turborel_client::{Column, Failure};

use super::output::{Out, Redirect};
use super::{Next, Outcome, Shell, Statements, Wrong, cancelled};
use crate::command::Target;
use crate::format::Printer;
use crate::vars::{self, Echo, Verbosity};
use crate::words;

/// The savepoint a statement is run in under ROLLBACK_ON_ERROR.
const SAVEPOINT: &str = "TURBOREL_STATEMENT";
/// The SQLSTATE of success [ISO/IEC 9075:1992 §22.1].
const SUCCESS: &str = "00000";

/// How a statement's results are written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct How {
    /// Where.
    pub(super) target: Target,
    /// Whether expanded, whatever the layout says.
    pub(super) expanded: bool,
}

impl Default for How {
    fn default() -> How {
        How { target: Target::Default, expanded: false }
    }
}

/// A sink that keeps a query's columns and rows.
#[derive(Debug, Default)]
pub struct Kept {
    /// The columns.
    pub columns: Vec<Column>,
    /// The rows.
    pub rows: Vec<Row>,
}

impl Sink for Kept {
    fn columns(&mut self, columns: &[Column]) {
        columns.clone_into(&mut self.columns);
    }

    fn rows(&mut self, rows: &[Row]) {
        self.rows.extend_from_slice(rows);
    }
}

/// `failure` as VERBOSITY tells it, of the statement `sql`.
fn told(failure: &Failure, verbosity: Verbosity, sql: &str) -> String {
    match verbosity {
        Verbosity::Terse => format!("SQLCODE {} (SQLSTATE {}): {}", failure.sqlcode, failure.sqlstate, failure.message),
        Verbosity::Default => failure.to_string(),
        Verbosity::Verbose => {
            let Some(position) = failure.position.and_then(|p| usize::try_from(p).ok()) else { return failure.to_string() };
            let before: String = sql.chars().take(position.saturating_sub(1)).collect();
            let (number, start) =
                before.match_indices('\n').fold((1_usize, 0_usize), |(n, _), (at, _)| (n.saturating_add(1), at.saturating_add(1)));
            let line = sql.get(start..).unwrap_or_default().lines().next().unwrap_or_default();
            let column = before.get(start..).unwrap_or_default().chars().count();
            let lead = format!("line {number}: ");
            format!("{failure}\n{lead}{line}\n{}^", " ".repeat(lead.len().saturating_add(column)))
        }
    }
}

impl<S: Statements + ?Sized, C: FnMut() -> Duration> Shell<'_, S, C> {
    /// Runs `sql`, typed or not, its results written as the layout says.
    pub(super) fn statement(&mut self, sql: &str, typed: bool) -> Next {
        self.statement_as(sql, typed, &How::default())
    }

    /// Runs `sql`, typed or not, its results written as `how` says.
    pub(super) fn statement_as(&mut self, sql: &str, typed: bool, how: &How) -> Next {
        sql.clone_into(self.last.get_or_insert_with(String::new));
        if self.vars.echo() == Echo::Queries && self.result_line(sql).is_err() {
            return Next::Stop;
        }
        if self.vars.on(vars::SAFE_UPDATES) && words::unbounded_change(sql) {
            let msg = "[DESIGN] SAFE_UPDATES is on: an UPDATE or DELETE without WHERE would change every row, and is not sent";
            return self.failed_statement(&Failure::from(&SqlError::new(Code::SyntaxRule, msg)), sql, typed);
        }
        let single = self.options.settings.single_transaction;
        self.connection.set_autocommit(self.vars.on(vars::AUTOCOMMIT) && !single);
        let saved = self.vars.rollback_on_error(typed) && self.savepoint(sql);
        let began = self.vars.on(vars::TIMING).then(|| (self.now)());
        let ran = self.ran(sql, how);
        let elapsed = began.map(|at| (self.now)().saturating_sub(at));
        if let Some(interrupts) = self.interrupts {
            interrupts.cancel_by(self.connection.canceller());
        }
        let next = match ran {
            Ok(state) => {
                self.succeeded(sql, &state);
                self.quietly(saved, &format!("RELEASE SAVEPOINT {SAVEPOINT}"));
                Next::Go
            }
            Err(Wrong::Unwritten(e)) => self.tell(&format!("cannot write: {e}"), Next::Stop),
            Err(Wrong::Failed(failure)) => {
                self.quietly(saved, &format!("ROLLBACK TO SAVEPOINT {SAVEPOINT}"));
                self.failed_statement(&failure, sql, typed)
            }
        };
        if let Some(time) = elapsed {
            // Nothing is left to tell a failure to write a failure to.
            let _ = writeln!(self.err, "Time: {} ns", time.as_nanos()).and_then(|()| self.err.flush());
        }
        next
    }

    /// Takes the savepoint `sql` runs in, if its transaction is active
    /// and it neither ends nor marks one: whether it was taken.
    fn savepoint(&mut self, sql: &str) -> bool {
        let control = words::first(sql).is_some_and(|w| words::TRANSACTION_CONTROL.contains(&w.as_str()));
        if control || !self.connection.in_transaction() {
            return false;
        }
        self.connection.query_into(&format!("SAVEPOINT {SAVEPOINT}"), &mut Kept::default()).is_ok()
    }

    /// Runs `sql` unseen, if `wanted`: its outcome does not change the
    /// statement's.
    fn quietly(&mut self, wanted: bool, sql: &str) {
        if wanted {
            // The statement's own outcome is what is told.
            drop(self.connection.query_into(sql, &mut Kept::default()));
        }
    }

    /// Runs `sql`, its results written as `how` says: the SQLSTATE it
    /// completed with, and its rows' count, of a query.
    fn ran(&mut self, sql: &str, how: &How) -> Result<(String, Option<u64>), Wrong> {
        let mut once = Redirect::open(&how.target).map_err(Wrong::Unwritten)?;
        let pager = self.pager(once.is_some());
        let main: &mut dyn Write = match (&mut once, &mut self.redirect) {
            (Some(r), _) | (None, Some(r)) => r.writer(),
            (None, None) => &mut *self.out,
        };
        let mut out = Out { main, tee: self.tee.as_mut(), pager };
        let width = self.terminal.screen.map(|s| s.width);
        let fetch = self.vars.number(vars::FETCH_COUNT).filter(|n| *n > 0);
        let mut printer = Printer::new(&mut out, &self.layout, width, how.expanded);
        let streamed = match fetch {
            Some(rows) => self.connection.fetch_into(sql, rows, &mut printer),
            None => self.connection.query_into(sql, &mut printer),
        };
        let state = match streamed.map_err(Wrong::Failed)? {
            Streamed::Rows(count) => {
                let warning = self.connection.warning().map(str::to_owned);
                printer.finish(count, warning.as_deref()).map_err(Wrong::Unwritten)?;
                (warning.unwrap_or_else(|| SUCCESS.to_owned()), Some(count))
            }
            Streamed::Done(completed) => {
                drop(printer);
                let said = match (completed.sqlcode, completed.is_warning()) {
                    (0_i32, false) => "OK".to_owned(),
                    (0_i32, true) => format!("OK, with a warning: SQLSTATE {}", completed.sqlstate),
                    (_, _) => "no data".to_owned(),
                };
                if !self.vars.on(vars::QUIET) {
                    writeln!(out, "{said}").map_err(Wrong::Unwritten)?;
                }
                (completed.sqlstate, None)
            }
        };
        out.finish().map_err(Wrong::Unwritten)?;
        once.map(Redirect::close).transpose().map_err(Wrong::Unwritten)?;
        Ok(state)
    }

    /// Sets the variables of `sql` succeeded, `state` its SQLSTATE and
    /// rows; the names completion offers read again after a statement that
    /// changes the schema, and the prompt's status after one that changes
    /// the session.
    fn succeeded(&mut self, sql: &str, (sqlstate, rows): &(String, Option<u64>)) {
        self.vars.set(vars::ERROR, "false");
        self.vars.set(vars::SQLSTATE, sqlstate);
        self.vars.set(vars::ROW_COUNT, &rows.unwrap_or(0).to_string());
        self.track_schema(sql);
        let first = words::first(sql).unwrap_or_default();
        if words::SCHEMA_CHANGES.contains(&first.as_str()) {
            self.names = None;
        }
        if words::TRANSACTION_CONTROL.contains(&first.as_str()) || first == "ALTER" {
            self.status = None;
        }
    }

    /// Tells `failure`, of `sql`, typed or not, and sets the variables of
    /// a failure: how the run goes on.
    fn failed_statement(&mut self, failure: &Failure, sql: &str, typed: bool) -> Next {
        self.vars.set(vars::ERROR, "true");
        self.vars.set(vars::SQLSTATE, &failure.sqlstate);
        self.vars.set(vars::ROW_COUNT, "0");
        self.vars.set(vars::LAST_ERROR_MESSAGE, &failure.message);
        self.vars.set(vars::LAST_ERROR_SQLSTATE, &failure.sqlstate);
        if self.vars.echo() == Echo::Errors {
            self.told(sql);
        }
        let msg = told(failure, self.vars.verbosity(), sql);
        if self.connection.is_lost() {
            return self.stop(&msg, Outcome::Lost);
        }
        if typed {
            return self.tell(&msg, Next::Go);
        }
        if self.vars.on(vars::STOP_ON_ERROR) {
            return self.stop(&msg, Outcome::Stopped);
        }
        if cancelled(failure) {
            return self.stop(&msg, Outcome::Failed);
        }
        self.tell(&msg, Next::Go)
    }

    /// Runs `sql`, its rows kept; a failure told and its variables set.
    pub(super) fn kept(&mut self, sql: &str) -> Option<Kept> {
        let mut kept = Kept::default();
        match self.connection.query_into(sql, &mut kept) {
            Ok(_) => Some(kept),
            Err(failure) => {
                self.failed_statement(&failure, sql, self.typing);
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D212: a terse failure is its codes and
    /// message; a verbose one adds the line of the statement where it is,
    /// marked under its character; without a position, as by default.
    #[test]
    fn failures_are_told_as_verbosity_says() {
        let failure = Failure {
            sqlcode: -103,
            sqlstate: "2A000".into(),
            message: "no".into(),
            rule: Some("X".into()),
            position: Some(10),
            active: false,
        };
        assert_eq!(told(&failure, Verbosity::Terse, ""), "SQLCODE -103 (SQLSTATE 2A000): no");
        assert_eq!(told(&failure, Verbosity::Default, ""), failure.to_string());
        let sql = "SELECT 1\nFROM NOPE";
        assert_eq!(told(&failure, Verbosity::Verbose, sql), format!("{failure}\nline 2: FROM NOPE\n        ^"));
        let nowhere = Failure { position: None, ..failure.clone() };
        assert_eq!(told(&nowhere, Verbosity::Verbose, sql), nowhere.to_string());
    }
}

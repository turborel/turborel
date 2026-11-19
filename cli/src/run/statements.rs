//! \[DESIGN\] What runs a run's statements (RULES.md D46): a connection,
//! or an SQL-agent's connections [ISO/IEC 9075:1992 §15], which takes
//! CONNECT, SET CONNECTION and DISCONNECT itself.

use std::fs::File;
use std::io::{BufReader, Read, Write};

use turborel::storage::Row;
use turborel_client::agent::{Agent, Connector};
use turborel_client::connection::{Connection, Sink, Streamed};
use turborel_client::import::{self, Cause, Delimited, Import, ImportError, Imported, Rejected};
use turborel_client::{Canceller, Failure};

/// What a file `--import` names is read by.
pub type FileSource = Delimited<BufReader<File>>;

/// What runs a run's statements.
pub trait Statements {
    /// Runs `sql`, a query's rows given to `sink`.
    ///
    /// # Errors
    ///
    /// The statement's failure.
    fn query_into(&mut self, sql: &str, sink: &mut dyn Sink) -> Result<Streamed, Failure>;

    /// Runs `sql` as `query_into` does, a query's rows fetched `rows` at a
    /// time through a cursor (FETCH_COUNT, RULES.md D212): a statement
    /// that cannot be prepared, or is no query, run as `query_into` runs
    /// it.
    ///
    /// # Errors
    ///
    /// The statement's failure.
    fn fetch_into(&mut self, sql: &str, rows: u32, sink: &mut dyn Sink) -> Result<Streamed, Failure>;

    /// What cancels the statement running, if anything can.
    fn canceller(&self) -> Option<Canceller>;

    /// Whether the connection statements run over is lost.
    fn is_lost(&self) -> bool;

    /// The SQLSTATE of the warning the last statement completed with, if
    /// it did.
    fn warning(&self) -> Option<&str>;

    /// Whether the session's transaction is active, as the server last
    /// said; false with no connection.
    fn in_transaction(&mut self) -> bool;

    /// Whether each statement is its own transaction from now on.
    fn set_autocommit(&mut self, on: bool);

    /// The server's product and version, and the session's user, of the
    /// current connection, if there is one.
    fn session(&mut self) -> Option<(String, String)>;

    /// The current connection's name: none for the default one, or for a
    /// connection not an agent's.
    fn connection_name(&self) -> Option<String>;

    /// Imports `source`'s rows as `how` says (RULES.md D59), each row
    /// skipped given to `rejected`.
    ///
    /// # Errors
    ///
    /// Why the import stopped, and what of it was kept.
    fn import(&mut self, source: &mut FileSource, how: &Import, rejected: &mut dyn FnMut(Rejected)) -> Result<Imported, ImportError>;
}

/// An import's failure `failure` before it began: nothing kept.
fn unbegun(failure: Failure) -> ImportError {
    ImportError { position: None, cause: Cause::Sql(Box::new(failure)), committed: 0, rolled_back: 0 }
}

/// Runs `sql` over `connection`, a query's rows fetched `rows` at a time
/// through a cursor, given to `sink`; none if it cannot be prepared or
/// is no query, so it is run otherwise.
fn fetched<S: Read + Write>(
    connection: &mut Connection<S>,
    sql: &str,
    rows: u32,
    sink: &mut dyn Sink,
) -> Option<Result<Streamed, Failure>> {
    let statement = connection.prepare(sql).ok()?;
    if statement.columns().is_empty() {
        // Nothing of it ran: preparing has no effect but its number.
        drop(connection.deallocate(statement));
        return None;
    }
    sink.columns(statement.columns());
    let mut count = 0_u64;
    let mut take = |batch: &[Row], into: &mut dyn Sink| {
        count = count.saturating_add(u64::try_from(batch.len()).unwrap_or(u64::MAX));
        into.rows(batch);
    };
    let read = (|| {
        let mut batch = connection.open_cursor(&statement, &[], rows)?;
        take(&batch.rows, sink);
        while batch.more {
            batch = connection.fetch(&statement, rows)?;
            take(&batch.rows, sink);
        }
        Ok(())
    })();
    // The rows, or the failure, are the outcome; a failure to free the
    // statement's number after them changes neither.
    drop(connection.deallocate(statement));
    Some(read.map(|()| Streamed::Rows(count)))
}

impl<S: Read + Write> Statements for Connection<S> {
    fn query_into(&mut self, sql: &str, sink: &mut dyn Sink) -> Result<Streamed, Failure> {
        Connection::query_into(self, sql, sink)
    }

    fn fetch_into(&mut self, sql: &str, rows: u32, sink: &mut dyn Sink) -> Result<Streamed, Failure> {
        fetched(self, sql, rows, sink).unwrap_or_else(|| Connection::query_into(self, sql, sink))
    }

    fn canceller(&self) -> Option<Canceller> {
        Connection::canceller(self)
    }

    fn is_lost(&self) -> bool {
        Connection::is_lost(self)
    }

    fn warning(&self) -> Option<&str> {
        Connection::warning(self)
    }

    fn in_transaction(&mut self) -> bool {
        Connection::in_transaction(self)
    }

    fn set_autocommit(&mut self, on: bool) {
        Connection::set_autocommit(self, on);
    }

    fn session(&mut self) -> Option<(String, String)> {
        Some((self.server().to_owned(), self.authorization().to_owned()))
    }

    fn connection_name(&self) -> Option<String> {
        None
    }

    fn import(&mut self, source: &mut FileSource, how: &Import, mut rejected: &mut dyn FnMut(Rejected)) -> Result<Imported, ImportError> {
        import::import(self, source, how, &mut rejected)
    }
}

impl<C: Connector> Statements for Agent<C> {
    fn query_into(&mut self, sql: &str, sink: &mut dyn Sink) -> Result<Streamed, Failure> {
        Agent::query_into(self, sql, sink)
    }

    fn fetch_into(&mut self, sql: &str, rows: u32, sink: &mut dyn Sink) -> Result<Streamed, Failure> {
        if let Some(done) = self.connection_statement(sql)? {
            return Ok(Streamed::Done(done));
        }
        let current = self.current()?;
        fetched(current, sql, rows, sink).unwrap_or_else(|| current.query_into(sql, sink))
    }

    fn canceller(&self) -> Option<Canceller> {
        Agent::canceller(self)
    }

    fn is_lost(&self) -> bool {
        Agent::is_lost(self)
    }

    fn warning(&self) -> Option<&str> {
        Agent::warning(self)
    }

    fn in_transaction(&mut self) -> bool {
        self.current().is_ok_and(Statements::in_transaction)
    }

    fn set_autocommit(&mut self, on: bool) {
        if let Ok(current) = self.current() {
            current.set_autocommit(on);
        }
    }

    fn session(&mut self) -> Option<(String, String)> {
        let current = self.current().ok()?;
        Some((current.server().to_owned(), current.authorization().to_owned()))
    }

    fn connection_name(&self) -> Option<String> {
        self.current_name().map(str::to_owned)
    }

    fn import(&mut self, source: &mut FileSource, how: &Import, mut rejected: &mut dyn FnMut(Rejected)) -> Result<Imported, ImportError> {
        import::import(self.current().map_err(unbegun)?, source, how, &mut rejected)
    }
}

//! \[DESIGN\] What runs a run's statements (RULES.md D46), a connection,
//! or an SQL-agent's connections [ISO/IEC 9075:1992 §15], which takes
//! CONNECT, SET CONNECTION and DISCONNECT itself.

use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::time::Duration;

use turborel::error::{Code, SqlError};
use turborel::storage::Row;
use turborel::value::{DataType, Value};
use turborel_client::agent::{Agent, Connected, Connector};
use turborel_client::capability::PING;
use turborel_client::connection::{Connection, Outcome, Sink, Streamed};
use turborel_client::export::Layout;
use turborel_client::import::{self, Cause, Delimited, Import, ImportError, Imported, Rejected};
use turborel_client::{Canceller, Column, Failure, Notice, Timing};

use super::debugging::debugged;
use crate::command::DebugCommand;
use crate::timing::ServerTime;

/// The SQLSTATE class of a warning [ISO/IEC 9075:1992 §22.1].
const WARNING_CLASS: &str = "01";

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
    /// time through a cursor (FETCH_COUNT, RULES.md D212). A statement
    /// that cannot be prepared, or is no query, is run as `query_into`
    /// runs it.
    ///
    /// # Errors
    ///
    /// The statement's failure.
    fn fetch_into(&mut self, sql: &str, rows: u32, sink: &mut dyn Sink) -> Result<Streamed, Failure>;

    /// Describes `sql` without running it, to `sink`: a row of each
    /// column of its result, its name and its type, as declared where the
    /// server tells it (RULES.md D396). Fails where nothing prepares it.
    ///
    /// # Errors
    ///
    /// The statement's failure to be prepared.
    fn describe_into(&mut self, _sql: &str, _sink: &mut dyn Sink) -> Result<Streamed, Failure> {
        Err(Failure::from(&SqlError::new(Code::Api, "[DESIGN] these statements are not prepared, so none is described")))
    }

    /// What cancels the statement running, if anything can.
    fn canceller(&self) -> Option<Canceller>;

    /// Whether the connection statements run over is lost.
    fn is_lost(&self) -> bool;

    /// The SQLSTATE of the warning the last statement completed with, if
    /// it did.
    fn warning(&self) -> Option<&str>;

    /// The time the server reports spending on the last statement, as
    /// its completion told it (RULES.md D240); none of a server that
    /// tells none, and the time shown is the round trip alone.
    fn server_time(&self) -> Option<ServerTime> {
        None
    }

    /// Whether the server answers, asked by Ping, which no worker of its
    /// takes (RULES.md D189); none if the server does not take Ping.
    fn ping(&mut self) -> Option<Result<(), Failure>> {
        None
    }

    /// The session status `name`, as the server last told it, with no
    /// round trip (RULES.md D190); none of a server that tells none.
    fn status(&self, _name: &str) -> Option<String> {
        None
    }

    /// The notices the server sent since they were last taken, and the
    /// warnings the last statement completed with, each once (RULES.md
    /// D187).
    fn take_notices(&mut self) -> Vec<Notice> {
        Vec::new()
    }

    /// Writes the rows of the query `sql` to `out` by `layout`, as they
    /// come, each large object as its value (RULES.md D195, D975): how
    /// many.
    ///
    /// # Errors
    ///
    /// The query's failure, or the write's.
    fn export(&mut self, sql: &str, out: &mut dyn Write, layout: &mut dyn Layout) -> Result<u64, Failure>;

    /// Whether the session's transaction is active, as the server last
    /// said; false with no connection.
    fn in_transaction(&mut self) -> bool;

    /// Whether each statement is its own transaction from now on.
    fn set_autocommit(&mut self, on: bool);

    /// The server's product and version, and the session's user, of the
    /// current connection, if there is one.
    fn session(&mut self) -> Option<(String, String)>;

    /// The current connection's name, or none for the default one, or for
    /// a connection not an agent's.
    fn connection_name(&self) -> Option<String>;

    /// Imports `source`'s rows as `how` says (RULES.md D59), each row
    /// skipped given to `rejected`.
    ///
    /// # Errors
    ///
    /// Why the import stopped, and what of it was kept.
    fn import(&mut self, source: &mut FileSource, how: &Import, rejected: &mut dyn FnMut(Rejected)) -> Result<Imported, ImportError>;

    /// \[DESIGN\] RULES.md D1148: a connection of its own to the server the
    /// command line names, proved as the first was, for a job of an import
    /// over several connections at once.
    ///
    /// # Errors
    ///
    /// The server cannot be reached, or refuses the connection; SQLCODE
    /// -900 of what makes no other connection.
    fn spare(&mut self) -> Result<Connected, Failure> {
        Err(Failure::from(&SqlError::new(Code::Unsupported, "[DESIGN] what this run's statements go to makes no other connection")))
    }

    /// \[EXTENSION\] What `\debug` asks, of the current connection, as
    /// lines, null values written as `null` (RULES.md D1036).
    ///
    /// # Errors
    ///
    /// The debugger's call's failure, as the server reports it; SQLCODE
    /// -900 of what serves no debugger.
    fn debug(&mut self, _asked: &DebugCommand, _null: &str) -> Result<String, Failure> {
        Err(Failure::from(&SqlError::new(Code::Unsupported, "[EXTENSION] what this run's statements go to serves no debugger")))
    }
}

/// An import's failure `failure` before it began, so nothing kept.
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
        // Nothing of it ran, as preparing has no effect but its number.
        drop(connection.deallocate(statement));
        return None;
    }
    sink.columns(statement.columns());
    // [ISO/IEC 9075-2:2023 §20.7 GR7a] a CALL's output dynamic parameters
    // come as a row as it is executed, no cursor's.
    if is_call(sql) {
        let outcome = connection.execute(&statement, &[]);
        drop(connection.deallocate(statement));
        return Some(outcome.map(|o| {
            let Outcome::Rows { rows: row, .. } = o else { return Streamed::Rows(0) };
            sink.rows(&row);
            Streamed::Rows(u64::try_from(row.len()).unwrap_or(u64::MAX))
        }));
    }
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

/// The headings of a description's two columns.
const DESCRIBED: [&str; 2] = ["Column", "Type"];

/// Describes `sql`, prepared over `connection` and never run, to `sink`:
/// a row of each column, its name and its type as SQL writes it, as it
/// was declared where the server tells it (RULES.md D396).
fn described<S: Read + Write>(connection: &mut Connection<S>, sql: &str, sink: &mut dyn Sink) -> Result<Streamed, Failure> {
    let statement = connection.prepare(sql)?;
    let rows: Vec<Row> = statement
        .columns()
        .iter()
        .map(|c| vec![Value::Char(c.name.clone().unwrap_or_default().into()), Value::Char(c.type_written().into())])
        .collect();
    // Nothing of it ran; a failure to free its number changes nothing
    // described.
    drop(connection.deallocate(statement));
    let widest = |at: usize| {
        let longest = rows.iter().filter_map(|r| r.get(at)).map(|v| v.to_string().chars().count()).max().unwrap_or(1);
        u32::try_from(longest.max(1)).unwrap_or(u32::MAX)
    };
    let headings = DESCRIBED.iter().enumerate();
    let columns: Vec<Column> = headings
        .map(|(at, name)| Column {
            name: Some((*name).to_owned()),
            data_type: DataType::varying(widest(at)),
            not_null: true,
            declared: None,
        })
        .collect();
    sink.columns(&columns);
    sink.rows(&rows);
    Ok(Streamed::Rows(u64::try_from(rows.len()).unwrap_or(u64::MAX)))
}

/// Whether `sql` is a CALL that may have output dynamic parameters, one
/// with a `?`, which only a statement prepared takes, so it is run as
/// `fetched` runs it, its outputs shown as a row [ISO/IEC 9075-2:2023
/// §20.7 GR7a].
pub(super) fn calls_out(sql: &str) -> bool {
    is_call(sql) && sql.contains('?')
}

/// Whether `sql` is a CALL [ISO/IEC 9075-2:2023 §16.1]: its first word.
fn is_call(sql: &str) -> bool {
    let first = sql.trim_start().split(|c: char| !c.is_ascii_alphabetic()).next().unwrap_or_default();
    first.eq_ignore_ascii_case(CALL)
}

/// [ISO/IEC 9075-2:2023 §16.1] The key word CALL begins.
const CALL: &str = "CALL";

/// `timing`, the server's report of a statement's time, as the timing
/// line shows it: its waits for locks among the whole, and its wait for
/// a worker before it.
fn shown(timing: Timing) -> ServerTime {
    let micros = Duration::from_micros;
    ServerTime {
        total: micros(timing.total),
        waiting: Some(micros(timing.locks)),
        queued: Some(micros(timing.queued)),
        ..ServerTime::default()
    }
}

/// The notices `connection`'s server sent since they were last taken,
/// and the warnings among the conditions its last completion carried, as
/// notices: class 01 alone, as a statement's other conditions, no data
/// among them, are told by its result, and not the last when the
/// statement completed with it, which its result names.
fn noticed<S: Read + Write>(connection: &mut Connection<S>) -> Vec<Notice> {
    let mut all = connection.take_notices();
    let mut conditions = connection.take_conditions();
    if conditions.last().is_some_and(|c| Some(c.sqlstate.as_str()) == connection.warning()) {
        conditions.pop();
    }
    let warnings = conditions.into_iter().filter(|c| c.sqlstate.starts_with(WARNING_CLASS));
    all.extend(warnings.map(|c| Notice {
        sqlstate: c.sqlstate,
        message: c.message,
        diagnostics: c.diagnostics.map(|d| *d).unwrap_or_default(),
    }));
    all
}

/// Whether `connection`'s server answers its Ping, if it takes one.
fn pinged<S: Read + Write>(connection: &mut Connection<S>) -> Option<Result<(), Failure>> {
    connection.granted().has(PING).then(|| connection.ping())
}

impl<S: Read + Write> Statements for Connection<S> {
    fn server_time(&self) -> Option<ServerTime> {
        Connection::server_time(self).map(shown)
    }

    fn ping(&mut self) -> Option<Result<(), Failure>> {
        pinged(self)
    }

    fn status(&self, name: &str) -> Option<String> {
        self.setting(name).map(str::to_owned)
    }

    fn take_notices(&mut self) -> Vec<Notice> {
        noticed(self)
    }

    fn export(&mut self, sql: &str, out: &mut dyn Write, layout: &mut dyn Layout) -> Result<u64, Failure> {
        self.export_with(sql, out, layout)
    }

    fn query_into(&mut self, sql: &str, sink: &mut dyn Sink) -> Result<Streamed, Failure> {
        Connection::query_into(self, sql, sink)
    }

    fn fetch_into(&mut self, sql: &str, rows: u32, sink: &mut dyn Sink) -> Result<Streamed, Failure> {
        fetched(self, sql, rows, sink).unwrap_or_else(|| Connection::query_into(self, sql, sink))
    }

    fn describe_into(&mut self, sql: &str, sink: &mut dyn Sink) -> Result<Streamed, Failure> {
        described(self, sql, sink)
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

    fn debug(&mut self, asked: &DebugCommand, null: &str) -> Result<String, Failure> {
        debugged(self, asked, null)
    }
}

impl<C: Connector> Statements for Agent<C> {
    fn server_time(&self) -> Option<ServerTime> {
        self.current_connection()?.server_time().map(shown)
    }

    fn ping(&mut self) -> Option<Result<(), Failure>> {
        pinged(self.current().ok()?)
    }

    fn status(&self, name: &str) -> Option<String> {
        self.current_connection()?.setting(name).map(str::to_owned)
    }

    fn take_notices(&mut self) -> Vec<Notice> {
        self.current().map(noticed).unwrap_or_default()
    }

    fn export(&mut self, sql: &str, out: &mut dyn Write, layout: &mut dyn Layout) -> Result<u64, Failure> {
        self.current()?.export_with(sql, out, layout)
    }

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

    fn describe_into(&mut self, sql: &str, sink: &mut dyn Sink) -> Result<Streamed, Failure> {
        described(self.current()?, sql, sink)
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

    fn spare(&mut self) -> Result<Connected, Failure> {
        Agent::spare(self)
    }

    fn debug(&mut self, asked: &DebugCommand, null: &str) -> Result<String, Failure> {
        debugged(self.current()?, asked, null)
    }
}

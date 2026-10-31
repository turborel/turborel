//! \[DESIGN\] A session's statements run, one at a time, in the order
//! given (RULES.md D46): each a `<direct SQL statement>` [ISO/IEC
//! 9075:1992 §20.1], its rows shown as they come, its failure told and the
//! run gone on. A run not typed stops at a cancelled statement, or with
//! `--stop-on-error` at the first that fails; any run stops once the
//! connection is lost. A file `--import` names has its rows imported, in
//! its turn among the statements (RULES.md D59). Split from lib.rs
//! (RULES.md R-SIZE-4).

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::Path;

use turborel::error::{Code, SqlError};
use turborel_client::agent::{Agent, Connector};
use turborel_client::connection::{Connection, Sink, Streamed};
use turborel_client::import::{self, Cause, Delimited, Import, ImportError, Imported, Rejected, rows};
use turborel_client::{Canceller, Failure};

use crate::args::{Input, Options};
use crate::interrupt::Interrupts;
use crate::show::Table;
use crate::split::Splitter;

/// The prompt for a statement.
pub const PROMPT: &str = "turborel> ";
/// The prompt for the rest of one begun.
pub const MORE: &str = "turborel- ";

/// Where a run reads and writes.
pub struct Io<'a> {
    /// Standard input.
    pub input: &'a mut dyn BufRead,
    /// Whether it is typed at a terminal: prompted, and not stopped by a
    /// failure.
    pub typed: bool,
    /// Where results go.
    pub out: &'a mut dyn Write,
    /// Where failures go.
    pub err: &'a mut dyn Write,
    /// What Ctrl-C cancels, if it is taken: kept to the current
    /// connection's statement as connection statements change it.
    pub interrupts: Option<&'a Interrupts>,
}

/// Whether a run goes on.
enum Next {
    Go,
    Stop,
}

/// What went wrong with a statement.
enum Wrong {
    /// It failed.
    Failed(Failure),
    /// Its results could not be written.
    Unwritten(io::Error),
}

/// What runs a run's statements: a connection, or an SQL-agent's
/// connections [ISO/IEC 9075:1992 §15], which takes CONNECT, SET
/// CONNECTION and DISCONNECT itself.
pub trait Statements {
    /// Runs `sql`, a query's rows given to `sink`.
    ///
    /// # Errors
    ///
    /// The statement's failure.
    fn query_into(&mut self, sql: &str, sink: &mut dyn Sink) -> Result<Streamed, Failure>;

    /// What cancels the statement running, if anything can.
    fn canceller(&self) -> Option<Canceller>;

    /// Whether the connection statements run over is lost.
    fn is_lost(&self) -> bool;

    /// The SQLSTATE of the warning the last statement completed with, if
    /// it did.
    fn warning(&self) -> Option<&str>;

    /// Imports `source`'s rows as `how` says (RULES.md D59), each row
    /// skipped given to `rejected`.
    ///
    /// # Errors
    ///
    /// Why the import stopped, and what of it was kept.
    fn import(&mut self, source: &mut FileSource, how: &Import, rejected: &mut dyn FnMut(Rejected)) -> Result<Imported, ImportError>;
}

/// What a file `--import` names is read by.
pub type FileSource = Delimited<BufReader<File>>;

/// An import's failure `failure` before it began: nothing kept.
fn unbegun(failure: Failure) -> ImportError {
    ImportError { position: None, cause: Cause::Sql(Box::new(failure)), committed: 0, rolled_back: 0 }
}

impl<S: Read + Write> Statements for Connection<S> {
    fn query_into(&mut self, sql: &str, sink: &mut dyn Sink) -> Result<Streamed, Failure> {
        Connection::query_into(self, sql, sink)
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

    fn import(&mut self, source: &mut FileSource, how: &Import, mut rejected: &mut dyn FnMut(Rejected)) -> Result<Imported, ImportError> {
        import::import(self, source, how, &mut rejected)
    }
}

impl<C: Connector> Statements for Agent<C> {
    fn query_into(&mut self, sql: &str, sink: &mut dyn Sink) -> Result<Streamed, Failure> {
        Agent::query_into(self, sql, sink)
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

    fn import(&mut self, source: &mut FileSource, how: &Import, mut rejected: &mut dyn FnMut(Rejected)) -> Result<Imported, ImportError> {
        import::import(self.current().map_err(unbegun)?, source, how, &mut rejected)
    }
}

/// A run, under way.
struct Shell<'a, S: Statements + ?Sized> {
    connection: &'a mut S,
    options: &'a Options,
    /// Standard input, while it is not being read.
    input: Option<&'a mut dyn BufRead>,
    typed: bool,
    out: &'a mut dyn Write,
    err: &'a mut dyn Write,
    interrupts: Option<&'a Interrupts>,
    /// Whether anything failed.
    failed: bool,
}

/// Runs `options`' inputs over `connection`, as `io` says: whether every
/// statement succeeded. What failed is told to `io.err`.
pub fn run<S: Statements + ?Sized>(connection: &mut S, options: &Options, io: Io<'_>) -> bool {
    let Io { input, typed, out, err, interrupts } = io;
    let mut shell = Shell { connection, options, input: Some(input), typed, out, err, interrupts, failed: false };
    for named in &options.inputs {
        if let Next::Stop = shell.input(named) {
            break;
        }
    }
    !shell.failed
}

/// Whether `failure` is of a statement cancelled (RULES.md D44).
fn cancelled(failure: &Failure) -> bool {
    failure.sqlcode == SqlError::new(Code::Cancelled, "").sqlcode()
}

impl<S: Statements + ?Sized> Shell<'_, S> {
    /// Runs the statements of `input`.
    fn input(&mut self, input: &Input) -> Next {
        match input {
            Input::Command(sql) => self.text(&mut sql.as_bytes(), false),
            Input::File(path) => match File::open(path) {
                Ok(file) => self.text(&mut BufReader::new(file), false),
                Err(e) => self.tell(&format!("cannot read {}: {e}", path.display()), Next::Stop),
            },
            // Read once: a second time, it is at its end.
            Input::Standard => match self.input.take() {
                Some(standard) => self.text(standard, self.typed),
                None => Next::Go,
            },
            Input::Import(path) => self.import(path),
        }
    }

    /// Imports the rows of the file at `path` (RULES.md D59), as the
    /// options say: each row skipped told as a failure, and what was
    /// imported said.
    fn import(&mut self, path: &Path) -> Next {
        let Some(importing) = &self.options.import else { return self.tell("[DESIGN] --import needs --table", Next::Stop) };
        let file = match File::open(path) {
            Ok(file) => file,
            Err(e) => return self.tell(&format!("cannot read {}: {e}", path.display()), Next::Stop),
        };
        let mut source = Delimited::new(BufReader::new(file), importing.dialect.clone());
        let shown = path.display();
        let err = &mut *self.err;
        // Nothing is left to tell a failure to write a failure to.
        let mut told = |row: Rejected| drop(writeln!(err, "turborel: {shown}: {row}"));
        let done = self.connection.import(&mut source, &importing.into, &mut told);
        match done {
            Ok(imported) => {
                // A row skipped is a failure, told as it was skipped.
                self.failed |= imported.rejected > 0;
                let skipped = if imported.rejected > 0 { format!(", {} skipped", imported.rejected) } else { String::new() };
                let said = writeln!(self.out, "OK, {} imported{skipped}", rows(imported.rows)).and_then(|()| self.out.flush());
                said.map_or_else(|e| self.tell(&format!("cannot write: {e}"), Next::Stop), |()| Next::Go)
            }
            Err(e) => {
                let gave_up = match &e.cause {
                    Cause::Sql(f) => cancelled(f),
                    Cause::Input(_) => false,
                };
                let stop = self.connection.is_lost() || self.options.stop_on_error || gave_up;
                self.tell(&format!("{shown}: {e}"), if stop { Next::Stop } else { Next::Go })
            }
        }
    }

    /// Tells `msg` as a failure: the run then does `next`.
    fn tell(&mut self, msg: &str, next: Next) -> Next {
        self.failed = true;
        // Nothing is left to tell a failure to write a failure to.
        let _ = writeln!(self.err, "turborel: {msg}");
        next
    }

    /// Runs the statements read from `text`, line by line: prompted, if
    /// `typed`.
    fn text(&mut self, text: &mut dyn BufRead, typed: bool) -> Next {
        let mut splitter = Splitter::default();
        let mut line = String::new();
        loop {
            if typed && self.prompt(if splitter.pending() { MORE } else { PROMPT }).is_err() {
                return Next::Stop;
            }
            line.clear();
            match text.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {}
                Err(e) => return self.tell(&format!("cannot read: {e}"), Next::Stop),
            }
            for sql in splitter.feed(&line) {
                if let Next::Stop = self.statement(&sql, typed) {
                    return Next::Stop;
                }
            }
        }
        if typed && writeln!(self.out).is_err() {
            return Next::Stop;
        }
        splitter.finish().map_or(Next::Go, |sql| self.statement(&sql, typed))
    }

    /// Writes `prompt`.
    fn prompt(&mut self, prompt: &str) -> io::Result<()> {
        let written = write!(self.out, "{prompt}").and_then(|()| self.out.flush());
        written.inspect_err(|e| {
            self.tell(&format!("cannot write: {e}"), Next::Stop);
        })
    }

    /// Runs `sql`, typed or not.
    fn statement(&mut self, sql: &str, typed: bool) -> Next {
        let ran = self.ran(sql);
        if let Some(interrupts) = self.interrupts {
            interrupts.cancel_by(self.connection.canceller());
        }
        match ran {
            Ok(()) => Next::Go,
            Err(Wrong::Unwritten(e)) => self.tell(&format!("cannot write: {e}"), Next::Stop),
            Err(Wrong::Failed(failure)) => {
                let stop = self.connection.is_lost() || (!typed && (self.options.stop_on_error || cancelled(&failure)));
                self.tell(&failure.to_string(), if stop { Next::Stop } else { Next::Go })
            }
        }
    }

    /// Runs `sql`, its results written.
    fn ran(&mut self, sql: &str) -> Result<(), Wrong> {
        let mut table = Table::new(self.out, &self.options.null);
        match self.connection.query_into(sql, &mut table).map_err(Wrong::Failed)? {
            Streamed::Rows(count) => table.finish(count, self.connection.warning()).map_err(Wrong::Unwritten),
            Streamed::Done(completed) => {
                drop(table);
                let said = match (completed.sqlcode, completed.is_warning()) {
                    (0, false) => "OK".to_owned(),
                    (0, true) => format!("OK, with a warning: SQLSTATE {}", completed.sqlstate),
                    (_, _) => "no data".to_owned(),
                };
                writeln!(self.out, "{said}").and_then(|()| self.out.flush()).map_err(Wrong::Unwritten)
            }
        }
    }
}

//! \[DESIGN\] A session's statements run, one at a time, in the order
//! given (RULES.md D46), each a `<direct SQL statement>` [ISO/IEC
//! 9075:1992 §20.1], its rows shown as they come, its failure told and the
//! run gone on. A run not typed stops at a cancelled statement, or with
//! STOP_ON_ERROR at the first that fails; any run stops once the
//! connection is lost. A file `--import` names has its rows imported, in
//! its turn among the statements (RULES.md D59). Lines may hold backslash
//! commands (RULES.md D210), read variables (D212) and open conditional
//! blocks; how a run ends is its outcome (D215).

mod buffer;
mod commands;
mod files;
mod output;
mod session;
mod statement;
mod statements;

use std::fs::File;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use turborel::error::{Code, SqlError};
use turborel_client::Failure;

use crate::args::{Input, Options};
use crate::complete::Names;
use crate::conditional::Conditions;
use crate::format::Layout;
use crate::interrupt::Interrupts;
use crate::prompt::Status;
use crate::split::{Piece, Splitter};
use crate::vars::{self, Variables};

pub(crate) use session::{secured, set_password};
pub use statement::Kept;
pub use statements::{FileSource, Statements};

/// The prompt for a statement.
pub const PROMPT: &str = "turborel> ";
/// The prompt for the rest of one begun.
pub const MORE: &str = "turborel- ";

/// A screen's size, in columns and lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Screen {
    /// Its columns.
    pub width: usize,
    /// Its lines.
    pub height: usize,
}

/// The words a completion may put in place of the word typed, and where
/// in the text that word begins.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Completion {
    /// Where the word being completed begins, in bytes.
    pub start: usize,
    /// What may take its place.
    pub candidates: Vec<String>,
}

/// What reads typed lines, a line editor (`line.rs`).
pub trait Lines {
    /// A line typed after `prompt`, without its newline; none at the end
    /// of input. `complete` gives the completions of the text before the
    /// cursor.
    ///
    /// # Errors
    ///
    /// The terminal cannot be read.
    fn read_line(&mut self, prompt: &str, complete: &mut dyn FnMut(&str) -> Completion) -> io::Result<Option<String>>;

    /// Keeps `entry`, a statement or command as typed, in the history, at
    /// most `size` entries kept, in the file `file` if one is named.
    fn remember(&mut self, entry: &str, size: usize, file: Option<&std::path::Path>);
}

/// What reads a secret typed, as a password, after a prompt.
pub type Secret<'a> = &'a mut dyn FnMut(&str) -> io::Result<String>;

/// What a terminal adds to a run, if it has one.
#[derive(Default)]
pub struct Terminal<'a> {
    /// The screen's size, if output is a screen, for the pager, wrapped
    /// output and expanded output decided by width.
    pub screen: Option<Screen>,
    /// The line editor typed lines are read by, if any.
    pub editor: Option<&'a mut dyn Lines>,
    /// What reads a password typed, if anything can.
    pub secret: Option<Secret<'a>>,
}

/// Where a run reads and writes.
pub struct Io<'a> {
    /// Standard input.
    pub input: &'a mut dyn BufRead,
    /// Whether it is typed at a terminal, so prompted and not stopped by a
    /// failure.
    pub typed: bool,
    /// Where results go.
    pub out: &'a mut dyn Write,
    /// Where failures go.
    pub err: &'a mut dyn Write,
    /// What Ctrl-C cancels, if it is taken, kept to the current
    /// connection's statement as connection statements change it.
    pub interrupts: Option<&'a Interrupts>,
    /// What the terminal adds, if any.
    pub terminal: Terminal<'a>,
}

/// \[DESIGN\] RULES.md D215: how a run ended, and the program's exit
/// status of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Every statement succeeded: 0.
    Succeeded,
    /// Something failed, and the run went on, or could not begin: 1.
    Failed,
    /// The connection was lost: 2.
    Lost,
    /// A statement failed and STOP_ON_ERROR ended the run there: 3.
    Stopped,
}

impl Outcome {
    /// The exit status of it.
    pub fn code(self) -> u8 {
        match self {
            Outcome::Succeeded => 0,
            Outcome::Failed => 1,
            Outcome::Lost => 2,
            Outcome::Stopped => 3,
        }
    }
}

/// Whether a run goes on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// The most files included in one another at once (RULES.md R-CF-6).
const MAX_INCLUDES: usize = 16;

/// A run, under way.
struct Shell<'a, S: Statements + ?Sized, C: FnMut() -> Duration> {
    connection: &'a mut S,
    options: &'a Options,
    /// Standard input, while it is not being read.
    input: Option<&'a mut dyn BufRead>,
    typed: bool,
    out: &'a mut dyn Write,
    err: &'a mut dyn Write,
    interrupts: Option<&'a Interrupts>,
    terminal: Terminal<'a>,
    /// Whether the text being read is typed at a terminal.
    typing: bool,
    /// A monotonic elapsed clock, `Instant` in a run, supplied in tests
    /// (RULES.md R-TEST-5).
    now: C,
    vars: Variables,
    layout: Layout,
    /// Where results go, if `\o` sent them elsewhere.
    redirect: Option<output::Redirect>,
    /// The file `\tee` copies output to.
    tee: Option<File>,
    /// The last statement run, for `\g`, `\watch` and `\e`.
    last: Option<String>,
    /// The files being read, innermost last, for `\ir`.
    files: Vec<PathBuf>,
    /// The names tab completion offers, once read.
    names: Option<Names>,
    /// What the prompt shows of the session.
    status: Option<Status>,
    /// The default schema SET SCHEMA set, if it was.
    schema: Option<String>,
    /// How many connections `\c` made.
    connections: u32,
    /// Whether anything failed.
    failed: bool,
    /// How the run ended, if it ended early.
    ended: Option<Outcome>,
}

/// Runs `options`' inputs over `connection`, as `io` says, and returns
/// whether every statement succeeded. What failed is told to `io.err`.
pub fn run<'a, S: Statements + ?Sized>(connection: &'a mut S, options: &'a Options, io: Io<'a>) -> bool {
    outcome(connection, options, io) == Outcome::Succeeded
}

/// Runs as `run` does and returns how the run ended (RULES.md D215).
pub fn outcome<'a, S: Statements + ?Sized>(connection: &'a mut S, options: &'a Options, io: Io<'a>) -> Outcome {
    let origin = Instant::now();
    run_with_clock(connection, options, io, || origin.elapsed())
}

/// Runs with a supplied elapsed clock so timing tests need no real time.
fn run_with_clock<'a, S: Statements + ?Sized>(
    connection: &'a mut S,
    options: &'a Options,
    io: Io<'a>,
    now: impl FnMut() -> Duration,
) -> Outcome {
    let Io { input, typed, out, err, interrupts, terminal } = io;
    let mut shell = Shell {
        connection,
        options,
        input: Some(input),
        typed,
        out,
        err,
        interrupts,
        terminal,
        typing: false,
        now,
        vars: Variables::default(),
        layout: Layout { null: options.null.clone(), ..options.settings.layout.clone() },
        redirect: None,
        tee: None,
        last: None,
        files: Vec::new(),
        names: None,
        status: None,
        schema: None,
        connections: 0,
        failed: false,
        ended: None,
    };
    shell.begin();
    for named in &options.inputs {
        if shell.input(named) == Next::Stop {
            break;
        }
    }
    shell.end()
}

/// Whether `failure` is of a statement cancelled (RULES.md D44).
fn cancelled(failure: &Failure) -> bool {
    failure.sqlcode == SqlError::new(Code::Cancelled, "").sqlcode()
}

impl<S: Statements + ?Sized, C: FnMut() -> Duration> Shell<'_, S, C> {
    /// Sets the variables a run begins with, the client's defaults, then
    /// those `-v` set.
    fn begin(&mut self) {
        self.vars.set(vars::STOP_ON_ERROR, if self.options.stop_on_error { "on" } else { "off" });
        self.vars.set(vars::AUTOCOMMIT, "off");
        self.vars.set(vars::PROMPT, PROMPT);
        self.vars.set(vars::PROMPT_MORE, MORE);
        self.vars.set(vars::VERBOSITY, "default");
        self.vars.set(vars::HISTORY_SIZE, &crate::line::history::DEFAULT_SIZE.to_string());
        if let Some(history) = &self.options.settings.environment.history {
            self.vars.set(vars::HISTORY_FILE, &history.display().to_string());
        }
        if let Some((server, user)) = self.connection.session() {
            self.vars.set(vars::SERVER, &server);
            self.vars.set(vars::USER, &user);
        }
        for (name, value) in &self.options.settings.variables {
            self.vars.set(name, value);
        }
    }

    /// Ends the run, a single transaction committed or rolled back, the
    /// output redirected closed, and returns how it ended.
    fn end(&mut self) -> Outcome {
        if self.options.settings.single_transaction && self.ended != Some(Outcome::Lost) {
            let end = if self.failed { "ROLLBACK" } else { "COMMIT" };
            if self.statement(end, false) == Next::Stop {
                self.failed = true;
            }
        }
        self.close_output();
        match (self.ended, self.failed) {
            (Some(outcome), _) => outcome,
            (None, true) => Outcome::Failed,
            (None, false) => Outcome::Succeeded,
        }
    }

    /// Runs the statements of `input`.
    fn input(&mut self, input: &Input) -> Next {
        match input {
            Input::Command(sql) => self.text(&mut sql.as_bytes(), false),
            Input::File(path) => self.file(path),
            // Read once, as a second time it is at its end.
            Input::Standard => match self.input.take() {
                Some(standard) => self.text(standard, self.typed),
                None => Next::Go,
            },
            Input::Import(path) => self.import(path),
        }
    }

    /// Tells `msg` as a failure, and the run then does `next`.
    fn tell(&mut self, msg: &str, next: Next) -> Next {
        self.failed = true;
        self.told(&format!("turborel: {msg}"));
        next
    }

    /// Writes `line` to the errors, and to the file `\tee` names.
    fn told(&mut self, line: &str) {
        // Nothing is left to tell a failure to write a failure to.
        let _ = writeln!(self.err, "{line}");
        if let Some(tee) = &mut self.tee {
            // The copy is the user's convenience; the error itself was told.
            let _ = writeln!(tee, "{line}");
        }
    }

    /// Stops the run as `outcome`, telling `msg`.
    fn stop(&mut self, msg: &str, outcome: Outcome) -> Next {
        self.ended = Some(outcome);
        self.tell(msg, Next::Stop)
    }

    /// Runs the statements and commands read from `text`, line by line:
    /// prompted, if `typed`.
    fn text(&mut self, text: &mut dyn BufRead, typed: bool) -> Next {
        let was = std::mem::replace(&mut self.typing, typed);
        let next = self.lines(text, typed);
        self.typing = was;
        next
    }

    /// Runs the lines of `text`, as `text` says.
    fn lines(&mut self, text: &mut dyn BufRead, typed: bool) -> Next {
        let mut splitter = Splitter::default();
        let mut conditions = Conditions::default();
        let mut entry = String::new();
        loop {
            let line = match self.line(text, typed, &splitter) {
                Ok(Some(line)) => line,
                Ok(None) => break,
                Err(next) => return next,
            };
            if typed {
                entry.push_str(&line);
            }
            if self.line_read(&line, &mut splitter, &mut conditions) == Next::Stop {
                return Next::Stop;
            }
            if typed && !splitter.pending() {
                self.remember(&std::mem::take(&mut entry));
            }
        }
        if typed && self.print_line("").is_err() {
            return Next::Stop;
        }
        if conditions.is_open() {
            self.tell("\\if: a block is not closed by \\endif", Next::Go);
        }
        splitter.finish().map_or(Next::Go, |sql| self.statement(&sql, typed))
    }

    /// The next line of `text`, with its newline, prompted if `typed`;
    /// none at its end; how the run goes on if it cannot be read.
    fn line(&mut self, text: &mut dyn BufRead, typed: bool, splitter: &Splitter) -> Result<Option<String>, Next> {
        let prompt = if typed { self.prompt(splitter) } else { String::new() };
        if typed && let Some(editor) = self.terminal.editor.as_deref_mut() {
            let names = &mut self.names;
            let connection = &mut *self.connection;
            let mut complete = |before: &str| crate::complete::complete(before, names, connection);
            return match editor.read_line(&prompt, &mut complete) {
                Ok(line) => Ok(line.map(|l| format!("{l}\n"))),
                Err(e) => Err(self.tell(&format!("cannot read: {e}"), Next::Stop)),
            };
        }
        if typed && let Err(e) = write!(self.out, "{prompt}").and_then(|()| self.out.flush()) {
            return Err(self.tell(&format!("cannot write: {e}"), Next::Stop));
        }
        let mut line = String::new();
        match text.read_line(&mut line) {
            Ok(0) => Ok(None),
            Ok(_) => Ok(Some(line)),
            Err(e) => Err(self.tell(&format!("cannot read: {e}"), Next::Stop)),
        }
    }

    /// Runs what `line` holds, or in a branch not taken its conditional
    /// command alone, if it is one.
    fn line_read(&mut self, line: &str, splitter: &mut Splitter, conditions: &mut Conditions) -> Next {
        if !conditions.active() {
            return match line.trim_start().strip_prefix('\\') {
                Some(text) => self.conditional(text, conditions).unwrap_or(Next::Go),
                None => Next::Go,
            };
        }
        for piece in splitter.pieces(line, Some(&self.vars)) {
            let next = match piece {
                Piece::Statement(sql) => self.statement(&sql, self.typing),
                Piece::Command(text) => self.command(&text, splitter, conditions),
            };
            if next == Next::Stop {
                return Next::Stop;
            }
        }
        Next::Go
    }

    /// Keeps `entry` in the line editor's history.
    fn remember(&mut self, entry: &str) {
        let size = self.vars.number(vars::HISTORY_SIZE).map_or(crate::line::history::DEFAULT_SIZE, |n| usize::try_from(n).unwrap_or(0));
        let file = self.vars.get(vars::HISTORY_FILE).filter(|f| !f.is_empty()).map(PathBuf::from);
        if let Some(editor) = self.terminal.editor.as_deref_mut() {
            editor.remember(entry.trim_end_matches('\n'), size, file.as_deref());
        }
    }

    /// Writes `line` and a newline to the output, copied to the file `\tee`
    /// names, as all that is written is.
    fn print_line(&mut self, line: &str) -> io::Result<()> {
        let written = writeln!(self.out, "{line}").and_then(|()| self.out.flush()).and_then(|()| match &mut self.tee {
            Some(tee) => writeln!(tee, "{line}"),
            None => Ok(()),
        });
        written.inspect_err(|e| {
            self.tell(&format!("cannot write: {e}"), Next::Stop);
        })
    }
}

#[cfg(test)]
mod tests;

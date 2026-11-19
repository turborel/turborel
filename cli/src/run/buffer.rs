//! \[DESIGN\] The commands of the statement begun (RULES.md D210, D215,
//! D219): `\g` and `\G` run it, or the last statement if none is begun,
//! its results sent elsewhere or expanded; `\gset` stores its one row's
//! columns as variables; `\gexec` runs each value of its result as a
//! statement; `\watch` runs it again and again; `\e` edits it.

use std::path::Path;
use std::process::Command as Program;
use std::time::Duration;

use super::statement::How;
use super::{Next, Shell, Statements};
use crate::command::Target;
use crate::format::cell;
use crate::split::Splitter;

/// The editor, unless one is named.
const DEFAULT_EDITOR: &str = "vi";
/// What the file `\e` edits the statement begun in is named, before the
/// process's number.
const EDIT_FILE: &str = "turborel-edit-";
/// How long `\watch` sleeps at a time, so Ctrl-C ends it soon.
const WATCH_STEP: Duration = Duration::from_millis(100);

impl<S: Statements + ?Sized, C: FnMut() -> Duration> Shell<'_, S, C> {
    /// The statement begun in `splitter`, taken; else the last one run.
    fn begun(&mut self, splitter: &mut Splitter, command: &str) -> Option<String> {
        let found = splitter.take_pending().or_else(|| self.last.clone());
        if found.is_none() {
            self.tell(&format!("\\{command}: no statement is begun, and none was run"), Next::Go);
        }
        found
    }

    /// `\g` and `\G`: the statement begun, or the last, run, its results
    /// sent to `target`, expanded if `expanded`.
    pub(super) fn go(&mut self, splitter: &mut Splitter, target: Target, expanded: bool) -> Next {
        let Some(sql) = self.begun(splitter, "g") else { return Next::Go };
        self.statement_as(&sql, self.typing, &How { target, expanded })
    }

    /// `\gset`: the statement's one row stored, each column the variable
    /// `prefix` and its name; a null unsets it.
    pub(super) fn gset(&mut self, splitter: &mut Splitter, prefix: &str) -> Next {
        let Some(sql) = self.begun(splitter, "gset") else { return Next::Go };
        self.last = Some(sql.clone());
        let Some(kept) = self.kept(&sql) else { return Next::Go };
        let [row] = kept.rows.as_slice() else {
            return self.tell(&format!("\\gset: the query gave {} rows, not one", kept.rows.len()), Next::Go);
        };
        for (column, value) in kept.columns.iter().zip(row) {
            let name = format!("{prefix}{}", column.name.as_deref().unwrap_or_default());
            if !crate::vars::is_name(&name) {
                return self.tell(&format!("\\gset: {name:?} is no variable's name"), Next::Go);
            }
            if matches!(value, turborel::value::Value::Null) {
                self.vars.unset(&name);
            } else {
                self.vars.set(&name, &cell(value, ""));
            }
        }
        Next::Go
    }

    /// `\gexec`: the statement run, then each value of its result, row by
    /// row, as a statement; nulls passed over.
    pub(super) fn gexec(&mut self, splitter: &mut Splitter) -> Next {
        let Some(sql) = self.begun(splitter, "gexec") else { return Next::Go };
        self.last = Some(sql.clone());
        let Some(kept) = self.kept(&sql) else { return Next::Go };
        for value in kept.rows.iter().flatten() {
            if matches!(value, turborel::value::Value::Null) {
                continue;
            }
            if self.statement(&cell(value, ""), self.typing) == Next::Stop {
                return Next::Stop;
            }
        }
        Next::Go
    }

    /// `\watch`: the statement run every `every`, `count` times if given,
    /// else until Ctrl-C or a failure; a line naming each run before it.
    pub(super) fn watch(&mut self, splitter: &mut Splitter, every: Duration, count: Option<u64>) -> Next {
        let Some(sql) = self.begun(splitter, "watch") else { return Next::Go };
        let pressed = self.interrupts.map(crate::interrupt::Interrupts::presses);
        let mut run = 0_u64;
        while count.is_none_or(|c| run < c) {
            run = run.saturating_add(1);
            if run > 1 && !self.slept(every, pressed) {
                break;
            }
            if self.say(&format!("watch every {} s, run {run}", every.as_secs_f64())) == Next::Stop {
                return Next::Stop;
            }
            let failed = self.failed;
            self.failed = false;
            let next = self.statement(&sql, self.typing);
            let this = std::mem::replace(&mut self.failed, failed);
            if next == Next::Stop || this {
                self.failed |= this;
                return next;
            }
        }
        Next::Go
    }

    /// Sleeps `every`, a step at a time: whether it slept it all, Ctrl-C
    /// not pressed since `pressed` presses.
    fn slept(&self, every: Duration, pressed: Option<u64>) -> bool {
        let mut left = every;
        while !left.is_zero() {
            if self.interrupts.map(crate::interrupt::Interrupts::presses) != pressed {
                return false;
            }
            let step = left.min(WATCH_STEP);
            std::thread::sleep(step);
            left = left.saturating_sub(step);
        }
        self.interrupts.map(crate::interrupt::Interrupts::presses) == pressed
    }

    /// `\e`: the statement begun, or the last, or the file `path`, edited
    /// in the editor, then read as if typed.
    pub(super) fn edit(&mut self, splitter: &mut Splitter, path: Option<&Path>) -> Next {
        let made = std::env::temp_dir().join(format!("{EDIT_FILE}{}.sql", std::process::id()));
        let file = path.unwrap_or(&made);
        if path.is_none() {
            let text = splitter.take_pending().or_else(|| self.last.clone()).unwrap_or_default();
            if let Err(e) = std::fs::write(file, format!("{text}\n")) {
                return self.tell(&format!("\\e: cannot write {}: {e}", file.display()), Next::Go);
            }
        }
        let editor = self.options.settings.environment.editor.clone().unwrap_or_else(|| DEFAULT_EDITOR.to_owned());
        let mut words = editor.split_whitespace();
        let program = words.next().unwrap_or(DEFAULT_EDITOR);
        let edited = Program::new(program).args(words).arg(file).status();
        let text = edited.and_then(|status| {
            if status.success() {
                std::fs::read_to_string(file)
            } else {
                Err(std::io::Error::other(format!("the editor ended with {status}")))
            }
        });
        if path.is_none() {
            // A file of the client's own, its text read or not.
            drop(std::fs::remove_file(file));
        }
        match text {
            Ok(text) => self.edited(&text, splitter),
            Err(e) => self.tell(&format!("\\e: {e}"), Next::Go),
        }
    }

    /// Reads `text`, edited, into `splitter` as if typed: statements it
    /// ends are run, and the rest is the statement begun.
    fn edited(&mut self, text: &str, splitter: &mut Splitter) -> Next {
        let mut conditions = crate::conditional::Conditions::default();
        for line in text.split_inclusive('\n') {
            if self.line_read(line, splitter, &mut conditions) == Next::Stop {
                return Next::Stop;
            }
        }
        Next::Go
    }
}

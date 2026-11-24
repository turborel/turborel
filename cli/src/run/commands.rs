//! \[DESIGN\] A backslash command run (RULES.md D210), its line read, its
//! arguments' variables read, and what it names done.

use std::time::Duration;

use super::{Next, Shell, Statements};
use crate::command::{self, Command, Expansion};
use crate::conditional::Conditions;
use crate::format::{Expanded, OPTIONS, Style};
use crate::split::Splitter;
use crate::vars::truth;

/// The text of `\?`.
pub const COMMANDS: &str = include_str!("commands.txt");

impl<S: Statements + ?Sized, C: FnMut() -> Duration> Shell<'_, S, C> {
    /// Runs the command whose line, after its `\`, is `text`, the
    /// statement begun in `splitter`, the blocks open in `conditions`.
    pub(super) fn command(&mut self, text: &str, splitter: &mut Splitter, conditions: &mut Conditions) -> Next {
        if let Some(next) = self.conditional(text, conditions) {
            return next;
        }
        let (name, rest) = command::split_name(text);
        let parsed = command::arguments(rest, &self.vars).and_then(|args| command::parse(name, args, rest));
        match parsed {
            Ok(found) => self.run_command(found, splitter),
            Err(e) => self.tell(&e, Next::Go),
        }
    }

    /// Runs `text` if it is a conditional command, `\if`, `\elif`, `\else`
    /// or `\endif`, in `conditions`, and returns how the run goes on if it
    /// was one.
    pub(super) fn conditional(&mut self, text: &str, conditions: &mut Conditions) -> Option<Next> {
        let (name, rest) = command::split_name(text);
        let value = || -> Result<bool, String> {
            let words = command::arguments(rest, &self.vars)?.join(" ");
            truth(&words).ok_or_else(|| format!("\\{name}: {words:?} is neither true nor false"))
        };
        let done = match name {
            // A value that is neither opens the block even so, as false,
            // so its branches are not all run.
            "if" => {
                let read = value();
                let opened = conditions.begin(read.as_ref().is_ok_and(|v| *v));
                read.and(opened)
            }
            "elif" => value().and_then(|v| conditions.elif(v)),
            "else" => conditions.otherwise(),
            "endif" => conditions.end(),
            _ => return None,
        };
        Some(done.map_or_else(|e| self.tell(&e, Next::Go), |()| Next::Go))
    }

    /// Runs `found`, the statement begun in `splitter`.
    fn run_command(&mut self, found: Command, splitter: &mut Splitter) -> Next {
        match found {
            Command::Quit => Next::Stop,
            Command::Commands => self.say(COMMANDS.trim_end()),
            Command::Syntax(topic) => self.syntax(topic.as_deref()),
            Command::Set(assignment) => self.set(assignment),
            Command::Unset(name) => self.unset(&name),
            Command::Echo(text) => self.say(&text),
            Command::Include { path, relative } => self.include(&path, relative),
            Command::Output(target) => self.redirect(&target),
            Command::Go { target, expanded } => self.go(splitter, target, expanded),
            Command::Gset(prefix) => self.gset(splitter, &prefix),
            Command::Gexec => self.gexec(splitter),
            Command::Gdesc => self.gdesc(splitter),
            Command::Watch { every, count } => self.watch(splitter, every, count),
            Command::Edit(path) => self.edit(splitter, path.as_deref()),
            Command::Print => self.say(splitter.pending_text().trim_end()),
            Command::Reset => Self::reset(splitter),
            Command::Expanded(how) => self.expanded(how),
            Command::Aligned => self.aligned(),
            Command::Tuples(on) => self.tuples(on),
            Command::Format(option) => self.format(option),
            Command::Describe(name, pattern) => self.describe(&name, pattern.as_deref()),
            Command::Import(args) => self.import_command(&args),
            Command::Export(args) => self.export(&args),
            Command::ConnInfo => self.conninfo(),
            Command::Connect(server, user) => self.reconnect(server.as_deref(), user.as_deref()),
            Command::Password(user) => self.password(user.as_deref()),
            Command::Tee(path) => self.tee(&path),
            Command::NoTee => self.no_tee(),
            Command::Delimiter(text) => splitter.set_delimiter(text).map_or_else(|e| self.tell(&e, Next::Go), |()| Next::Go),
            Command::Debug(asked) => self.debug(&asked),
            Command::If(_) | Command::Elif(_) | Command::Else | Command::Endif => Next::Go,
        }
    }

    /// `\unset`.
    fn unset(&mut self, name: &str) -> Next {
        self.vars.unset(name);
        Next::Go
    }

    /// `\r` throws the statement begun away.
    fn reset(splitter: &mut Splitter) -> Next {
        splitter.reset();
        Next::Go
    }

    /// `\a` makes output aligned if unaligned, else unaligned.
    fn aligned(&mut self) -> Next {
        self.layout.style = if self.layout.style == Style::Aligned { Style::Unaligned } else { Style::Aligned };
        let shown = self.layout.get("style").unwrap_or_default();
        self.say(&format!("Output is {shown}."))
    }

    /// `\t` sets or toggles the rows alone.
    fn tuples(&mut self, on: Option<bool>) -> Next {
        self.layout.tuples_only = on.unwrap_or(!self.layout.tuples_only);
        self.say(if self.layout.tuples_only { "Rows alone are on." } else { "Rows alone are off." })
    }

    /// `\notee`.
    fn no_tee(&mut self) -> Next {
        self.tee = None;
        Next::Go
    }

    /// Writes `text` to the output.
    pub(super) fn say(&mut self, text: &str) -> Next {
        if self.print_line(text).is_err() { Next::Stop } else { Next::Go }
    }

    /// `\set` lists the variables, or sets one.
    fn set(&mut self, assignment: Option<(String, String)>) -> Next {
        let Some((name, value)) = assignment else {
            let all: Vec<String> = self.vars.all().map(|(n, v)| format!("{n} = {}", crate::vars::literal(v))).collect();
            return self.say(&all.join("\n"));
        };
        if !crate::vars::is_name(&name) {
            return self.tell(&format!("\\set: {name:?} is no variable's name"), Next::Go);
        }
        self.vars.set(&name, &value);
        Next::Go
    }

    /// `\x` sets expanded output.
    fn expanded(&mut self, how: Expansion) -> Next {
        self.layout.expanded = match (how, self.layout.expanded) {
            (Expansion::Toggle, Expanded::Off) | (Expansion::Set(true), _) => Expanded::On,
            (Expansion::Toggle, Expanded::On | Expanded::Auto) | (Expansion::Set(false), _) => Expanded::Off,
            (Expansion::Auto, _) => Expanded::Auto,
        };
        let shown = self.layout.get("expanded").unwrap_or_default();
        self.say(&format!("Expanded output is {shown}."))
    }

    /// `\format` lists every option, shows one, or sets one.
    fn format(&mut self, option: Option<(String, Option<String>)>) -> Next {
        match option {
            None => {
                let all: Vec<String> = OPTIONS.iter().map(|o| format!("{o} {}", self.layout.get(o).unwrap_or_default())).collect();
                self.say(&all.join("\n"))
            }
            Some((name, None)) => match self.layout.get(&name) {
                Some(value) => self.say(&format!("{name} {value}")),
                None => self.tell(&format!("{name} is no option of \\format: {}", OPTIONS.join(", ")), Next::Go),
            },
            Some((name, Some(value))) => self.layout.set(&name, &value).map_or_else(|e| self.tell(&e, Next::Go), |()| Next::Go),
        }
    }
}

//! \[EXTENSION\] `\debug`'s words read (RULES.md D1036): what a debugger's
//! connection asks of the session it debugs, each a word after `\debug`
//! and its arguments.

use std::time::Duration;

use turborel_client::debug::{Action, Kind};

use super::seconds;

/// What `\debug` asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DebugCommand {
    /// `\debug id`: this connection's ID.
    Id,
    /// `\debug attach ID`.
    Attach(u64),
    /// `\debug detach`.
    Detach,
    /// `\debug break KIND NAME LINE [CONDITION]`.
    Break {
        /// What the name names.
        kind: Kind,
        /// The routine's or the trigger's name, as typed.
        name: String,
        /// Its line, from 1.
        line: u32,
        /// The `<search condition>`, the rest of the words, if any.
        condition: Option<String>,
    },
    /// `\debug clear NUMBER`.
    Clear(u32),
    /// `\debug breakpoints`.
    Breakpoints,
    /// `\debug step [into | over | out]` and `\debug continue`.
    Resume(Action),
    /// `\debug interrupt`.
    Interrupt,
    /// `\debug stack`.
    Stack,
    /// `\debug wait [SECONDS]`: the next event, waited for at most so
    /// long, without limit if not given.
    Wait(Option<Duration>),
}

/// The words of `\debug break` before its condition: `break`, the kind,
/// the name and the line.
const BEFORE_CONDITION: usize = 4;

/// What `\debug`'s words `args` ask, `rest` the line after `\debug` as
/// typed, of which a breakpoint's condition is taken as written.
///
/// # Errors
///
/// No word, one `\debug` does not know, or its arguments wrong.
pub fn debug(args: &[String], line: &str) -> Result<DebugCommand, String> {
    let (word, rest) = args
        .split_first()
        .ok_or("\\debug takes a word: id, attach, detach, break, clear, breakpoints, step, continue, interrupt, stack or wait")?;
    let none = |found: DebugCommand| if rest.is_empty() { Ok(found) } else { Err(format!("\\debug {word} takes no arguments")) };
    match word.to_ascii_lowercase().as_str() {
        "id" => none(DebugCommand::Id),
        "attach" => one_number(word, rest).map(DebugCommand::Attach),
        "detach" => none(DebugCommand::Detach),
        "break" => breakpoint(rest, condition(line)),
        "clear" => one_number(word, rest)
            .and_then(|n| u32::try_from(n).map_err(|_| format!("{n} is no breakpoint's number")))
            .map(DebugCommand::Clear),
        "breakpoints" => none(DebugCommand::Breakpoints),
        "step" => step(rest).map(DebugCommand::Resume),
        "continue" => none(DebugCommand::Resume(Action::Continue)),
        "interrupt" => none(DebugCommand::Interrupt),
        "stack" => none(DebugCommand::Stack),
        "wait" => match rest {
            [] => Ok(DebugCommand::Wait(None)),
            [within] => seconds(within).map(|s| DebugCommand::Wait(Some(s))),
            [_, _, ..] => Err("\\debug wait takes at most a number of seconds".to_owned()),
        },
        other => Err(format!("\\debug {other} is no word of \\debug: \\? lists them")),
    }
}

/// The one number `\debug word` takes.
fn one_number(word: &str, rest: &[String]) -> Result<u64, String> {
    match rest {
        [n] => n.parse().map_err(|_| format!("{n} is no number")),
        [] | [_, _, ..] => Err(format!("\\debug {word} takes one number")),
    }
}

/// How `\debug step` goes on: into, unless `over` or `out` is given.
fn step(rest: &[String]) -> Result<Action, String> {
    match rest {
        [] => Ok(Action::Into),
        [how] => match how.to_ascii_lowercase().as_str() {
            "into" => Ok(Action::Into),
            "over" => Ok(Action::Over),
            "out" => Ok(Action::Out),
            other => Err(format!("\\debug step takes into, over or out, not {other}")),
        },
        [_, _, ..] => Err("\\debug step takes into, over or out".to_owned()),
    }
}

/// The condition of `\debug break`, its line after `\debug` being
/// `line`: what follows its line's number, as written; none if nothing
/// does.
fn condition(line: &str) -> Option<String> {
    let mut left = line.trim_start();
    for _ in 0..BEFORE_CONDITION {
        left = left.split_once(char::is_whitespace).map_or("", |(_, after)| after.trim_start());
    }
    (!left.trim().is_empty()).then(|| left.trim().to_owned())
}

/// `\debug break`'s kind, name and line, then its `condition`.
fn breakpoint(rest: &[String], condition: Option<String>) -> Result<DebugCommand, String> {
    let [kind, name, line, ..] = rest else {
        return Err("\\debug break takes procedure, function or trigger, a name and a line".to_owned());
    };
    let kind = match kind.to_ascii_lowercase().as_str() {
        "procedure" => Kind::Procedure,
        "function" => Kind::Function,
        "trigger" => Kind::Trigger,
        other => return Err(format!("\\debug break takes procedure, function or trigger, not {other}")),
    };
    let line = line.parse().ok().filter(|l| *l > 0).ok_or_else(|| format!("{line} is no line"))?;
    Ok(DebugCommand::Break { kind, name: name.clone(), line, condition })
}

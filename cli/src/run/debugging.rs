//! \[EXTENSION\] `\debug` run (RULES.md D1036): each word one call of the
//! client's debugger (`turborel_client::debug`) on the current connection,
//! and what it gave written as lines. A pause is written as its call
//! stack, the innermost frame first, each with its routine, its line and
//! column, and its parameters, variables and cursors.

use std::io::{Read, Write};
use std::time::Duration;

use turborel::catalog::QualifiedName;
use turborel_client::Failure;
use turborel_client::connection::Connection;
use turborel_client::debug::{DebugBreakpoint, Event, Frame, Kind, Named, Paused, Reason};

use super::{Next, Shell, Statements};
use crate::command::DebugCommand;
use crate::format::cell;

/// The part `typed` of a name, as SQL reads an `<identifier>`: within
/// double quotes as written, each doubled quote one; else in upper case
/// [ISO/IEC 9075-2:2023 §5.2].
fn identifier(typed: &str) -> String {
    match typed.strip_prefix('"').and_then(|t| t.strip_suffix('"')) {
        Some(delimited) => delimited.replace("\"\"", "\""),
        None => typed.to_uppercase(),
    }
}

/// The name `typed`, as SQL reads a `<schema qualified name>`: its schema
/// before a period, if one is, each part read as an identifier is.
fn qualified(typed: &str) -> QualifiedName {
    match typed.split_once('.') {
        Some((schema, name)) => QualifiedName { auth: identifier(schema), ident: identifier(name) },
        None => QualifiedName { auth: String::new(), ident: identifier(typed) },
    }
}

/// What `asked` gives of `connection`, written as lines, null values as
/// `null`.
///
/// # Errors
///
/// The call's failure, as the server reports it.
pub(crate) fn debugged<S: Read + Write>(connection: &mut Connection<S>, asked: &DebugCommand, null: &str) -> Result<String, Failure> {
    Ok(match asked {
        DebugCommand::Id => format!("This is connection {}.", connection.debug_id()?),
        DebugCommand::Attach(id) => connection.debug_attach(*id).map(|()| format!("Attached to connection {id}."))?,
        DebugCommand::Detach => connection.debug_detach().map(|()| "Detached.".to_owned())?,
        DebugCommand::Break { kind, name, line, condition } => {
            let set = connection.debug_break((*kind, &qualified(name)), *line, condition.as_deref())?;
            format!("Breakpoint {}.", breakpoint(&set))
        }
        DebugCommand::Clear(number) => connection.debug_clear(*number).map(|()| format!("Breakpoint {number} cleared."))?,
        DebugCommand::Breakpoints => {
            let all = connection.debug_breakpoints()?;
            if all.is_empty() { "No breakpoints.".to_owned() } else { all.iter().map(breakpoint).collect::<Vec<_>>().join("\n") }
        }
        DebugCommand::Resume(how) => connection.debug_resume(*how).map(|()| "Going on.".to_owned())?,
        DebugCommand::Interrupt => connection.debug_interrupt().map(|()| "Pausing at the next statement.".to_owned())?,
        DebugCommand::Stack => connection.debug_stack()?.map_or_else(|| "Running.".to_owned(), |p| stack(&p, null)),
        DebugCommand::Wait(within) => connection.debug_wait(*within)?.map_or_else(|| nothing_within(*within), |e| event(&e, null)),
    })
}

/// That no event came within `within`.
fn nothing_within(within: Option<Duration>) -> String {
    within.map_or_else(|| "No event.".to_owned(), |w| format!("No event within {} seconds.", w.as_secs_f64()))
}

/// A breakpoint: its number, routine, line and condition.
fn breakpoint(b: &DebugBreakpoint) -> String {
    let line = format!("{} at {}.{} line {}", b.number, b.routine.auth, b.routine.ident, b.line);
    match &b.condition {
        Some(condition) => format!("{line} where {condition}"),
        None => line,
    }
}

/// An event, as lines.
fn event(e: &Event, null: &str) -> String {
    match e {
        Event::Paused(p) => stack(p, null),
        Event::TimedOut => "Timed out: the session went on by itself.".to_owned(),
        Event::Cancelled => "Cancelled: the session's statement was cancelled, and it went on.".to_owned(),
        Event::Ended => "The session ended: detached.".to_owned(),
    }
}

/// A pause, as lines: why, then each frame, the innermost first.
fn stack(p: &Paused, null: &str) -> String {
    let mut lines = vec![match p.reason {
        Reason::Breakpoint(number) => format!("Paused at breakpoint {number}."),
        Reason::Step => "Paused.".to_owned(),
    }];
    for (depth, f) in p.frames.iter().rev().enumerate() {
        lines.push(format!("#{} {}", depth.saturating_add(1), frame_head(f)));
        lines.extend(f.parameters.iter().chain(&f.variables).map(|n| format!("    {}", named(n, null))));
        lines.extend(f.cursors.iter().map(|c| format!("    CURSOR {} {}: {}", c.name, if c.open { "open" } else { "closed" }, c.query)));
    }
    lines.join("\n")
}

/// A frame's routine and place.
fn frame_head(f: &Frame) -> String {
    let Some(origin) = &f.origin else { return "a statement run directly".to_owned() };
    let kind = match origin.kind {
        Kind::Procedure => "PROCEDURE",
        Kind::Function => "FUNCTION",
        Kind::Trigger => "TRIGGER",
    };
    let place = f.line.map_or_else(String::new, |(line, column)| format!(" line {line}, column {column}"));
    let mut head = format!("{kind} {}.{}{place}", origin.shown.auth, origin.shown.ident);
    if !f.visible {
        head.push_str(" (not debugged)");
    }
    head
}

/// A parameter or a variable, its type and its value.
fn named(n: &Named, null: &str) -> String {
    format!("{} {} = {}", n.name, n.data_type, cell(&n.value, null))
}

impl<S: Statements + ?Sized, C: FnMut() -> Duration> Shell<'_, S, C> {
    /// `\debug` does what `asked` says, of the current connection.
    pub(super) fn debug(&mut self, asked: &DebugCommand) -> Next {
        let null = self.layout.null.clone();
        match self.connection.debug(asked, &null) {
            Ok(text) => self.say(&text),
            Err(failure) => self.tell(&format!("\\debug: {failure}"), Next::Go),
        }
    }
}

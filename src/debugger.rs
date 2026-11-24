//! \[EXTENSION\] The routine debugger (RULES.md D1003 to D1012): one
//! session, the debugger, attached to another, its target, stops the
//! target's SQL-invoked procedures, functions and triggered actions at
//! breakpoints, steps through their statements and reads their call stack,
//! each frame's parameters, variables and cursors with their values.
//!
//! What the two share is a `Link` (`link.rs`), hung on the target's `Stop`
//! (`txn/stop/debugged.rs`), the one handle another thread may hold of a
//! session. A program the target runs looks at the link's flag as each of
//! its statements begins (`exec/program/debug.rs`): with no debugger
//! attached that is one load and one branch. Attached, each run of a
//! program is a frame on the link's stack (`tracked.rs`), and a statement
//! that a breakpoint or a step stops at pauses its session: the frames are
//! taken as they are, the debugger told, and the session waits for the
//! debugger's word, its cancellation, or DEBUGGER.PAUSE_TIMEOUT_MS, after
//! which it goes on by itself, so no lock it holds is held for ever.
//!
//! The session's side, attaching, breakpoints, steps and waiting for an
//! event, is `db/debugging.rs`.

mod link;
mod tracked;

pub use link::Link;
pub(crate) use link::refused;
pub(crate) use tracked::{Tracked, may_debug};

use crate::catalog::QualifiedName;
use crate::value::Value;

/// \[DESIGN\] How long a paused session sleeps between looks at its
/// debugger's word, its cancellation and its timeout, as a lock wait looks
/// at its cancellation (`txn/locks.rs`, `STOP_POLL`).
pub(crate) const PAUSE_POLL: std::time::Duration = std::time::Duration::from_millis(20);

/// What kind of routine a frame runs the body of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// An SQL-invoked procedure.
    Procedure,
    /// An SQL-invoked function.
    Function,
    /// A trigger's triggered action.
    Trigger,
}

/// \[DESIGN\] The routine or trigger a program is the body of (RULES.md
/// D1004).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Origin {
    /// Its kind.
    pub kind: Kind,
    /// Of a routine its specific name, of a trigger its name: what a
    /// breakpoint names it by.
    pub name: QualifiedName,
    /// Of a routine its name, of a trigger its name again.
    pub shown: QualifiedName,
}

/// What a paused session is told to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Pause at the next statement, in whatever frame it is.
    Into,
    /// Pause at the next statement of this frame or one it returns to.
    Over,
    /// Pause at the next statement of the frame this one returns to.
    Out,
    /// Run until a breakpoint.
    Continue,
}

/// A parameter or variable of a frame, and its value.
#[derive(Debug, Clone)]
pub struct Named {
    /// Its name.
    pub name: String,
    /// Its declared type, as SQL writes it.
    pub data_type: String,
    /// Its value.
    pub value: Value,
}

/// A local cursor of a frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorShown {
    /// Its `<cursor name>`.
    pub name: String,
    /// Whether it is open.
    pub open: bool,
    /// Its `<cursor specification>`, as written.
    pub query: String,
}

/// A frame of the call stack: a run of a routine's or a trigger's body.
#[derive(Debug, Clone, Default)]
pub struct Frame {
    /// What it is the body of; none of a control statement run directly.
    pub origin: Option<Origin>,
    /// Whether the debugger may see into it: owns the routine or holds
    /// DEBUG on it. A frame it may not shows its origin alone, and never
    /// pauses (RULES.md D1008).
    pub visible: bool,
    /// Where the statement it runs now begins, in characters from the
    /// beginning of its routine's or trigger's text.
    pub at: Option<usize>,
    /// That place's line, from 1, and column, from 1, in the text of the
    /// routine or trigger as its definition holds it.
    pub line: Option<(u32, u32)>,
    /// Its parameters, or a trigger's transition variables, in order.
    pub parameters: Vec<Named>,
    /// The SQL variables in scope where it is, the outermost first.
    pub variables: Vec<Named>,
    /// The local cursors in scope where it is.
    pub cursors: Vec<CursorShown>,
}

/// Why a session paused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// At the breakpoint of this number.
    Breakpoint(u32),
    /// At the end of a step, or as the debugger asked it to pause.
    Step,
}

/// A session paused: why, and its frames, the outermost first.
#[derive(Debug, Clone)]
pub struct Paused {
    /// Why it paused.
    pub reason: Reason,
    /// Its call stack, the outermost first, the paused statement's frame
    /// last.
    pub frames: Vec<Frame>,
}

/// What the debugger hears of its target.
#[derive(Debug, Clone)]
pub enum Event {
    /// The target paused.
    Paused(Paused),
    /// The target, paused, went on by itself past
    /// DEBUGGER.PAUSE_TIMEOUT_MS.
    TimedOut,
    /// The target, paused, went on as its statement was cancelled.
    Cancelled,
    /// The target session ended: the debugger is detached.
    Ended,
}

/// Where the character `at` of `text` is: its line, from 1, and its
/// column, from 1.
pub(crate) fn line_of(text: &str, at: usize) -> (u32, u32) {
    let (mut line, mut column) = (1_u32, 1_u32);
    for c in text.chars().take(at) {
        (line, column) = if c == '\n' { (line.saturating_add(1), 1) } else { (line, column.saturating_add(1)) };
    }
    (line, column)
}

/// A breakpoint: in a routine's or a trigger's body, at the statements
/// that begin on a line of its text, if its condition, given, is true.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Breakpoint {
    /// Its number, from 1, which the debugger clears it by.
    pub number: u32,
    /// The specific name of the routine, or the name of the trigger.
    pub routine: QualifiedName,
    /// Its line, from 1.
    pub line: u32,
    /// The characters of the line, from where it begins to where the next
    /// begins.
    pub(crate) span: (usize, usize),
    /// A `<search condition>` over the frame's parameters and variables,
    /// if given: it pauses only where that is true (RULES.md D1009).
    pub condition: Option<String>,
}

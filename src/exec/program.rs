//! [ISO/IEC 9075-4:2006 draft Clause 14, §17.2, §17.3] A program run:
//! the steps of a bound SQL control statement (`logical/program.rs`)
//! executed over its frame, in a host (RULES.md D76).
//!
//! A host is where the program runs. The session's runs a procedure a CALL
//! invokes, a triggered action, a function that may change data and a
//! control statement executed directly: its statements are the session's,
//! its local cursors the session's cursors, its diagnostics area the
//! session's, and each statement that changes data is undone alone if it
//! fails (`db/routines/program.rs`). The executor's runs a function a query
//! evaluates, which reads SQL-data and changes none (`program/executor.rs`).
//!
//! Each statement is run in its turn, and a condition it raises, an
//! exception or a completion condition other than successful completion,
//! is offered to the handlers in scope where it was raised, the innermost
//! compound statement's first and of each its specific handler before its
//! general one [§14.2 GR1, SR8] (`program/conditions.rs`). A CONTINUE
//! handler's action runs and the statement after the one that raised it
//! follows; an EXIT handler's runs and its compound statement is left; an
//! UNDO handler's runs once that compound statement's changes are undone
//! [§14.2 GR3]. A condition no handler handles ends the program, if it is
//! an exception; a completion condition is the program's completion, and
//! the statement after the one that raised it follows.
//!
//! A CALL bound with the program, in a function's body, runs the
//! procedure's program in the same host, its parameters a frame of their
//! own [ISO/IEC 9075-2:2023 §9.18 GR4c, GR10b].

mod called;
mod conditions;
mod debug;
mod framed;
mod leaves;
mod run;

pub(crate) use conditions::{appropriate_among, completed, completion, item_text, raised, unhandled};
pub(crate) use debug::{pause_most, transition_named};
pub(crate) use framed::Framed;
pub(crate) use leaves::{assign_target, rows};
pub(crate) use run::{Ended, run_program};

use crate::ast::FetchOrientation;
use crate::db::Completion;
use crate::error::{Result, SqlError};
use crate::logical::{Contained, Cursor, Leaf, Step};
use crate::params::Params;
use crate::storage::Row;

/// \[DESIGN\] Where a program runs: what its statements, cursors, marks and
/// diagnostics area are.
pub(crate) trait Host {
    /// A place to undo to: of a host whose statements change nothing, none.
    type Mark;

    /// The rows the leaf `leaf` gives over `frame`, at most `limit` if
    /// given, as `leaves::rows` runs it with what reads its tables.
    ///
    /// # Errors
    ///
    /// The leaf's, or a lock's.
    fn rows(&self, leaf: &Leaf, frame: &Params, limit: Option<usize>) -> Result<Vec<Row>>;

    /// `row`, of a query, as the session shows its datetimes.
    fn present(&self, row: &mut Row);

    /// \[DESIGN\] [ISO/IEC 9075-4:2006 draft §17.3 GR6a iii] The completion
    /// condition `e` raised, a SIGNAL of class 01 or 02 no handler handled,
    /// for the session's client to be told of (RULES.md D187). Default: a
    /// host of no session tells no one.
    fn noted(&self, _e: &SqlError) {}

    /// Whether the program's statement has been cancelled, or has run past
    /// its time: checked as each iterated statement repeats (RULES.md D44).
    ///
    /// # Errors
    ///
    /// The cancellation's.
    fn interrupted(&self) -> Result<()>;

    /// Runs the statement `c` with the frame `frame`, its variables those
    /// in scope where it is: its completion.
    ///
    /// # Errors
    ///
    /// The statement's.
    fn run(&mut self, c: &Contained, frame: &mut Params) -> Result<Completion>;

    /// [§14.1 SR15] A number no program running in the host has as its
    /// execution, which names its local cursors apart (`params::local_key`).
    fn execution(&mut self) -> usize;

    /// [§12.1] Declares the local cursor `cursor` as `key`, its name in this
    /// execution, as its compound statement or FOR statement begins.
    ///
    /// # Errors
    ///
    /// Its declaration's.
    fn declare(&mut self, key: &str, cursor: &Cursor) -> Result<()>;

    /// [§12.2] Opens the cursor `key`, its query of the frame `frame`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 24000 if it is open; its query's.
    fn open(&mut self, key: &str, cursor: &Cursor, frame: &mut Params) -> Result<()>;

    /// [§12.3] The row the cursor `key` fetches in `orientation`; none past
    /// its rows.
    ///
    /// # Errors
    ///
    /// SQLSTATE 24000 if it is not open; the position's.
    fn fetch(&mut self, key: &str, orientation: &FetchOrientation, frame: &Params) -> Result<Option<Row>>;

    /// [§12.4] Closes the cursor `key`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 24000 if it is not open.
    fn close(&mut self, key: &str) -> Result<()>;

    /// [§14.1 GR4, GR5] The cursor `key` closed if it is open, and
    /// destroyed, as its compound statement ends.
    fn destroy(&mut self, key: &str);

    /// A mark here, of a host whose statements change data.
    ///
    /// # Errors
    ///
    /// The transaction's.
    fn mark(&mut self) -> Result<Option<Self::Mark>>;

    /// Every change since `mark` undone, as `failure` asks.
    ///
    /// # Errors
    ///
    /// SQLCODE -914 if a change could not be undone.
    fn undo(&mut self, mark: Self::Mark, failure: &SqlError) -> Result<()>;

    /// `mark` released: what it marked stays.
    fn release(&mut self, mark: Self::Mark);

    /// [ISO/IEC 9075-4:2006 draft §14.1 GR1; ISO/IEC 9075-2:2023 §9.18
    /// GR7] A new savepoint level established: of an ATOMIC compound
    /// statement if `atomic`, else of a procedure of NEW SAVEPOINT LEVEL
    /// the program calls.
    fn open_level(&mut self, atomic: bool);

    /// [§14.1 GR6; §9.18 GR15] The current savepoint level destroyed, with
    /// its savepoints.
    fn close_level(&mut self);

    /// The first diagnostics area emptied, as the statement `step` begins.
    fn begin(&mut self, step: &Step);

    /// The condition `step` completed with placed in the first diagnostics
    /// area.
    fn end(&mut self, step: &Step, outcome: &Result<Completion>);

    /// [ISO/IEC 9075-2:2023 §23.2] The diagnostics area stack pushed.
    fn push_area(&mut self);

    /// [ISO/IEC 9075-2:2023 §23.2] The diagnostics area stack popped.
    fn pop_area(&mut self);

    /// \[EXTENSION\] The link of the debugger attached to the session the
    /// program runs in, and the state it runs in, if one is (RULES.md
    /// D1005). Default: a host of no session has none.
    fn debugged(&self) -> Option<(std::sync::Arc<crate::debugger::Link>, std::sync::Arc<crate::db::State>)> {
        None
    }

    /// \[EXTENSION\] Whether the local cursor `key` is open, as a debugger
    /// shows it. Default: not.
    fn cursor_open(&self, _key: &str) -> bool {
        false
    }
}

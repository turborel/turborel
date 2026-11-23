//! \[DESIGN\] A procedure's body compiled (RULES.md D841).
//!
//! As a CALL first runs a procedure whose body its session keeps (RULES.md
//! D406), the body's program is compiled to a tree of code (`code.rs`),
//! kept with the program, so it is made again whenever the program is:
//! once any definition, privilege, role, schema, time zone or setting the
//! program was bound with no longer holds (D45, D414). The tree runs
//! without the interpreter (`run.rs`, `flow.rs`), its values found as it
//! was compiled: a row of values that reads nothing is evaluated where it
//! is over the frame's slots, a variable or parameter assigned as a whole
//! is its slot (`valued.rs`, `evaluated.rs`); a SELECT ... INTO of one
//! point read of a whole unique key reads its row by that point read;
//! and a change by a whole unique key, or an INSERT of one row, runs
//! straight on its table once its kept plans show it may (`change.rs`).
//!
//! Every other statement runs as the interpreter runs it, through the
//! session as its host, from inside the compiled program: so any body
//! runs, its semantics the interpreter's. EXECUTOR.COMPILED_PROCEDURES OFF
//! runs each body by the interpreter instead.

mod append;
mod change;
mod code;
mod evaluated;
mod flow;
mod run;
mod valued;

pub(in crate::db) use change::compiled_procedures;

use super::super::super::{Session, StatementContext};
use super::super::kept::KeptProgram;
use crate::error::Result;
use crate::exec::{Ended, Host, unhandled};
use crate::logical::{Program, Step};
use crate::params::Params;
use code::Node;
use run::{Exec, Flow, Stop};

/// A procedure's body compiled.
#[derive(Debug)]
pub(in crate::db) struct Compiled {
    body: Node,
    /// Whether nothing of it reads the diagnostics area as it runs
    /// (`run.rs`, `unread`, RULES.md D943).
    unread: bool,
}

impl Compiled {
    /// `program` compiled; none if it holds a CALL bound with it, which a
    /// function's body alone holds.
    pub(in crate::db) fn of(program: &Program) -> Option<Compiled> {
        let mut work = vec![&program.body];
        while let Some(step) = work.pop() {
            if let Step::Call(_) = step {
                return None;
            }
            work.extend(step.steps());
        }
        let body = code::compiled(program);
        let unread = run::unread(std::slice::from_ref(&body));
        Some(Compiled { body, unread })
    }
}

/// [ISO/IEC 9075-4:2006 draft Clause 14] The compiled program of `kept`
/// run by `session` in `cx` over `frame`, as `run_program` runs a program.
///
/// # Errors
///
/// An exception no handler handles.
pub(in crate::db) fn run_compiled(
    (session, cx): (&Session, StatementContext<'_>),
    (kept, compiled): (&KeptProgram, &Compiled),
    frame: &mut Params,
) -> Result<Ended> {
    let mut exec = Exec::new(session, cx, kept, frame);
    exec.unread = compiled.unread;
    let execution = exec.host.execution();
    exec.frame.set_execution(execution);
    let ran = exec.list(std::slice::from_ref(&compiled.body));
    // What its last statement's condition would take as it is placed.
    if compiled.unread {
        session.unheard();
    }
    match ran {
        Ok((Flow::Return(v), _)) => Ok(Ended::Returned(v)),
        Ok((Flow::Next | Flow::Leave(_) | Flow::Iterate(_), completion)) => Ok(Ended::Completed(completion)),
        Err(Stop::Raised(e) | Stop::Unhandled(e)) => Err(unhandled(e)),
    }
}

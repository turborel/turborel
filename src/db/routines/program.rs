//! [ISO/IEC 9075-4:2006 draft Clause 14] A program the session runs: the
//! body of a procedure a CALL invokes, a triggered action, or an SQL
//! control statement executed directly (RULES.md D76).
//!
//! The program is bound as it runs, as a procedure's body is read at each
//! CALL, over a frame of the values of the statement's parameters: the
//! procedure's SQL parameters, or the action's transition variables, then
//! its variables. Once it has run, the parameters' values go back, so the
//! CALL reads its output parameters and the BEFORE trigger its new row.
//!
//! The session is its host (`exec/program.rs`). Each statement it runs is
//! the session's, in the scope of the variables and cursors where it is,
//! and one that may change data is marked as it begins, so it is undone
//! alone if it fails [ISO/IEC 9075-2:2023 §4.41.5]. Its local cursors are
//! the session's cursors, named so no `<cursor name>` can name them
//! (`binder/psm.rs`, `cursor_key`), so a positioned UPDATE or DELETE reads them as
//! any other. Its diagnostics area is the session's.
//!
//! A function whose body is an SQL control statement runs where a query
//! evaluates it, its host the query's (`program/executor.rs`).

mod checked;
mod cursors;
mod executor;
mod host;
mod hosted;

pub(crate) use executor::run_unhosted;
pub(in crate::db) use hosted::{Hosted, hosted_rows};

use host::SessionHost;

use std::sync::Arc;

use super::super::area::commands::{self, Command};
use super::super::execute::command_of;
use super::super::{Completion, Session, StatementContext};
use crate::ast::{ControlStatement, SqlStatement};
use crate::binder::{Frame, bind_program};
use crate::catalog::{DataAccess, WHOLE};
use crate::error::Result;
use crate::exec::{Ended, run_program};
use crate::logical::{Program, Step};
use crate::params::{Params, RoutineScope};
use crate::value::Value;

/// [ISO/IEC 9075-2:2023 §4.41.5; ISO/IEC 9075-4:2006 draft §4.10.4]
/// Whether `stmt` is atomic: undone whole if it fails. CALL, RETURN, a
/// compound statement not ATOMIC, and the assignment, CASE, FOR, IF, LOOP,
/// REPEAT and WHILE statements are not.
pub(in crate::db) fn atomic(stmt: &SqlStatement) -> bool {
    let SqlStatement::Control(c) = stmt else { return !matches!(stmt, SqlStatement::Call(_) | SqlStatement::Return(_)) };
    match &**c {
        ControlStatement::Compound(c) => c.atomic,
        ControlStatement::Iterate(_) | ControlStatement::Leave(_) => true,
        ControlStatement::Assignment(_)
        | ControlStatement::Case(_)
        | ControlStatement::If(_)
        | ControlStatement::Iterated(_)
        | ControlStatement::For(_) => false,
    }
}

/// [ISO/IEC 9075-2:2023 §11.60 SR23c, SR23d] What `stmt` itself possibly
/// does with SQL-data, each statement within it counted, of the routines
/// it invokes and the tables it names aside.
pub(super) fn written(stmt: &SqlStatement) -> DataAccess {
    let mut access = DataAccess::Contains;
    let mut work = vec![stmt];
    while let Some(s) = work.pop() {
        if let SqlStatement::Control(c) = s {
            work.extend(c.statements());
            continue;
        }
        let a = super::body::written(s);
        if super::body::rank(a) > super::body::rank(access) {
            access = a;
        }
    }
    access
}

impl Session {
    /// [ISO/IEC 9075-4:2006 draft Clause 14] The SQL control statement, or
    /// SIGNAL or RESIGNAL, `stmt` in `cx`, of the procedure, triggered
    /// action or session `params` are of: bound and run, the parameters'
    /// values as it left them. Run directly, of the session's own, it runs
    /// in a context of its own, as it may end its transaction [§4.10.4].
    ///
    /// # Errors
    ///
    /// Its binding's; an exception no handler of it handles.
    pub(in crate::db) fn control_statement(
        &self,
        cx: StatementContext<'_>,
        stmt: &SqlStatement,
        params: &mut Params,
    ) -> Result<Completion> {
        let base = params.dynamic_values().len();
        let scope = params.routine_scope().map_or_else(|| RoutineScope::unnamed(WHOLE.clone()), RoutineScope::clone);
        let transitions = params.transitions_shared();
        let direct = params.routine_scope().is_none() && transitions.is_none();
        let frame = Frame { base, scope: Arc::new(scope), transitions: transitions.clone(), returns: None, inline: false };
        let shared = Arc::clone(&frame.scope);
        let program = self.bound(cx.st, params, |b| bind_program(b, stmt, frame))?;
        let mut values = params.dynamic_values().to_vec();
        values.resize(base.saturating_add(program.variables.len()), Value::Null);
        let mut inner = Params::of_routine(values, shared);
        if let Some(t) = transitions {
            inner = inner.with_transitions(t);
        }
        let ended = if direct {
            run_program(&program, &mut inner, &mut SessionHost::direct(self, cx))?
        } else {
            self.run_hosted(cx, &program, &mut inner)?
        };
        for (k, v) in inner.dynamic_values().iter().take(base).enumerate() {
            params.set_slot(k, v.clone())?;
        }
        match ended {
            Ended::Completed(completion) => Ok(completion),
            Ended::Returned(_) => Ok(Completion::Success),
        }
    }

    /// The body `body` of a procedure run in `cx` with `params`: a statement
    /// that is atomic undone alone if it fails, as CALL is not atomic
    /// [ISO/IEC 9075-2:2023 §4.41.5].
    ///
    /// # Errors
    ///
    /// The body's.
    pub(in crate::db) fn run_body(&self, cx: StatementContext<'_>, body: &SqlStatement, params: &mut Params) -> Result<Completion> {
        if !atomic(body) {
            return self.run_statement(cx, body, params, None);
        }
        let mark = cx.tx.mark()?;
        match self.run_statement(cx, body, params, None) {
            Ok(done) => {
                cx.tx.release_mark(mark);
                Ok(done)
            }
            Err(e) => Err(cx.st.undo_to(cx.tx, mark, &e).err().unwrap_or(e)),
        }
    }

    /// `program` run over `frame` in `cx`, the session its host.
    ///
    /// # Errors
    ///
    /// An exception no handler of it handles.
    pub(in crate::db) fn run_hosted(&self, cx: StatementContext<'_>, program: &Program, frame: &mut Params) -> Result<Ended> {
        run_program(program, frame, &mut SessionHost::within(self, cx))
    }
}

/// [ISO/IEC 9075-4:2006 draft §17.1 Table 2] How the diagnostics area names
/// the statement `step`.
pub(super) fn step_command(step: &Step) -> Command {
    match step {
        Step::Block(_) => commands::BEGIN_END,
        Step::Assign(_) => commands::ASSIGNMENT,
        Step::If(_) => commands::IF,
        Step::Case(_) => commands::CASE,
        Step::Loop(_) => commands::LOOP,
        Step::For(_) => commands::FOR,
        Step::Leave(_) => commands::LEAVE,
        Step::Iterate(_) => commands::ITERATE,
        Step::Return(_) => commands::RETURN,
        Step::Signal(s) => {
            if s.resignal {
                commands::RESIGNAL
            } else {
                commands::SIGNAL
            }
        }
        Step::Into(_) => commands::SELECT_ROW,
        Step::Cursor(c) => match c.action {
            crate::logical::CursorAction::Open => commands::OPEN,
            crate::logical::CursorAction::Fetch(..) => commands::FETCH,
            crate::logical::CursorAction::Close => commands::CLOSE_CURSOR,
        },
        Step::Statement(c) => command_of(&c.statement),
        Step::Call(_) => commands::CALL,
    }
}

/// Whether `step` is GET DIAGNOSTICS, which leaves the area as it was.
pub(super) fn diagnostics(step: &Step) -> bool {
    matches!(step, Step::Statement(c) if matches!(c.statement, SqlStatement::GetDiagnostics(_)))
}

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
mod compiled;
mod cursors;
mod executor;
mod host;
mod hosted;

pub(in crate::db) use compiled::{Compiled, compiled_procedures, runs_straight};
pub(crate) use executor::{run_component, run_unhosted};
pub(in crate::db) use hosted::{Hosted, evaluates, hosted_rows};

use host::SessionHost;

use super::kept::{KeptProgram, KeptRoutine};

use std::sync::Arc;

use super::super::area::commands::{self, Command};
use super::super::execute::command_of;
use super::super::{Completion, Session, State, StatementContext};
use crate::ast::{ControlStatement, SqlStatement};
use crate::binder::{Frame, bind_program};
use crate::catalog::{DataAccess, WHOLE};
use crate::error::{Code, Result, SqlError};
use crate::exec::{Ended, run_program};
use crate::logical::{Program, Step};
use crate::params::{Params, RoutineScope};
use crate::value::Value;

/// [ISO/IEC 9075-2:2023 §4.41.5; ISO/IEC 9075-4:2006 draft §4.10.4]
/// Whether `stmt` is atomic: undone whole if it fails. CALL, RETURN, a
/// savepoint's statement [ISO/IEC 9075-2:2023 §4.41.5], a compound
/// statement not ATOMIC, and the assignment, CASE, FOR, IF, LOOP,
/// REPEAT and WHILE statements are not.
pub(in crate::db) fn atomic(stmt: &SqlStatement) -> bool {
    let SqlStatement::Control(c) = stmt else {
        return !matches!(stmt, SqlStatement::Call(_) | SqlStatement::Return(_) | SqlStatement::Savepoint(_));
    };
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
        self.control_kept(cx, stmt, params, None)
    }

    /// `control_statement`, of the body of the procedure `routine` keeps,
    /// if given: its program as it keeps it (RULES.md D406).
    fn control_kept(
        &self,
        cx: StatementContext<'_>,
        stmt: &SqlStatement,
        params: &mut Params,
        routine: Option<&KeptRoutine>,
    ) -> Result<Completion> {
        let base = params.dynamic_values().len();
        let transitions = params.transitions_shared();
        let direct = params.routine_scope().is_none() && transitions.is_none();
        let kept = self.program_of(cx, stmt, params, routine)?;
        let program = &kept.program;
        let mut values = params.dynamic_values().to_vec();
        values.resize(base.saturating_add(program.variables.len()), Value::Null);
        let mut inner = Params::of_routine(values, Arc::clone(&program.scope));
        if let Some(t) = transitions {
            inner = inner.with_transitions(t);
        }
        let ended = if direct {
            run_program(program, &mut inner, &mut SessionHost::direct(self, cx))?
        } else if let Some(compiled) = routine.and_then(|_| compiled_procedures(cx.st, self).then(|| kept.compiled()).flatten()) {
            // [DESIGN] RULES.md D841: a procedure's body kept runs compiled.
            compiled::run_compiled((self, cx), (&kept, compiled), &mut inner)?
        } else {
            let kept = routine.is_some().then_some(&kept);
            run_program(program, &mut inner, &mut SessionHost::within(self, cx, kept))?
        };
        for (k, v) in inner.dynamic_values().iter().take(base).enumerate() {
            params.set_slot(k, v.clone())?;
        }
        match ended {
            Ended::Completed(completion) => Ok(completion),
            Ended::Returned(_) => Ok(Completion::Success),
        }
    }

    /// The program of `stmt`, bound over the frame of `params`: as
    /// `routine` keeps it, if given (`kept.rs`).
    ///
    /// # Errors
    ///
    /// Its binding's.
    fn program_of(
        &self,
        cx: StatementContext<'_>,
        stmt: &SqlStatement,
        params: &Params,
        routine: Option<&KeptRoutine>,
    ) -> Result<KeptProgram> {
        // Its frame is made only as it is bound, so a program kept copies
        // no scope (RULES.md D940).
        let frame = || {
            let scope = params.routine_scope().map_or_else(|| RoutineScope::unnamed(WHOLE.clone()), RoutineScope::clone);
            let (base, transitions) = (params.dynamic_values().len(), params.transitions_shared());
            let origin = routine.map(|r| r.origin.clone()).or_else(|| transitions.as_ref().and_then(|_| self.trigger_executing()));
            Frame { base, scope: Arc::new(scope), transitions, returns: None, inline: false, origin }
        };
        let Some(routine) = routine else { return Ok(KeptProgram::of(self.bound(cx.st, params, |b| bind_program(b, stmt, frame()))?)) };
        routine.program((cx.st, self), || {
            // Bound as a statement kept is, so a binding that reads the
            // statement's instant is noted, and not kept.
            let binding = params.keeping();
            let program = self.bound(cx.st, &binding, |b| bind_program(b, stmt, frame()))?;
            Ok((program, binding.held()))
        })
    }

    /// The body `body` of the procedure `routine` keeps, run in `cx` with
    /// `params`: a statement that is atomic undone alone if it fails, as
    /// CALL is not atomic [ISO/IEC 9075-2:2023 §4.41.5]. A control
    /// statement runs as the program `routine` keeps.
    ///
    /// # Errors
    ///
    /// The body's.
    pub(in crate::db) fn run_body(
        &self,
        cx: StatementContext<'_>,
        body: &SqlStatement,
        params: &mut Params,
        routine: &KeptRoutine,
    ) -> Result<Completion> {
        marked(cx, atomic(body), || {
            if matches!(body, SqlStatement::Control(_) | SqlStatement::Signal(_)) {
                let _compiling = self.compiling(cx.st, cx.tx.id);
                return self.control_kept(cx, body, params, Some(routine));
            }
            self.run_statement(cx, body, params, None)
        })
    }

    /// \[DESIGN\] The body of a procedure, a compound statement, ATOMIC if
    /// `atomic`, whose program `kept` is compiled, run in `cx` over
    /// `frame`, the whole frame of the program with its parameters' values
    /// first. It runs as `run_body` runs it, with no frame made of
    /// another's values and none copied back (RULES.md D940).
    ///
    /// # Errors
    ///
    /// The body's; SQLCODE -910 if `kept` is not compiled.
    pub(in crate::db) fn run_framed(&self, cx: StatementContext<'_>, kept: &KeptProgram, frame: &mut Params, atomic: bool) -> Result<()> {
        let Some(compiled) = kept.compiled() else {
            return Err(SqlError::new(Code::Api, "[DESIGN] a program run over its frame is compiled"));
        };
        marked(cx, atomic, || {
            let _compiling = self.compiling(cx.st, cx.tx.id);
            compiled::run_compiled((self, cx), (kept, compiled), frame).map(|_| Completion::Success)
        })
        .map(drop)
    }

    /// The warnings heard emptied, as a statement that begins empties them
    /// with the diagnostics area (`area.rs`, `emptied`), the area left as
    /// it is.
    pub(in crate::db) fn unheard(&self) {
        let warnings = self.stop.warnings();
        warnings.take();
        warnings.take_left_out();
    }

    /// Whether the session runs `kept`, a procedure's body, compiled in
    /// `st` (RULES.md D841).
    pub(in crate::db) fn runs_compiled(&self, st: &State, kept: &KeptProgram) -> bool {
        compiled_procedures(st, self) && kept.compiled().is_some()
    }

    /// `program` run over `frame` in `cx`, the session its host.
    ///
    /// # Errors
    ///
    /// An exception no handler of it handles.
    pub(in crate::db) fn run_hosted(&self, cx: StatementContext<'_>, program: &Program, frame: &mut Params) -> Result<Ended> {
        run_program(program, frame, &mut SessionHost::within(self, cx, None))
    }
}

/// [ISO/IEC 9075-2:2023 §4.41.5] What `run` gives in `cx`: undone alone
/// if it fails, if it is `atomic`.
fn marked(cx: StatementContext<'_>, atomic: bool, run: impl FnOnce() -> Result<Completion>) -> Result<Completion> {
    if !atomic {
        return run();
    }
    let mark = cx.tx.mark()?;
    match run() {
        Ok(done) => {
            cx.tx.release_mark(mark);
            Ok(done)
        }
        Err(e) => Err(cx.st.undo_to(cx.tx, mark, &e).err().unwrap_or(e)),
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
        Step::Statement(c) => command_of(c.statement()),
        Step::Call(_) => commands::CALL,
    }
}

/// Whether `step` is GET DIAGNOSTICS, which leaves the area as it was.
pub(super) fn diagnostics(step: &Step) -> bool {
    matches!(step, Step::Statement(c) if matches!(c.statement(), SqlStatement::GetDiagnostics(_)))
}

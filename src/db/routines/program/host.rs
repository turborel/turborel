//! \[DESIGN\] The session as the host of a program it runs
//! (`exec/program.rs`).
//!
//! The program of a procedure, a triggered action or a function runs in
//! the state and transaction of the statement that runs it. An SQL
//! control statement NOT ATOMIC run directly holds its own instead, as it
//! may end its transaction by COMMIT or ROLLBACK [ISO/IEC 9075-4:2006
//! draft §4.10.4; ISO/IEC 9075-2:2023 §17.7, §17.8]: the statements after
//! that one run in a new transaction, begun as the first of them needs
//! one. Its state is taken anew after a transaction ends and after a
//! statement that changes a definition, so a statement sees what the
//! statements before it defined and committed.

use std::cell::RefCell;
use std::sync::Arc;

use super::super::super::area::commands;
use super::super::super::savepoints::{Level, none_named};
use super::super::super::{Completion, Session, State, StatementContext};
use super::super::kept::{KeptProgram, KeptRuns};
use super::{Hosted, diagnostics, step_command};
use crate::ast::{FetchOrientation, SavepointStatement, SqlStatement, TableReference};
use crate::error::{Result, SqlError};
use crate::exec::{Host, rows};
use crate::logical::{Contained, Cursor, Leaf, Step};
use crate::params::Params;
use crate::storage::Row;
use crate::txn::{Mark, Txn};
use crate::value::datetime::show_in;

/// Where the statements of a program run.
enum Context<'c> {
    /// In the statement that runs it: a CALL, a trigger's, or one that
    /// invokes a function.
    Within(StatementContext<'c>),
    /// Of an SQL control statement run directly: the state as it is now,
    /// and the transaction, while one is active.
    Own { st: Arc<State>, tx: RefCell<Option<Arc<Txn>>> },
}

/// \[DESIGN\] The session as a program's host.
pub(super) struct SessionHost<'s, 'c> {
    session: &'s Session,
    context: Context<'c>,
    /// Of a procedure's body kept, the plans its statements keep
    /// (`routines/kept.rs`).
    kept: Option<&'s KeptProgram>,
}

impl<'s, 'c> SessionHost<'s, 'c> {
    /// The host of a program `session` runs within the statement in `cx`:
    /// of a procedure's body, the program `kept` keeps, if given.
    pub(super) fn within(session: &'s Session, cx: StatementContext<'c>, kept: Option<&'s KeptProgram>) -> SessionHost<'s, 'c> {
        SessionHost { session, context: Context::Within(cx), kept }
    }

    /// The host of an SQL control statement `session` runs directly,
    /// beginning in `cx`.
    pub(super) fn direct(session: &'s Session, cx: StatementContext<'_>) -> SessionHost<'s, 'c> {
        let tx = RefCell::new(session.current_transaction());
        SessionHost { session, context: Context::Own { st: Arc::clone(cx.st), tx }, kept: None }
    }

    /// `f` run in the context the program's statements run in now: in a
    /// transaction begun, if the last one ended.
    fn with<T>(&self, f: impl FnOnce(StatementContext<'_>) -> Result<T>) -> Result<T> {
        match &self.context {
            Context::Within(cx) => f(*cx),
            Context::Own { st, tx } => {
                let open = tx.borrow().clone();
                let now = if let Some(now) = open {
                    now
                } else {
                    let begun = self.session.begun(st)?;
                    *tx.borrow_mut() = Some(Arc::clone(&begun));
                    begun
                };
                f(StatementContext { st, tx: &now })
            }
        }
    }

    /// \[DESIGN\] RULES.md D454: reading at a snapshot for each statement,
    /// a statement of a procedure's body, or of a control statement run
    /// directly, reads at one taken as it begins, as a statement the
    /// session runs does, unless a cursor is open. A function's or a
    /// triggered action's reads at the snapshot of the statement that runs
    /// it, which is still reading.
    fn fresh(&self, tx: &Txn) -> Result<()> {
        if self.session.nesting.savepoints.borrow().in_statement() {
            return Ok(());
        }
        tx.renew_snapshot(|| !self.session.cursors_open())
    }

    /// Its state taken anew, of an SQL control statement run directly.
    fn refreshed(&mut self) -> Result<()> {
        if let Context::Own { st, .. } = &mut self.context {
            *st = self.session.current_state()?;
        }
        Ok(())
    }

    /// [ISO/IEC 9075-2:2023 §17.7, §17.8] COMMIT if `commit`, else
    /// ROLLBACK: the transaction ended, none active until a statement
    /// after begins one.
    fn end_transaction(&mut self, commit: bool) -> Result<Completion> {
        let Context::Own { st, tx } = &self.context else {
            return Err(SqlError::new(crate::error::Code::Api, "[DESIGN] only a control statement run directly ends its transaction"));
        };
        tx.borrow_mut().take();
        let ended = self.session.end_within(st, commit);
        self.refreshed()?;
        ended.map(|()| Completion::Success)
    }
}

impl SessionHost<'_, '_> {
    /// [ISO/IEC 9075-2:2023 §17.5, §17.6, §17.8] The savepoint's statement
    /// `s` of an SQL control statement run directly: SAVEPOINT in a
    /// transaction begun if none is active, which RELEASE and ROLLBACK TO
    /// need; its state taken anew after, as ROLLBACK TO may cancel changes
    /// of definitions.
    fn savepoint(&mut self, s: &SavepointStatement) -> Result<Completion> {
        let active = match &self.context {
            Context::Own { tx, .. } => tx.borrow().is_some(),
            Context::Within(_) => true,
        };
        if !active && !matches!(s, SavepointStatement::Establish(_)) {
            return Err(none_named(s));
        }
        let session = self.session;
        let done = self.with(|cx| session.savepoint_statement(cx, s, false));
        self.refreshed()?;
        done
    }
}

/// Whether `stmt` changes a definition, so the statements after it see a
/// new state.
fn defines(stmt: &SqlStatement) -> bool {
    matches!(stmt, SqlStatement::Drop { .. } | SqlStatement::AlterTable { .. } | SqlStatement::Schema { .. } | SqlStatement::Index(_))
}

impl Host for SessionHost<'_, '_> {
    type Mark = Mark;

    fn rows(&self, leaf: &Leaf, frame: &Params, limit: Option<usize>) -> Result<Vec<Row>> {
        self.with(|cx| {
            self.fresh(cx.tx)?;
            for (t, needed) in leaf.query.plan.columns_read() {
                cx.st.read_lock_of(cx.tx, (&t, needed))?;
            }
            // A runner of its own, so no rows kept are read after a
            // statement changed them.
            let hosted = Hosted::new(Some(self.session), cx);
            // [DESIGN] A leaf of a program kept, by its plans kept (RULES.md D408).
            if let Some(plans) = self.kept.map(|kept| kept.leaves.of(cx.st, leaf)).transpose()?.flatten() {
                return rows(&KeptRuns { runner: &hosted, plans: &plans, at: (cx.st, cx.tx.id) }, leaf, frame, limit);
            }
            rows(&hosted, leaf, frame, limit)
        })
    }

    fn present(&self, row: &mut Row) {
        show_in(row, self.session.now.get().zone);
    }

    fn interrupted(&self) -> Result<()> {
        self.session.stop.check()
    }

    fn noted(&self, e: &SqlError) {
        self.session.stop.served().note(e.sqlstate(), &e.msg);
    }

    fn run(&mut self, c: &Contained, frame: &mut Params) -> Result<Completion> {
        if matches!(c.statement(), SqlStatement::Commit | SqlStatement::Rollback) {
            return self.end_transaction(matches!(c.statement(), SqlStatement::Commit));
        }
        if let (SqlStatement::Savepoint(s), Context::Own { .. }) = (c.statement(), &self.context) {
            return self.savepoint(s);
        }
        frame.set_routine(Arc::clone(&c.scope));
        let session = self.session;
        // [DESIGN] An INSERT, searched UPDATE or searched DELETE of a
        // program kept keeps its plan (RULES.md D406).
        let kept = self.kept.and_then(|k| k.plans_of(c)).filter(|_| keeps_plan(c.statement()));
        let ran = self.with(|cx| {
            self.fresh(cx.tx)?;
            session.may_run(cx.st, c.statement())?;
            let keeping = kept.map(|plans| session.keeping(cx.st, plans));
            session.run_statement(cx, c.statement(), frame, keeping.as_ref())
        });
        if defines(c.statement()) {
            self.refreshed()?;
        }
        ran
    }

    fn execution(&mut self) -> usize {
        self.session.nesting.execution()
    }

    fn declare(&mut self, key: &str, cursor: &Cursor) -> Result<()> {
        self.session.declare_local(key, cursor)
    }

    fn open(&mut self, key: &str, cursor: &Cursor, frame: &mut Params) -> Result<()> {
        frame.set_routine(Arc::clone(&cursor.scope));
        // [DESIGN] A local cursor of a program kept, opened from the
        // specification and plan it keeps (RULES.md D416).
        let kept = self.kept.and_then(|k| k.cursor_plans(cursor));
        let session = self.session;
        self.with(|cx| {
            self.fresh(cx.tx)?;
            let keeping = kept.map(|plans| session.keeping(cx.st, plans));
            session.open_local(cx, (key, cursor), frame, keeping.as_ref())
        })
    }

    fn fetch(&mut self, key: &str, orientation: &FetchOrientation, frame: &Params) -> Result<Option<Row>> {
        self.with(|cx| self.session.fetch_local(cx, key, orientation, frame))
    }

    fn close(&mut self, key: &str) -> Result<()> {
        self.session.close_local(key)
    }

    fn destroy(&mut self, key: &str) {
        self.session.destroy_local(key);
    }

    fn mark(&mut self) -> Result<Option<Mark>> {
        self.with(|cx| cx.tx.mark().map(Some))
    }

    fn undo(&mut self, mark: Mark, failure: &SqlError) -> Result<()> {
        self.with(|cx| cx.st.undo_to(cx.tx, mark, failure))
    }

    fn release(&mut self, mark: Mark) {
        match &self.context {
            Context::Within(cx) => cx.tx.release_mark(mark),
            Context::Own { tx, .. } => {
                if let Some(tx) = tx.borrow().as_ref() {
                    tx.release_mark(mark);
                }
            }
        }
    }

    fn open_level(&mut self, atomic: bool) {
        self.session.open_level(if atomic { Level::Atomic } else { Level::Procedure });
    }

    fn close_level(&mut self) {
        self.session.close_level();
    }

    fn begin(&mut self, step: &Step) {
        if !diagnostics(step) {
            self.session.emptied((step_command(step), commands::UNRECOGNIZED));
        }
    }

    fn end(&mut self, step: &Step, outcome: &Result<Completion>) {
        if diagnostics(step) {
            return;
        }
        let outcome = outcome.as_ref().map(|c| *c).map_err(SqlError::clone);
        let _ = self.session.diagnosed(outcome, |done| done.sqlstate());
    }

    fn push_area(&mut self) {
        self.session.push_area();
    }

    fn pop_area(&mut self) {
        self.session.pop_area();
    }
}

/// Whether `stmt`, a statement of a program kept, keeps its plan as a
/// prepared statement does: an INSERT, a searched UPDATE or DELETE, or
/// the one of these a SELECT ... INTO reads as its delta table (RULES.md
/// D792).
fn keeps_plan(stmt: &SqlStatement) -> bool {
    if let SqlStatement::Select { query, .. } = stmt
        && let [TableReference::Delta(d)] = query.table_expression.from.as_slice()
    {
        return changes_kept(&d.statement);
    }
    changes_kept(stmt)
}

/// Whether `stmt` is an INSERT, a searched UPDATE or a searched DELETE,
/// whose plan a program kept keeps.
fn changes_kept(stmt: &SqlStatement) -> bool {
    matches!(stmt, SqlStatement::Insert(_) | SqlStatement::UpdateSearched { .. } | SqlStatement::DeleteSearched { .. })
}

#[cfg(test)]
mod tests;

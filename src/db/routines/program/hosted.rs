//! [ISO/IEC 9075-4:2006 draft §11.2; ISO/IEC 9075-2:2023 §4.35.2] What
//! evaluates the values of a statement of the session that may invoke a
//! function that possibly modifies SQL-data: the SET values of an UPDATE
//! or a MERGE, the rows of an INSERT's or MERGE's VALUES, the arguments of
//! a CALL, and the values of a program (`binder::Binder::hosting`).
//!
//! It runs subqueries as the statement's runner does; a function that
//! possibly modifies SQL-data it runs with the session as its host, each
//! statement of its body a statement of the session nested in the one
//! that invokes it, in the context its invocation pushes [ISO/IEC
//! 9075-2:2023 §9.18 GR6]. Its changes are the statement's, undone with
//! it if it fails.

use std::sync::Arc;

use super::super::super::savepoints::Level;
use super::super::super::{Session, StatementContext};
use crate::catalog::{SequenceDesc, owner_of};
use crate::db::State;
use crate::error::{Code, Result, SqlError, Warnings};
use crate::exec::{Db, Ended, Runner, Subqueries, SubqueryRows, eval};
use crate::logical::{Expr, Invocation, LogicalPlan, Subquery};
use crate::params::Params;
use crate::storage::{Row, sized_row};
use crate::value::Value;

/// \[DESIGN\] A statement's runner, with the session that runs a function
/// that possibly modifies SQL-data, if one does.
pub(in crate::db) struct Hosted<'s> {
    session: Option<&'s Session>,
    cx: StatementContext<'s>,
    runner: Runner,
}

impl<'s> Hosted<'s> {
    /// The runner of the statement in `cx` of `session`, if it is one.
    pub(in crate::db) fn new(session: Option<&'s Session>, cx: StatementContext<'s>) -> Hosted<'s> {
        Hosted { session, cx, runner: Runner::new(cx.st, cx.tx.id) }
    }

    /// The runner of the statement in `cx` of `session`, in `db`, whose
    /// slots its plans and expressions read (RULES.md D610).
    pub(in crate::db) fn in_db(session: Option<&'s Session>, cx: StatementContext<'s>, db: &Db) -> Hosted<'s> {
        Hosted { session, cx, runner: Runner::new(db, cx.tx.id) }
    }

    /// Its runner.
    pub(in crate::db) fn runner(&self) -> &Runner {
        &self.runner
    }
}

impl Subqueries for Hosted<'_> {
    fn rows(&self, subquery: &Subquery, outer: &[Value], limit: Option<usize>) -> Result<SubqueryRows> {
        self.runner.rows(subquery, outer, limit)
    }

    fn slot(&self, index: usize) -> Option<&Value> {
        self.runner.slot(index)
    }

    fn outer(&self, index: usize) -> Option<&Value> {
        self.runner.outer(index)
    }

    fn warnings(&self) -> Option<&Warnings> {
        self.runner.warnings()
    }

    fn like(&self, value: &str, pattern: &str, escape: Option<char>) -> Result<bool> {
        self.runner.like(value, pattern, escape)
    }

    fn next_value(&self, seq: &SequenceDesc) -> Result<Value> {
        self.runner.next_value(seq)
    }

    fn interrupted(&self) -> Result<()> {
        self.runner.interrupted()
    }

    fn state(&self) -> Option<&Arc<State>> {
        self.runner.state()
    }

    fn invoke_hosted(&self, i: &Invocation, values: Vec<Value>) -> Option<Result<Option<Value>>> {
        let session = self.session.filter(|_| i.modifies)?;
        Some(session.invoke_function(self.cx, i, values))
    }
}

/// [ISO/IEC 9075-2:2023 §15.11 GR3, §9.18 GR3] The rows of `plan`, the
/// VALUES of an INSERT, or the arguments of a CALL, as a row, or a
/// projection of one, if it invokes a function that possibly modifies
/// SQL-data: each evaluated in turn by `hosted`, not as a query, which no
/// SQL-session runs, the tables it reads locked first. \[DESIGN\] So too
/// if every value reads nothing (`Expr::reads_nothing`), as planning
/// such a row would give the same values with more work (RULES.md
/// D419). None else.
pub(in crate::db) fn hosted_rows(plan: &LogicalPlan, hosted: &Hosted<'_>) -> Option<Result<Vec<Row>>> {
    let (rows, projected) = evaluated(plan)?;
    for t in plan.tables() {
        if let Err(e) = hosted.cx.st.read_lock(hosted.cx.tx, &t) {
            return Some(Err(e));
        }
    }
    let row_of = |exprs: &[Expr], input: &Row| sized_row(exprs.len(), exprs.iter().map(|e| eval(e, input, hosted)));
    // A row of no projection is the values themselves, not a copy.
    let made = |r: &Vec<Expr>| {
        let values = row_of(r, &Row::new())?;
        match projected {
            Some(p) => row_of(p, &values),
            None => Ok(values),
        }
    };
    Some(rows.iter().map(made).collect())
}

impl Session {
    /// [ISO/IEC 9075-2:2023 §9.18 GR6, GR8b] The function `i`, which
    /// possibly modifies SQL-data, invoked in `cx` with the parameters'
    /// values `values`: its body run with the session as its host, in the
    /// context its invocation pushes; the value it returns, none if it
    /// returns none.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 of a function whose body is no program; as `push`'s;
    /// an exception of it no handler of it handles.
    pub(in crate::db) fn invoke_function(&self, cx: StatementContext<'_>, i: &Invocation, mut values: Vec<Value>) -> Result<Option<Value>> {
        if let Some(p) = &i.rust {
            return self.rust_function(cx, &**p, &values).map(Some);
        }
        let program = i.program.as_ref().ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] function {} has no program", i.name)))?;
        values.resize(program.base.saturating_add(program.variables.len()), Value::Null);
        let mut frame = Params::of_routine(values, Arc::clone(&program.scope));
        let owner = owner_of(&*cx.st.catalog, &i.name.auth).to_owned();
        // [ISO/IEC 9075-2:2023 §9.18 GR7, §11.60 GR3k] a function at a
        // savepoint level of its own.
        let _pushed = self.push(&i.name, (owner, i.name.auth.clone()), Some(Level::Function))?;
        match self.run_hosted(cx, program, &mut frame)? {
            Ended::Returned(v) => Ok(Some(v)),
            Ended::Completed(_) => Ok(None),
        }
    }
}

/// The rows of a VALUES and the projection over them, if any.
type Evaluated<'p> = (&'p Vec<Vec<Expr>>, Option<&'p Vec<Expr>>);

/// The rows of `plan` and the projection over them, if `hosted_rows`
/// evaluates them in turn, not as a query: a row, or a projection of one,
/// that invokes a function that possibly modifies SQL-data, or whose
/// every value reads nothing.
fn evaluated(plan: &LogicalPlan) -> Option<Evaluated<'_>> {
    let (rows, projected) = if let LogicalPlan::Values { rows, .. } = plan {
        (rows, None)
    } else if let LogicalPlan::Project { exprs, input } = plan
        && let LogicalPlan::Values { rows, .. } = &**input
    {
        (rows, Some(exprs))
    } else {
        return None;
    };
    let reads_none = rows.iter().flatten().chain(projected.into_iter().flatten()).all(Expr::reads_nothing);
    (reads_none || rows.iter().flatten().any(Expr::invokes_modifying)).then_some((rows, projected))
}

/// \[EXTENSION\] Whether the rows of `plan`, an INSERT's query, are
/// evaluated in turn as `hosted_rows` evaluates them, with no plan, which
/// EXPLAIN says of the INSERT (RULES.md D980).
pub(in crate::db) fn evaluates(plan: &LogicalPlan) -> bool {
    evaluated(plan).is_some()
}

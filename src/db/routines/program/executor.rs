//! [ISO/IEC 9075-4:2006 draft §11.2] The host of a function whose body is
//! an SQL control statement, run where a query evaluates it: one that
//! changes no SQL-data, as its SQL-data access says (RULES.md D76).
//!
//! Its leaves run as the query's subqueries do, in the statement's
//! transaction. Its local cursors are its own, each its rows as it is
//! opened, which FETCH moves through as a scrollable cursor's [ISO/IEC
//! 9075:1992 §13.3]. Its diagnostics area, and the stack below it, are its
//! own too, as a routine's are [ISO/IEC 9075-2:2023 §9.18 GR6f], read by
//! GET DIAGNOSTICS as the session's are, by the owner of the routine whose
//! body it is in. A CALL is bound with the program, so the procedure's
//! body runs here too; no statement of it changes SQL-data, as none of the
//! function's may.

use std::collections::HashMap;
use std::sync::Arc;

use super::super::super::area::commands::UNRECOGNIZED;
use super::super::super::area::{Area, Reader};
use super::super::super::cursor::movement;
use super::super::super::cursor_error;
use super::super::super::position::{Places, Position, Slot, fetch};
use super::super::super::savepoints::{Level, Levels};
use super::{diagnostics, step_command};
use crate::ast::{DynamicStatement, FetchOrientation, SavepointStatement, SqlStatement};
use crate::binder;
use crate::catalog::{Principal, Roles, owner_of};
use crate::db::dynamic::no_ptf_area_here;
use crate::db::ptf::{DataArea, run_statement};
use crate::db::{Completion, State};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::exec::{Ended, Host, Subqueries, rows, run_program};
use crate::logical::{Contained, Cursor, Leaf, Program, Step};
use crate::params::Params;
use crate::storage::Row;
use crate::value::Value;

/// [ISO/IEC 9075-4:2006 draft §11.2; ISO/IEC 9075-2:2023 §9.18 GR8b] The
/// value the function whose body is `program` returns of the parameters'
/// values `values`, run as `subqueries` run a query's subqueries; none if
/// it ends with no RETURN executed.
///
/// # Errors
///
/// An exception of it no handler of it handles.
pub(crate) fn run_unhosted(program: &Program, mut values: Vec<Value>, subqueries: &dyn Subqueries) -> Result<Option<Value>> {
    values.resize(program.base.saturating_add(program.variables.len()), Value::Null);
    let mut frame = Params::of_routine(values, Arc::clone(&program.scope));
    let mut host =
        Executor { subqueries, area: Area::default(), cursors: HashMap::new(), executions: 0, ptf: None, savepoints: Levels::default() };
    match run_program(program, &mut frame, &mut host)? {
        Ended::Returned(v) => Ok(Some(v)),
        Ended::Completed(_) => Ok(None),
    }
}

/// [ISO/IEC 9075-2:2023 §9.27 GR4, §9.18 GR8] The PTF component procedure
/// whose body is `program` run of the arguments `values` on the PTF data
/// area `ptf`, where `subqueries` runs a query's subqueries: the values of
/// its parameters after, of which its INOUT ones give the area's private
/// variables and status variable their values.
///
/// # Errors
///
/// An exception of it no handler of it handles.
pub(crate) fn run_component(
    program: &Program,
    mut values: Vec<Value>,
    subqueries: &dyn Subqueries,
    ptf: &mut DataArea,
) -> Result<Vec<Value>> {
    values.resize(program.base.saturating_add(program.variables.len()), Value::Null);
    let mut frame = Params::of_routine(values, Arc::clone(&program.scope));
    let mut host = Executor {
        subqueries,
        area: Area::default(),
        cursors: HashMap::new(),
        executions: 0,
        ptf: Some(ptf),
        savepoints: Levels::default(),
    };
    run_program(program, &mut frame, &mut host)?;
    Ok(frame.dynamic_values().to_vec())
}

/// \[DESIGN\] A query's evaluation as a function's host.
struct Executor<'s> {
    subqueries: &'s dyn Subqueries,
    area: Area,
    /// Its local cursors, by their names in their executions: each, if
    /// open, its rows and where it is among them.
    cursors: HashMap<String, Option<(Vec<Row>, Position)>>,
    executions: usize,
    /// [ISO/IEC 9075-2:2023 §9.27] The PTF data area of the PTF component
    /// procedure it runs, if it runs one: what the SQL dynamic statements
    /// of its body name (`db/ptf/statements.rs`).
    ptf: Option<&'s mut DataArea>,
    /// [ISO/IEC 9075-2:2023 §9.18 GR7] Its savepoint levels and
    /// savepoints, the function's level its own.
    savepoints: Levels<()>,
}

/// The rows of an open local cursor, as a FETCH moves through them.
struct Listed<'r>(&'r [Row]);

impl Places for Listed<'_> {
    fn at(&mut self, i: usize) -> Result<Slot> {
        Ok(if i < self.0.len() { Slot::Row } else { Slot::End })
    }

    fn row(&mut self, i: usize) -> Result<Row> {
        self.0.get(i).cloned().ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] the cursor has no row {i}")))
    }

    fn count(&mut self) -> Result<usize> {
        Ok(self.0.len())
    }
}

impl Executor<'_> {
    /// The database the function runs in.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 where no statement runs, as in a constraint, where no
    /// function that needs it is invoked.
    fn state(&self) -> Result<&State> {
        Self::state_in(self.subqueries)
    }

    /// The database `subqueries` run in, as `state` gives it.
    fn state_in(subqueries: &dyn Subqueries) -> Result<&State> {
        subqueries
            .state()
            .map(|st| &**st)
            .ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] a function's body reads the database where no statement runs"))
    }

    /// The principal the statement `c` runs as: the owner of the schema of
    /// the routine whose body it is in [ISO/IEC 9075-2:2023 §9.18 GR6l].
    fn principal<'a>(st: &'a State, c: &'a Contained) -> Principal<'a> {
        Principal::sole(st.catalog.roles(), owner_of(&*st.catalog, &c.scope.name.auth))
    }

    /// [ISO/IEC 9075-2:2023 §17.5, §17.6, §17.8] The savepoint's statement
    /// `s`, of a function that changes no SQL-data: its savepoints are
    /// only named, as going back to one has no change to cancel.
    fn savepoint(&mut self, s: &SavepointStatement) -> Result<()> {
        match s {
            SavepointStatement::Establish(name) => {
                self.savepoints.make_room(name)?;
                self.savepoints.establish(name, ());
            }
            SavepointStatement::Release(name) => drop(self.savepoints.release(name)?),
            SavepointStatement::RollbackTo(name) => {
                self.savepoints.target(name)?;
                drop(self.savepoints.after(name));
            }
        }
        Ok(())
    }

    /// GET DIAGNOSTICS `c` of the area here.
    fn get_diagnostics(&self, c: &Contained, frame: &mut Params) -> Result<Completion> {
        let SqlStatement::GetDiagnostics(get) = &c.statement else { return Ok(Completion::Success) };
        let st = self.state()?;
        let area = self.area.read(get.stacked)?;
        // [ISO/IEC 9075-2:2023 §23.1 GR2j] a function runs in the
        // transaction of the statement that invokes it.
        Reader { st, user: Self::principal(st, c), active: true }.get(&area, get, frame)
    }
}

impl Host for Executor<'_> {
    type Mark = ();

    fn rows(&self, leaf: &Leaf, frame: &Params, limit: Option<usize>) -> Result<Vec<Row>> {
        rows(self.subqueries, leaf, frame, limit)
    }

    /// As a `<return statement>`'s value is: the datetimes as the statement
    /// reads them.
    fn present(&self, _: &mut Row) {}

    fn interrupted(&self) -> Result<()> {
        self.subqueries.interrupted()
    }

    fn run(&mut self, c: &Contained, frame: &mut Params) -> Result<Completion> {
        frame.set_routine(Arc::clone(&c.scope));
        if let SqlStatement::GetDiagnostics(_) = c.statement {
            return self.get_diagnostics(c, frame);
        }
        if let SqlStatement::Savepoint(s) = &c.statement {
            return self.savepoint(s).map(|()| Completion::Success);
        }
        if let SqlStatement::Dynamic(d) = &c.statement {
            // [DESIGN] A describe component procedure runs as a definition
            // is bound too, where no statement runs: its operands, SQL
            // variables, parameters and literals, need no role.
            let none = Roles::default();
            let roles = Self::state_in(self.subqueries).map_or(&none, |st| st.catalog.roles());
            let user = Principal::sole(roles, &c.scope.name.auth);
            if let Some(area) = self.ptf.as_deref_mut() {
                return run_statement(area, d, frame, user);
            }
            // [ISO/IEC 9075-2:2023 §20.28 GR1, §5.4] of a routine run where
            // no PTF component procedure runs.
            if let DynamicStatement::PipeRow(_) = &**d {
                let msg =
                    "[ISO/IEC 9075-2:2023 §20.28 GR1] PIPE ROW is executed by a PTF start, fulfill or finish component procedure alone";
                return Err(SqlError::new(Code::Dynamic, msg).with_state(SqlState::PIPE_ROW_OUTSIDE));
            }
            return Err(no_ptf_area_here());
        }
        let msg = "[DESIGN] a statement that changes SQL-data in the body of a function run where a query evaluates it";
        Err(SqlError::new(Code::Api, msg))
    }

    fn execution(&mut self) -> usize {
        self.executions = self.executions.wrapping_add(1);
        self.executions
    }

    fn declare(&mut self, key: &str, _: &Cursor) -> Result<()> {
        self.cursors.insert(key.to_owned(), None);
        Ok(())
    }

    fn open(&mut self, key: &str, cursor: &Cursor, frame: &mut Params) -> Result<()> {
        if self.cursors.get(key).is_some_and(Option::is_some) {
            return cursor_error(&cursor.name, format!("[X3.135-1986 §8.8 GR1] cursor {} is already open", cursor.name));
        }
        frame.set_routine(Arc::clone(&cursor.scope));
        let found = self.rows(&cursor.query, frame, None)?;
        self.cursors.insert(key.to_owned(), Some((found, Position::Before(0))));
        Ok(())
    }

    fn fetch(&mut self, key: &str, orientation: &FetchOrientation, frame: &Params) -> Result<Option<Row>> {
        let name = key.split('\u{0}').next().unwrap_or_default().to_owned();
        let to = match orientation {
            FetchOrientation::Absolute(_) | FetchOrientation::Relative(_) => {
                let st = self.state()?;
                let user = Principal::sole(st.catalog.roles(), owner_of(&*st.catalog, frame.routine_scope().map_or("", |r| &r.name.auth)));
                movement(orientation, true, &|v| binder::value_specification(v, user, frame))?
            }
            FetchOrientation::Next | FetchOrientation::Prior | FetchOrientation::First | FetchOrientation::Last => {
                movement(orientation, true, &|_| Err(SqlError::new(Code::Api, "[DESIGN] no value of the orientation")))?
            }
        };
        let Some(Some((found, at))) = self.cursors.get_mut(key) else {
            return cursor_error(&name, format!("[X3.135-1986 §8.6 GR1] cursor {name} is not open"));
        };
        let (moved, row) = fetch(&mut Listed(found), *at, to)?;
        *at = moved;
        Ok(row)
    }

    fn close(&mut self, key: &str) -> Result<()> {
        let name = key.split('\u{0}').next().unwrap_or_default().to_owned();
        match self.cursors.get_mut(key) {
            Some(open @ Some(_)) => {
                *open = None;
                Ok(())
            }
            Some(None) | None => cursor_error(&name, format!("[X3.135-1986 §8.1 GR1] cursor {name} is not open")),
        }
    }

    fn destroy(&mut self, key: &str) {
        self.cursors.remove(key);
    }

    fn mark(&mut self) -> Result<Option<()>> {
        Ok(None)
    }

    fn undo(&mut self, (): (), _: &SqlError) -> Result<()> {
        Ok(())
    }

    fn release(&mut self, (): ()) {}

    fn open_level(&mut self, atomic: bool) {
        self.savepoints.open(if atomic { Level::Atomic } else { Level::Procedure });
    }

    fn close_level(&mut self) {
        drop(self.savepoints.close());
    }

    fn begin(&mut self, step: &Step) {
        if !diagnostics(step) {
            self.area.begin((step_command(step), UNRECOGNIZED));
        }
    }

    fn end(&mut self, step: &Step, outcome: &Result<Completion>) {
        if !diagnostics(step) {
            self.area.place(outcome);
        }
    }

    fn push_area(&mut self) {
        self.area.push();
    }

    fn pop_area(&mut self) {
        self.area.pop();
    }
}

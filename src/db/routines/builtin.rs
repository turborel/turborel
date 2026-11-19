//! \[EXTENSION\] CALL of a procedure written in Rust, built in or
//! registered (`functions/builtin.rs`): one invoked by its name,
//! unqualified, which no schema holds. Its arguments are bound to its
//! parameters' types and evaluated as an SQL-invoked procedure's are
//! [ISO/IEC 9075-2:2023 §9.18 GR3, GR4]; it then runs in the caller's
//! SQL-session and transaction, its statements the session's, as the
//! current user, and its changes to the catalog staged with the
//! transaction's, undone if it rolls back (RULES.md D125).

use std::sync::Arc;

use super::super::compiled::plan;
use super::super::ddl::Changed;
use super::super::{Session, State, StatementContext, rule};
use super::call::defaulted;
use crate::ast::{CallStatement, SqlArgument};
use crate::binder::{Context, bind_argument, bind_cursor};
use crate::catalog::{QualifiedName, SpatialChange, owner_of};
use crate::error::{Code, Result, SqlError};
use crate::exec::execute;
use crate::functions::builtin::{Procedure, ProcedureContext};
use crate::geometry::{fixed_srid, of_data_type};
use crate::log::record::Change;
use crate::logical::{Expr, LogicalPlan};
use crate::params::Params;
use crate::parser::Parser;
use crate::txn::LockMode;
use crate::value::{DataType, Value, assign};

/// The name of the lock a change to the spatial catalog takes, so two
/// transactions change it one after the other.
const SPATIAL_LOCK: &str = "ST_DEFINITION_SCHEMA";

impl Session {
    /// The procedure written in Rust that `call` invokes, if it names one,
    /// unqualified.
    pub(in crate::db) fn rust_procedure(st: &State, call: &CallStatement) -> Option<Arc<dyn Procedure>> {
        call.name.auth.is_none().then(|| st.functions.procedure(&call.name.ident)).flatten()
    }

    /// [ISO/IEC 9075-2:2023 §9.18 GR3, GR4] CALL of `p`, in `cx` with
    /// `params`: its arguments evaluated and assigned to its parameters'
    /// types, then it run in the session.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it takes no such count of arguments; an argument's;
    /// the procedure's.
    pub(in crate::db) fn rust_call(
        &self,
        cx: StatementContext<'_>,
        p: &dyn Procedure,
        call: &CallStatement,
        params: &mut Params,
    ) -> Result<super::super::Completion> {
        let (inputs, types) = self.bound(cx.st, params, |b| rust_arguments(b, p, call))?;
        let logical = LogicalPlan::Values { rows: vec![inputs], types: types.clone() };
        let row = Session::query_rows(cx, logical)?.into_iter().next().unwrap_or_default();
        let values =
            row.into_iter().zip(&types).map(|(v, t)| assign(v, t, "ISO/IEC 9075-2:2023 §9.18 GR4c i")).collect::<Result<Vec<_>>>()?;
        // [ISO/IEC 9075-2:2023 §4.41.5] a CALL that fails has no effect:
        // its rows, then the definitions it staged.
        let staged = self.inner.borrow().staged.as_ref().map_or(0, super::super::ddl::Staged::len);
        let mark = cx.tx.mark()?;
        if let Err(e) = p.call(&values, &mut InSession { session: self, tx: cx.tx }) {
            let undone = cx.st.undo_to(cx.tx, mark, &e);
            let now = self.inner.borrow().staged.as_ref().map_or(0, super::super::ddl::Staged::len);
            let cancelled = if now > staged { self.cancel_definitions(cx.tx, staged) } else { Ok(()) };
            return Err(undone.and(cancelled).err().unwrap_or(e));
        }
        cx.tx.release_mark(mark);
        Ok(super::super::Completion::Success)
    }

    /// \[EXTENSION\] The value of `p`, a procedure written in Rust invoked
    /// as a function, of the parameters' values `values`, run in the
    /// session and transaction of `cx`; its changes are the statement's.
    ///
    /// # Errors
    ///
    /// The function's own.
    pub(in crate::db) fn rust_function(&self, cx: StatementContext<'_>, p: &dyn Procedure, values: &[Value]) -> Result<Value> {
        p.evaluate(values, &mut InSession { session: self, tx: cx.tx })
    }

    /// The rows of `logical` in `cx`, the tables it reads locked.
    fn query_rows(cx: StatementContext<'_>, logical: LogicalPlan) -> Result<Vec<Vec<Value>>> {
        let (optimized, _, physical) = plan(cx.st, logical);
        for t in optimized.tables() {
            cx.st.read_lock(cx.tx, &t)?;
        }
        execute(physical, cx.st, cx.tx.id).collect()
    }

    /// [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §20.2, §20.3] `change`
    /// staged with transaction `tx`'s changes, once the lock of the
    /// spatial catalog is held.
    ///
    /// # Errors
    ///
    /// As `SpatialCatalog::applied`; SQLCODE -103 if a column registered
    /// is of no table, or of no geometry type, or of a type whose SRID is
    /// not the system's [§20.2 COLUMN_EXISTS]; the lock's.
    pub(in crate::db) fn define_spatial(&self, tx: &crate::txn::Txn, change: SpatialChange) -> Result<()> {
        let before = self.definitions(self.db.snapshot()?)?;
        before.acquire_name(tx, &QualifiedName { auth: SPATIAL_LOCK.to_owned(), ident: SPATIAL_LOCK.to_owned() }, LockMode::Exclusive)?;
        let fresh = self.definitions(self.db.snapshot()?)?;
        match &change {
            SpatialChange::Register(c) => registrable(&fresh, (&c.table, &c.column, c.srs.as_deref()), &self.auth())?,
            SpatialChange::Unregister(table, column) => registrable(&fresh, (table, column, None), &self.auth())?,
            SpatialChange::RegisterLr(c) => lr_registrable(&fresh, (&c.table, &c.column), &self.auth())?,
            SpatialChange::UnregisterLr(table, column) => lr_registrable(&fresh, (table, column), &self.auth())?,
            SpatialChange::Define(_) | SpatialChange::Drop(_) | SpatialChange::DefineLrm(_) | SpatialChange::DropLrm(_) => {}
        }
        let mut next = State::clone(&fresh);
        next.apply_change(&Change::Spatial(change.clone()), None)?;
        // [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §4.11] the rows a column
        // holds are of the system it is registered with.
        if let SpatialChange::Register(c) = &change
            && let Some(desc) = next.catalog.table(&c.table)
        {
            let spatial = next.catalog.spatial();
            fresh.each_row_where(tx, &c.table, &|_| true, &mut |_, row| {
                crate::geometry::registered::check_row_of(spatial, desc, &row).map(|()| std::ops::ControlFlow::Continue(()))
            })?;
        }
        self.stage(&fresh, tx, Changed { next: Arc::new(next), change: Change::Spatial(change), made: None })
    }
}

/// [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §20.2] That column `column`
/// of `table` may be registered with the system named `srs`, or not, by
/// `user`: the table is there, its schema `user`'s, the column of a
/// geometry type [COLUMN_EXISTS, Description 2], and of a type of an SRID,
/// that SRID the system's.
///
/// # Errors
///
/// SQLCODE -103 if not; -104 if the schema is not `user`'s.
fn registrable(st: &State, (table, column, srs): (&QualifiedName, &str, Option<&str>), user: &str) -> Result<()> {
    let desc = st.catalog.table(table).ok_or_else(|| rule(format!("[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §20.2] no table {table}")))?;
    if owner_of(&*st.catalog, &table.auth) != user {
        return Err(SqlError::new(
            Code::Privilege,
            format!(
                "[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §20.2] {user} owns no schema {}, so registers no column of {table}",
                table.auth
            ),
        ));
    }
    let c = desc
        .columns
        .iter()
        .find(|c| c.name == column)
        .ok_or_else(|| rule(format!("[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §20.2 COLUMN_EXISTS] {table} has no column {column}")))?;
    if of_data_type(&c.data_type).is_none() {
        return Err(rule(format!(
            "[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §20.2 Description 2] column {column} of {table} is of {}, no geometry type",
            c.data_type
        )));
    }
    let id = srs.and_then(|name| st.catalog.spatial().system_id(name));
    if let (Some(fixed), Some(id)) = (fixed_srid(&c.data_type), id)
        && fixed != id
    {
        return Err(rule(format!(
            "[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §20.2] column {column} of {table} holds values of SRID {fixed}, not {id}"
        )));
    }
    Ok(())
}

/// [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §21.2.2] That column `column`
/// of `table` may be registered with a linear referencing method, or not,
/// by `user`: the table is there, its schema `user`'s, the column of a
/// linear referencing type [COLUMN_EXISTS, Description 2].
///
/// # Errors
///
/// SQLCODE -103 if not; -104 if the schema is not `user`'s.
fn lr_registrable(st: &State, (table, column): (&QualifiedName, &str), user: &str) -> Result<()> {
    let at = "[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §21.2.2";
    let desc = st.catalog.table(table).ok_or_else(|| rule(format!("{at}] no table {table}")))?;
    if owner_of(&*st.catalog, &table.auth) != user {
        let msg = format!("{at}] {user} owns no schema {}, so registers no column of {table}", table.auth);
        return Err(SqlError::new(Code::Privilege, msg));
    }
    let c = desc
        .columns
        .iter()
        .find(|c| c.name == column)
        .ok_or_else(|| rule(format!("{at} COLUMN_EXISTS] {table} has no column {column}")))?;
    if crate::spatial::lr::sql::of_data_type(&c.data_type).is_none() {
        return Err(rule(format!("{at} Description 2] column {column} of {table} is of {}, no linear referencing type", c.data_type)));
    }
    Ok(())
}

/// [ISO/IEC 9075-2:2023 §9.18 SR10, §10.4] The arguments of `call` of
/// `p`, bound in `cx` to its parameters' types, and those types.
///
/// # Errors
///
/// SQLCODE -103 if it takes no such count of arguments; -900 of NULL or
/// DEFAULT (Feature T522); an argument's.
pub(in crate::db) fn rust_arguments(cx: Context<'_>, p: &dyn Procedure, call: &CallStatement) -> Result<(Vec<Expr>, Vec<DataType>)> {
    if p.returns(call.arguments.len()).is_some() {
        return Err(rule(format!("[ISO/IEC 9075-2:2023 §10.4 SR2] {} is a function, which no CALL invokes", p.name())));
    }
    let types = p.parameters(call.arguments.len()).ok_or_else(|| {
        rule(format!("[ISO/IEC 9075-2:2023 §9.18 SR10] procedure {} takes no {} arguments", p.name(), call.arguments.len()))
    })?;
    let mut inputs = Vec::with_capacity(types.len());
    let mut bound = Vec::with_capacity(types.len());
    for (a, t) in call.arguments.iter().zip(&types) {
        let v = match a {
            SqlArgument::Value(v) => v,
            SqlArgument::Default(at) | SqlArgument::Null(at) => return defaulted("T522", 6, *at),
        };
        let (e, of) = bind_argument(cx, v, t)?;
        inputs.push(e);
        bound.push(of);
    }
    // [§9.6] the arguments' types choose among its parameters' types.
    Ok((inputs, p.overload(&bound).unwrap_or(types)))
}

/// What a procedure written in Rust runs in: the session and transaction
/// of its CALL.
struct InSession<'s> {
    session: &'s Session,
    tx: &'s crate::txn::Txn,
}

impl InSession<'_> {
    /// The definitions as the transaction sees them now, its own changes
    /// among them.
    fn state(&self) -> Result<Arc<State>> {
        self.session.definitions(self.session.db.snapshot()?)
    }
}

impl ProcedureContext for InSession<'_> {
    fn run(&mut self, sql: &str, args: &[Value]) -> Result<()> {
        // [ISO/IEC 9075:1992 §20.1 GR6b ii, iii] a <schema definition>,
        // made in the transaction as a session's own is.
        let words: Vec<String> = sql.split_whitespace().take(2).map(str::to_ascii_uppercase).collect();
        if words == ["CREATE", "SCHEMA"] {
            let st = self.state()?;
            let name = Parser::new(sql)?.in_catalog(&st.catalog_name()).schema()?.name;
            return self.session.stage_schema(&st, self.tx, sql, &name).map(|_| ());
        }
        let statement = Parser::for_prepare(sql, true)?.sql_statement()?;
        let st = self.state()?;
        let mut params = Params::executing(args.to_vec());
        self.session.run_statement(StatementContext { st: &st, tx: self.tx }, &statement, &mut params, None).map(|_| ())
    }

    fn rows(&mut self, sql: &str, args: &[Value]) -> Result<Vec<Vec<Value>>> {
        let spec = Parser::for_prepare(sql, true)?.cursor_specification()?;
        let st = self.state()?;
        let params = Params::executing(args.to_vec());
        let logical = self.session.bound(&st, &params, |b| bind_cursor(b, &spec))?.plan;
        Session::query_rows(StatementContext { st: &st, tx: self.tx }, logical)
    }

    fn define_spatial(&mut self, change: SpatialChange) -> Result<()> {
        self.session.define_spatial(self.tx, change)
    }

    fn run_owned(&mut self, schema: &str, sql: &str, args: &[Value]) -> Result<()> {
        let st = self.state()?;
        let owner = owner_of(&*st.catalog, schema).to_owned();
        let name = QualifiedName { auth: schema.to_owned(), ident: schema.to_owned() };
        let _pushed = self.session.push(&name, (owner, schema.to_owned()), None)?;
        self.run(sql, args)
    }
}

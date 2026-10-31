//! A session's statements run [X3.135-1986 §7.3]: each parsed, run in its
//! transaction, undone if it fails, and its condition placed in the
//! diagnostics area (`db/area.rs`). Split from db.rs (RULES.md R-SIZE-2).

use std::sync::Arc;

use super::dynamic::{self, Name};
use super::prepared::{Keeping, Plans};
use super::write::{delete_searched, update_searched};
use super::{Call, Completion, Session, StatementContext, slow};
use crate::ast::{
    AlterAction, ConnectionStatement, DropObject, IndexStatement, PrivilegeDefinition, RevokeStatement, SchemaStatement, ServedStatement,
    SetClause, SqlStatement, TableName, TableStorage, UserStatement,
};
use crate::binder::{BoundWrite, Context, WriteKind, bind_write, qualify};
use crate::error::{Code, Result, SqlState, fail};
use crate::lexer::{Tok, Token, kw, lex};
use crate::params::Params;
use crate::settings::FUNCTION_CALLS;

/// A positioned DELETE or UPDATE run through a cursor (`Session::positioned`).
type Positioned = fn(&Session, StatementContext<'_>, (&Name, usize), &BoundWrite) -> Result<Completion>;

impl Session {
    /// Execute an `<SQL statement>` [X3.135-1986 §7.3]. `params` plays the part
    /// of the procedure's parameters: inputs are read from it, and FETCH and
    /// SELECT ... INTO write their targets into it.
    ///
    /// # Errors
    ///
    /// Any error of the statement, which is then undone [X3.135-1986 §7.3
    /// GR3b i]; SQLCODE -914 if undoing it failed too.
    pub fn execute(&self, sql: &str, params: &mut Params) -> Result<Completion> {
        self.execute_as(sql, params, None)
    }

    /// Runs an `<SQL statement>` with no parameters.
    ///
    /// # Errors
    ///
    /// As for `execute`.
    pub fn run(&self, sql: &str) -> Result<Completion> {
        self.execute(sql, &mut Params::new())
    }

    /// `execute`, of a statement prepared, if `prepared` names EXECUTE or
    /// EXECUTE IMMEDIATE, which runs it (`execute_prepared`), with the
    /// plans it keeps: the diagnostics area then names that, and the
    /// statement as its DYNAMIC_FUNCTION [ISO/IEC 9075:1992 §18.1 GR1c,
    /// GR1d].
    pub(super) fn execute_as(&self, sql: &str, params: &mut Params, prepared: Option<(&'static str, &Plans)>) -> Result<Completion> {
        let mut call = self.enter()?;
        call.timed = slow::Timed::of(call.state(), sql, &self.auth());
        let keeping = prepared.map(|(_, plans)| self.keeping(call.state(), plans));
        let parsed = Self::parsed(&call, sql, params, keeping.as_ref());
        // [ISO/IEC 9075:1992 §4.19] GET DIAGNOSTICS reads the area as the
        // statement before left it, and leaves it so, refused too.
        if let Ok(SqlStatement::GetDiagnostics(get)) = parsed.as_deref() {
            return self.get_diagnostics(call.state(), get, params);
        }
        if parsed.is_err() && diagnostics_statement(sql) {
            return parsed.map(|_| Completion::Success);
        }
        let (command, function) = parsed.as_deref().map_or(("", ""), |stmt| (command_of(stmt), prepared_command(stmt)));
        // [ISO/IEC 9075:1992 §17.10, §17.11] EXECUTE runs the statement
        // prepared as a statement of its own.
        if let Ok(SqlStatement::Execute(e)) = parsed.as_deref() {
            drop(call);
            return self.execute_dynamic(e, command, params);
        }
        self.emptied(prepared.map_or((command, ""), |(outer, _)| (outer, function)));
        let outcome = parsed.and_then(|stmt| {
            let target = target_of(&stmt).map(|t| qualify(t, &self.default_schema()));
            self.execute_parsed(&call, &stmt, params, keeping.as_ref()).map_err(|e| match target {
                Some(t) => e.or_about_table(t.parts()),
                None => e,
            })
        });
        // [ISO/IEC 9075:1992 §11.8 GR8] a referential action on a row a
        // cursor changed: a warning, about the cursor [§18.1 GR3e].
        let conflict = self.conflict.borrow_mut().take();
        // [ISO/IEC 9075:1992 §3.3.4.1] else one an expression raised.
        let raised = self.raised();
        let outcome = outcome.map(|done| {
            if conflict.is_some() && done == Completion::Success {
                Completion::Warning(SqlState::CURSOR_OPERATION_CONFLICT)
            } else {
                done.warned(raised)
            }
        });
        let diagnosed = self.diagnosed(outcome, |done| done.sqlstate());
        if let Some(cursor) = conflict {
            self.about_cursor(&cursor);
        }
        diagnosed
    }

    /// \[EXTENSION\] `ALTER TABLE table RESEAL`, of base table `table`
    /// written `AUTH.TABLE`: seals its rows again, through the encodings its
    /// option ENCODINGS names now (RULES.md D48): as ALTER TABLE changing its engine does,
    /// in this session's transaction, it gives the table a new generation
    /// of its storage, its rows copied there by the transaction's own
    /// writes, its indexes made anew. Once the transaction commits and a
    /// checkpoint follows, nothing of the table is read through the
    /// encodings, or the keys, it named before: a key may be retired.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if `table` is not written `AUTH.TABLE`; as ALTER
    /// TABLE's otherwise [ISO/IEC 9075:1992 §11.10]: SQLCODE -104 if the
    /// session's user does not own its schema, SQLCODE -103 if it is no
    /// base table, or a temporary one; the copy's own, as an encoding
    /// named that is not registered.
    pub fn reseal_table(&self, table: &str) -> Result<Completion> {
        let name = super::qualified_name(table)?;
        let call = self.enter()?;
        self.emptied(("ALTER TABLE", ""));
        let named = TableName { auth: Some(name.auth), ident: name.ident, at: 0 };
        let anew = AlterAction::Storage(TableStorage::default());
        let stmt = SqlStatement::AlterTable { table: named, action: anew, text: crate::parser::RESEAL.to_owned() };
        let outcome = self.execute_parsed(&call, &stmt, &mut Params::new(), None);
        self.diagnosed(outcome, |done| done.sqlstate())
    }

    /// `sql`, parsed with `params`: of a prepared statement, as it keeps
    /// it, while what it was parsed with holds (RULES.md D45).
    fn parsed(call: &Call<'_>, sql: &str, params: &Params, keeping: Option<&Keeping<'_>>) -> Result<Arc<SqlStatement>> {
        if let Some(kept) = keeping.map(Keeping::statement).transpose()?.flatten() {
            return Ok(kept);
        }
        params.validate()?;
        let stmt =
            Arc::new(params.parser(sql, call.state().settings.is_on(FUNCTION_CALLS), &call.state().catalog_name())?.sql_statement()?);
        if let Some(k) = keeping {
            k.keep_statement(Arc::clone(&stmt))?;
        }
        Ok(stmt)
    }

    /// `stmt`, parsed, run in the call `call`.
    fn execute_parsed(
        &self,
        call: &Call<'_>,
        stmt: &SqlStatement,
        params: &mut Params,
        keeping: Option<&Keeping<'_>>,
    ) -> Result<Completion> {
        let st = call.state();
        let tx = match stmt {
            SqlStatement::Commit => return self.db.noted(self.commit_in(call)).map(|()| Completion::Success),
            // [ISO/IEC 9075:1992 §4.22.7] not transaction initiating.
            SqlStatement::SetConstraints { names, deferred } => return self.set_constraints(call, names.as_deref(), *deferred),
            SqlStatement::Rollback => return self.db.noted(self.rollback_in(st)).map(|()| Completion::Success),
            // [ISO/IEC 9075:1992 §4.28, §4.30] not transaction initiating.
            SqlStatement::SetTransaction(modes) => return self.set_transaction(modes, params).map(|()| Completion::Success),
            SqlStatement::SetSchema(v) => return self.set_schema(st, v, params).map(|()| Completion::Success),
            SqlStatement::SetCatalog(v) => return self.set_catalog(st, v, params).map(|()| Completion::Success),
            // [ISO/IEC 9075:1992 §4.28] not transaction initiating.
            SqlStatement::SetSessionAuthorization(v) => return self.set_session_authorization(v, params).map(|()| Completion::Success),
            // [ISO/IEC 9075:1992 §15] the SQL-agent's, a client's.
            SqlStatement::Connection(_) => {
                return fail(
                    Code::Unsupported,
                    "[DESIGN] CONNECT, SET CONNECTION and DISCONNECT are the SQL-agent's: a client runs them, as turborel's does, not a session (RULES.md D46)",
                );
            }
            // [ISO/IEC 9075:1992 §4.28] not transaction initiating.
            SqlStatement::SetTimeZone(v) => return self.set_time_zone(st, v.as_ref(), params).map(|()| Completion::Success),
            SqlStatement::SetSetting { name, value } => {
                return self.set_named(st, name, value.as_ref(), params).map(|()| Completion::Success);
            }
            SqlStatement::SetDatabase { name, value } => {
                return self.set_database(st, name, value.as_ref(), params).map(|()| Completion::Success);
            }
            SqlStatement::Index(index) => return self.index_statement(st, index),
            SqlStatement::User(user) => return self.user_statement(user),
            SqlStatement::Served(served) => return self.served_statement(served),
            // [ISO/IEC 9075:1992 §4.28] not transaction initiating.
            SqlStatement::GetDiagnostics(get) => return self.get_diagnostics(st, get, params),
            // [ISO/IEC 9075:1992 §13.11 GR4] for the session, in no
            // transaction.
            SqlStatement::DeclareTable(t) => return self.declare_table(st, t),
            SqlStatement::Insert(_)
            | SqlStatement::UpdateSearched { .. }
            | SqlStatement::DeleteSearched { .. }
            | SqlStatement::UpdatePositioned { .. }
            | SqlStatement::DeletePositioned { .. }
            | SqlStatement::Open(_)
            | SqlStatement::Fetch { .. }
            | SqlStatement::Close(_)
            | SqlStatement::Select { .. }
            | SqlStatement::Drop { .. }
            | SqlStatement::AlterTable { .. }
            | SqlStatement::Schema { .. }
            // [ISO/IEC 9075:1992 §4.22.6] transaction initiating; EXECUTE
            // never reaches here (`execute_as`).
            | SqlStatement::Dynamic(_)
            | SqlStatement::Execute(_) => self.ensure_transaction(st),
        };
        call.runs(&tx);
        self.may_run(st, stmt)?;
        tx.begin_statement(|| !self.cursors_open())?;
        // [X3.135-1986 §7.3 GR3b i] a failed statement is undone; if that fails
        // too, the damage is the error reported, and it names the first one.
        let cx = StatementContext { st, tx: &tx };
        self.db.noted(self.run_statement(cx, stmt, params, keeping).map_err(|e| st.fail_statement(&tx, &e).err().unwrap_or(e)))
    }

    /// [X3.135-1986 §8.4, §8.11] The positioned DELETE or UPDATE of
    /// `table` giving `set`, bound as `kind`, run in `cx` by `run` through
    /// the cursor its `<cursor name>` names: a dynamic cursor's, of a
    /// statement prepared that names it [ISO/IEC 9075:1992 §17.6 GR2].
    fn positioned(
        &self,
        cx: StatementContext<'_>,
        (table, set): (&TableName, &[SetClause]),
        (kind, cursor, run): (WriteKind, &(String, usize), Positioned),
        params: &Params,
    ) -> Result<Completion> {
        let w = self.bound(cx.st, params, |b| bind_write(b, kind, table, set, None))?;
        self.settle_cursors(cx.st, &w.table);
        run(self, cx, (&self.positioned_key(&cursor.0), cursor.1), &w)
    }

    /// Runs a statement that runs in the transaction `tx`: not COMMIT,
    /// ROLLBACK or another that `execute_parsed` runs itself; of a prepared
    /// statement, with the plans it keeps.
    fn run_statement(
        &self,
        cx: StatementContext<'_>,
        stmt: &SqlStatement,
        params: &mut Params,
        keeping: Option<&Keeping<'_>>,
    ) -> Result<Completion> {
        let StatementContext { st, tx } = cx;
        match stmt {
            SqlStatement::Insert(ins) => self.insert_statement(cx, ins, params, keeping).map(|n| self.affected(n)),
            SqlStatement::UpdateSearched { table, set, where_clause } => {
                let bind = |b: Context<'_>| bind_write(b, WriteKind::UpdateSearched, table, set, where_clause.as_ref());
                let w = self.write_bound(st, params, keeping, bind)?;
                self.settle_cursors(st, &w.table);
                update_searched(cx, &w, self.effects(st)).map(|n| self.affected(n))
            }
            SqlStatement::DeleteSearched { table, where_clause } => {
                let bind = |b: Context<'_>| bind_write(b, WriteKind::DeleteSearched, table, &[], where_clause.as_ref());
                let w = self.write_bound(st, params, keeping, bind)?;
                self.settle_cursors(st, &w.table);
                delete_searched(cx, &w, self.effects(st)).map(|n| self.affected(n))
            }
            SqlStatement::UpdatePositioned { table, set, cursor } => {
                self.positioned(cx, (table, set), (WriteKind::UpdatePositioned, cursor, Self::update_positioned), params)
            }
            SqlStatement::DeletePositioned { table, cursor } => {
                self.positioned(cx, (table, &[]), (WriteKind::DeletePositioned, cursor, Self::delete_positioned), params)
            }
            SqlStatement::Open(name) => self.open(cx, &Name::plain(name), params),
            SqlStatement::Fetch { cursor, orientation, targets } => self.fetch(cx, &Name::plain(cursor), (orientation, targets), params),
            SqlStatement::Close(name) => self.close(&Name::plain(name)),
            SqlStatement::Dynamic(d) => self.run_dynamic(cx, d, params),
            SqlStatement::Select { query, targets } => self.select_into(cx, query, targets, params),
            SqlStatement::Drop { object, behavior } => self.drop_object(st, tx, object, *behavior),
            SqlStatement::AlterTable { table, action, text } => self.alter_table(st, tx, (table, action, text)),
            SqlStatement::Schema { statement, text } => self.schema_statement(st, tx, statement, text),
            // `execute_parsed` runs these itself, and never here.
            SqlStatement::Commit
            | SqlStatement::Rollback
            | SqlStatement::SetConstraints { .. }
            | SqlStatement::SetTransaction(_)
            | SqlStatement::SetSchema(_)
            | SqlStatement::SetCatalog(_)
            | SqlStatement::SetTimeZone(_)
            | SqlStatement::SetSessionAuthorization(_)
            | SqlStatement::Connection(_)
            | SqlStatement::SetSetting { .. }
            | SqlStatement::SetDatabase { .. }
            | SqlStatement::Index(_)
            | SqlStatement::User(_)
            | SqlStatement::Served(_)
            | SqlStatement::GetDiagnostics(_)
            | SqlStatement::Execute(_)
            | SqlStatement::DeclareTable(_) => Ok(Completion::Success),
        }
    }
}

/// [ISO/IEC 9075:1992 §18.1 GR3g] The table or view statement `stmt`
/// inserts into, changes, defines, alters, drops, indexes, or grants or
/// revokes privileges on: the table a syntax error or access rule
/// violation it raises is about, unless what raised it names another.
fn target_of(stmt: &SqlStatement) -> Option<&TableName> {
    match stmt {
        SqlStatement::Insert(ins) => Some(&ins.table),
        SqlStatement::UpdateSearched { table, .. }
        | SqlStatement::DeleteSearched { table, .. }
        | SqlStatement::UpdatePositioned { table, .. }
        | SqlStatement::DeletePositioned { table, .. }
        | SqlStatement::AlterTable { table, .. }
        | SqlStatement::Index(IndexStatement::Create { table, .. })
        | SqlStatement::Drop { object: DropObject::Table(table), .. }
        | SqlStatement::Drop { object: DropObject::View(table), .. }
        | SqlStatement::Schema { statement: SchemaStatement::Grant(PrivilegeDefinition { table, domain: false, .. }), .. }
        | SqlStatement::Schema { statement: SchemaStatement::Revoke(RevokeStatement { table, domain: false, .. }), .. } => Some(table),
        SqlStatement::Schema { statement: SchemaStatement::Table(t), .. } | SqlStatement::DeclareTable(t) => Some(&t.name),
        SqlStatement::Schema { statement: SchemaStatement::View(v), .. } => Some(&v.name),
        SqlStatement::Dynamic(d) => dynamic::changed_table(d).map(|(table, _)| table),
        SqlStatement::Schema { statement: SchemaStatement::Grant(_), .. }
        | SqlStatement::Schema { statement: SchemaStatement::Revoke(_), .. }
        | SqlStatement::Schema { statement: SchemaStatement::Domain(_), .. }
        | SqlStatement::Schema { statement: SchemaStatement::AlterDomain(_), .. }
        | SqlStatement::Schema { statement: SchemaStatement::Assertion(_), .. }
        | SqlStatement::Drop { .. }
        | SqlStatement::Index(_)
        | SqlStatement::Close(_)
        | SqlStatement::Commit
        | SqlStatement::Fetch { .. }
        | SqlStatement::GetDiagnostics(_)
        | SqlStatement::Open(_)
        | SqlStatement::Rollback
        | SqlStatement::Select { .. }
        | SqlStatement::SetTransaction(_)
        | SqlStatement::SetCatalog(_)
        | SqlStatement::SetSchema(_)
        | SqlStatement::SetTimeZone(_)
        | SqlStatement::SetSessionAuthorization(_)
        | SqlStatement::Connection(_)
        | SqlStatement::Served(_)
        | SqlStatement::SetSetting { .. }
        | SqlStatement::SetDatabase { .. }
        | SqlStatement::User(_)
        | SqlStatement::Execute(_)
        | SqlStatement::SetConstraints { .. } => None,
    }
}

/// [ISO/IEC 9075:1992 §18.1 GR1d, Table 22] How DYNAMIC_FUNCTION names
/// `stmt`, prepared: a positioned DELETE or UPDATE prepared is a
/// `<preparable dynamic delete statement: positioned>` or `<preparable
/// dynamic update statement: positioned>` [§17.6, §17.19, §17.20]; any
/// other as COMMAND_FUNCTION names it.
fn prepared_command(stmt: &SqlStatement) -> &'static str {
    if let SqlStatement::DeletePositioned { .. } = stmt {
        return "DYNAMIC DELETE CURSOR";
    }
    if let SqlStatement::UpdatePositioned { .. } = stmt {
        return "DYNAMIC UPDATE CURSOR";
    }
    command_of(stmt)
}

/// Whether `sql` begins as a `<get diagnostics statement>` does.
fn diagnostics_statement(sql: &str) -> bool {
    let tokens = lex(sql).unwrap_or_default();
    matches!(tokens.as_slice(), [Token { tok: Tok::KeyWord(kw::GET), .. }, Token { tok: Tok::KeyWord(kw::DIAGNOSTICS), .. }, ..])
}

/// [ISO/IEC 9075:1992 §18.1 GR1c, Table 22] How COMMAND_FUNCTION names
/// `stmt`; \[EXTENSION\] a statement of this implementation by its first
/// words.
fn command_of(stmt: &SqlStatement) -> &'static str {
    match stmt {
        SqlStatement::Close(_) => "CLOSE CURSOR",
        SqlStatement::Commit => "COMMIT WORK",
        SqlStatement::DeletePositioned { .. } => "DELETE CURSOR",
        SqlStatement::DeleteSearched { .. } => "DELETE WHERE",
        SqlStatement::AlterTable { .. } => "ALTER TABLE",
        SqlStatement::Drop { object: DropObject::Table(_), .. } => "DROP TABLE",
        SqlStatement::Drop { object: DropObject::View(_), .. } => "DROP VIEW",
        SqlStatement::Drop { object: DropObject::Schema { .. }, .. } => "DROP SCHEMA",
        SqlStatement::Drop { object: DropObject::Domain(_), .. } => "DROP DOMAIN",
        SqlStatement::Drop { object: DropObject::Assertion(_), .. } => "DROP ASSERTION",
        SqlStatement::Schema { statement: SchemaStatement::Assertion(_), .. } => "CREATE ASSERTION",
        SqlStatement::Schema { statement: SchemaStatement::Domain(_), .. } => "CREATE DOMAIN",
        SqlStatement::Schema { statement: SchemaStatement::AlterDomain(_), .. } => "ALTER DOMAIN",
        SqlStatement::Schema { statement: SchemaStatement::Table(_), .. } => "CREATE TABLE",
        SqlStatement::Schema { statement: SchemaStatement::View(_), .. } => "CREATE VIEW",
        SqlStatement::Schema { statement: SchemaStatement::Grant(_), .. } => "GRANT",
        SqlStatement::Schema { statement: SchemaStatement::Revoke(_), .. } => "REVOKE",
        SqlStatement::Fetch { .. } => "FETCH",
        SqlStatement::GetDiagnostics(_) => "GET DIAGNOSTICS",
        SqlStatement::Dynamic(d) => dynamic::command(d),
        SqlStatement::Execute(e) => dynamic::execute_command(e),
        SqlStatement::DeclareTable(_) => "DECLARE LOCAL TEMPORARY TABLE",
        SqlStatement::Insert(_) => "INSERT",
        SqlStatement::Open(_) => "OPEN",
        SqlStatement::Rollback => "ROLLBACK WORK",
        SqlStatement::Select { .. } => "SELECT",
        SqlStatement::SetTransaction(_) => "SET TRANSACTION",
        SqlStatement::SetSchema(_) => "SET SCHEMA",
        SqlStatement::SetCatalog(_) => "SET CATALOG",
        SqlStatement::SetTimeZone(_) => "SET TIME ZONE",
        SqlStatement::SetSessionAuthorization(_) => "SET SESSION AUTHORIZATION",
        SqlStatement::Connection(ConnectionStatement::Connect { .. } | ConnectionStatement::ConnectDefault) => "CONNECT",
        SqlStatement::Connection(ConnectionStatement::Set(_)) => "SET CONNECTION",
        SqlStatement::Connection(ConnectionStatement::Disconnect(_)) => "DISCONNECT",
        SqlStatement::SetConstraints { .. } => "SET CONSTRAINT",
        SqlStatement::UpdatePositioned { .. } => "UPDATE CURSOR",
        SqlStatement::UpdateSearched { .. } => "UPDATE WHERE",
        SqlStatement::SetSetting { .. } => "SET",
        SqlStatement::SetDatabase { .. } => "SET DATABASE",
        SqlStatement::Index(IndexStatement::Create { .. }) => "CREATE INDEX",
        SqlStatement::Index(IndexStatement::Drop { .. }) => "DROP INDEX",
        SqlStatement::Index(IndexStatement::Alter { .. }) => "ALTER INDEX",
        SqlStatement::User(UserStatement::Create { .. }) => "CREATE USER",
        SqlStatement::User(UserStatement::Alter { .. }) => "ALTER USER",
        SqlStatement::User(UserStatement::Drop { .. }) => "DROP USER",
        SqlStatement::User(UserStatement::Owner { .. }) => "ALTER DATABASE",
        SqlStatement::Served(ServedStatement::End { .. }) => "END CONNECTION",
        SqlStatement::Served(ServedStatement::Cancel { .. }) => "CANCEL CONNECTION",
    }
}

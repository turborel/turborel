//! A session's statements run [X3.135-1986 §7.3]: each parsed, run in its
//! transaction, undone if it fails, and its condition placed in the
//! diagnostics area (`db/area.rs`). Split from db.rs (RULES.md R-SIZE-2).

use std::sync::Arc;

use super::write::{delete_searched, update_searched};
use super::{Call, Completion, Session, State, StatementContext, slow};
use crate::ast::{
    ConnectionStatement, DropObject, IndexStatement, PrivilegeDefinition, RevokeStatement, SchemaStatement, ServedStatement, SqlStatement,
    TableName, UserStatement,
};
use crate::binder::{WriteKind, bind_write, qualify};
use crate::error::{Code, Result, SqlState, fail};
use crate::lexer::{Tok, Token, kw, lex};
use crate::params::Params;
use crate::txn::Txn;

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
        self.execute_as(sql, params, false)
    }

    /// Runs an `<SQL statement>` with no parameters.
    ///
    /// # Errors
    ///
    /// As for `execute`.
    pub fn run(&self, sql: &str) -> Result<Completion> {
        self.execute(sql, &mut Params::new())
    }

    /// `execute`, of a statement `prepared` (`execute_prepared`): the
    /// diagnostics area then names EXECUTE, and the statement as its
    /// DYNAMIC_FUNCTION [ISO/IEC 9075:1992 §18.1 GR1c, GR1d].
    pub(super) fn execute_as(&self, sql: &str, params: &mut Params, prepared: bool) -> Result<Completion> {
        let mut call = self.enter()?;
        call.timed = slow::Timed::of(call.state(), sql, &self.auth());
        let parsed = params.validate().and_then(|()| params.parser(sql, self.extensions, &call.state().catalog_name())?.sql_statement());
        // [ISO/IEC 9075:1992 §4.19] GET DIAGNOSTICS reads the area as the
        // statement before left it, and leaves it so, refused too.
        if let Ok(SqlStatement::GetDiagnostics(get)) = &parsed {
            return self.get_diagnostics(call.state(), get, params);
        }
        if parsed.is_err() && diagnostics_statement(sql) {
            return parsed.map(|_| Completion::Success);
        }
        let command = parsed.as_ref().map_or("", command_of);
        self.emptied(if prepared { ("EXECUTE", command) } else { (command, "") });
        let outcome = parsed.and_then(|stmt| {
            let target = target_of(&stmt).map(|t| qualify(t, &self.default_schema()));
            self.execute_parsed(&call, stmt, params).map_err(|e| match target {
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

    /// `stmt`, parsed, run in the call `call`.
    fn execute_parsed(&self, call: &Call<'_>, stmt: SqlStatement, params: &mut Params) -> Result<Completion> {
        let st = call.state();
        let tx = match stmt {
            SqlStatement::Commit => return self.db.noted(self.commit_in(call)).map(|()| Completion::Success),
            // [ISO/IEC 9075:1992 §4.22.7] not transaction initiating.
            SqlStatement::SetConstraints { names, deferred } => return self.set_constraints(call, names.as_deref(), deferred),
            SqlStatement::Rollback => return self.db.noted(self.rollback_in(st)).map(|()| Completion::Success),
            // [ISO/IEC 9075:1992 §4.28, §4.30] not transaction initiating.
            SqlStatement::SetTransaction(modes) => return self.set_transaction(&modes, params).map(|()| Completion::Success),
            SqlStatement::SetSchema(v) => return self.set_schema(st, &v, params).map(|()| Completion::Success),
            SqlStatement::SetCatalog(v) => return self.set_catalog(st, &v, params).map(|()| Completion::Success),
            // [ISO/IEC 9075:1992 §4.28] not transaction initiating.
            SqlStatement::SetSessionAuthorization(v) => return self.set_session_authorization(&v, params).map(|()| Completion::Success),
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
                return self.set_named(st, &name, value.as_ref(), params).map(|()| Completion::Success);
            }
            SqlStatement::SetDatabase { name, value } => {
                return self.set_database(st, &name, value.as_ref(), params).map(|()| Completion::Success);
            }
            SqlStatement::Index(index) => return self.index_statement(st, &index),
            SqlStatement::User(user) => return self.user_statement(&user),
            SqlStatement::Served(served) => return self.served_statement(&served),
            // [ISO/IEC 9075:1992 §4.28] not transaction initiating.
            SqlStatement::GetDiagnostics(get) => return self.get_diagnostics(st, &get, params),
            // [ISO/IEC 9075:1992 §13.11 GR4] for the session, in no
            // transaction.
            SqlStatement::DeclareTable(t) => return self.declare_table(st, &t),
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
            | SqlStatement::Schema { .. } => self.ensure_transaction(st),
        };
        call.runs(&tx);
        self.may_run(st, &stmt)?;
        tx.begin_statement(|| !self.cursors_open())?;
        // [X3.135-1986 §7.3 GR3b i] a failed statement is undone; if that fails
        // too, the damage is the error reported, and it names the first one.
        self.db.noted(self.run_statement(st, &tx, stmt, params).map_err(|e| st.fail_statement(&tx, &e).err().unwrap_or(e)))
    }

    /// Runs a statement that runs in the transaction `tx`: not COMMIT,
    /// ROLLBACK or another that `execute_parsed` runs itself.
    fn run_statement(&self, st: &Arc<State>, tx: &Txn, stmt: SqlStatement, params: &mut Params) -> Result<Completion> {
        match stmt {
            SqlStatement::Insert(ins) => self.insert_statement(st, tx, &ins, params).map(|n| self.affected(n)),
            SqlStatement::UpdateSearched { table, set, where_clause } => {
                let w = self.bound(st, params, |cx| bind_write(cx, WriteKind::UpdateSearched, &table, &set, where_clause.as_ref()))?;
                self.settle_cursors(st, &w.table);
                update_searched(StatementContext { st, tx }, &w, self.effects(st)).map(|n| self.affected(n))
            }
            SqlStatement::DeleteSearched { table, where_clause } => {
                let w = self.bound(st, params, |cx| bind_write(cx, WriteKind::DeleteSearched, &table, &[], where_clause.as_ref()))?;
                self.settle_cursors(st, &w.table);
                delete_searched(StatementContext { st, tx }, &w, self.effects(st)).map(|n| self.affected(n))
            }
            SqlStatement::UpdatePositioned { table, set, cursor } => {
                let w = self.bound(st, params, |cx| bind_write(cx, WriteKind::UpdatePositioned, &table, &set, None))?;
                self.settle_cursors(st, &w.table);
                self.update_positioned(StatementContext { st, tx }, (&cursor.0, cursor.1), &w)
            }
            SqlStatement::DeletePositioned { table, cursor } => {
                let w = self.bound(st, params, |cx| bind_write(cx, WriteKind::DeletePositioned, &table, &[], None))?;
                self.settle_cursors(st, &w.table);
                self.delete_positioned(StatementContext { st, tx }, (&cursor.0, cursor.1), &w)
            }
            SqlStatement::Open(name) => self.open(st, tx, &name, params),
            SqlStatement::Fetch { cursor, orientation, targets } => {
                self.fetch(StatementContext { st, tx }, &cursor, (&orientation, &targets), params)
            }
            SqlStatement::Close(name) => self.close(&name),
            SqlStatement::Select { query, targets } => self.select_into(StatementContext { st, tx }, &query, &targets, params),
            SqlStatement::Drop { object, behavior } => self.drop_object(st, tx, &object, behavior),
            SqlStatement::AlterTable { table, action, text } => self.alter_table(st, tx, (&table, &action, &text)),
            SqlStatement::Schema { statement, text } => self.schema_statement(st, tx, &statement, &text),
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
        | SqlStatement::SetConstraints { .. } => None,
    }
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
        SqlStatement::Served(ServedStatement::End { .. }) => "END CONNECTION",
        SqlStatement::Served(ServedStatement::Cancel { .. }) => "CANCEL CONNECTION",
    }
}

//! A session's statements run [X3.135-1986 §7.3]: each parsed, run in its
//! transaction, undone if it fails, and its condition placed in the
//! diagnostics area (`db/area.rs`).

use std::sync::Arc;

use super::area::commands::{self, Command};
use super::dynamic::{self, Name};
use super::integrity::Effects;
use super::prepared::{Keeping, Plans};
use super::triggers::{Event, instead_for};
use super::write::{Written, delete_searched, update_searched};
use super::{Call, Completion, Session, StatementContext, slow};
use crate::ast::{
    AlterAction, ConnectionStatement, DropObject, IndexStatement, PrivilegeDefinition, PrivilegeObject, RevokeStatement, RoleStatement,
    SchemaStatement, SearchCondition, ServedStatement, SetClause, SqlStatement, TableName, TableStorage, UserStatement,
};
use crate::binder::{Context, WriteKind, bind_write, qualify};
use crate::error::{Code, Result, SqlError, SqlState, fail};
use crate::lexer::{Tok, Token, kw, lex};
use crate::params::Params;
use crate::settings::FUNCTION_CALLS;

/// A positioned DELETE or UPDATE run through a cursor (`Session::positioned`).
type Positioned = fn(&Session, StatementContext<'_>, (&Name, usize), &Written) -> Result<Completion>;

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
    pub(super) fn execute_as(&self, sql: &str, params: &mut Params, prepared: Option<(Command, &Plans)>) -> Result<Completion> {
        let mut call = self.enter()?;
        call.timed = slow::Timed::of(call.state(), || sql, &self.auth());
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
        let (command, function) =
            parsed.as_deref().map_or((commands::UNRECOGNIZED, commands::UNRECOGNIZED), |stmt| (command_of(stmt), prepared_command(stmt)));
        // [ISO/IEC 9075:1992 §17.10, §17.11] EXECUTE runs the statement
        // prepared as a statement of its own.
        if let Ok(SqlStatement::Execute(e)) = parsed.as_deref() {
            drop(call);
            return self.execute_dynamic(e, command, params);
        }
        self.emptied(prepared.map_or((command, commands::UNRECOGNIZED), |(outer, _)| (outer, function)));
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
        self.emptied((commands::ALTER_TABLE, commands::UNRECOGNIZED));
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
        let done = |r: Result<()>| r.map(|()| Completion::Success);
        let tx = match stmt {
            SqlStatement::Commit => return done(self.db.noted(self.commit_in(st))),
            // [ISO/IEC 9075:1992 §4.22.7] not transaction initiating.
            SqlStatement::SetConstraints { names, deferred } => return self.set_constraints(call, names.as_deref(), *deferred),
            SqlStatement::Rollback => return done(self.db.noted(self.rollback_in(st))),
            // [ISO/IEC 9075:1992 §4.28, §4.30] not transaction initiating.
            SqlStatement::SetTransaction(modes) => return done(self.set_transaction(modes, params)),
            SqlStatement::SetSchema(v) => return done(self.set_schema(st, v, params)),
            SqlStatement::SetCollation(v) => return done(self.set_collation(st, v.as_ref(), params)),
            SqlStatement::SetCatalog(v) => return done(self.set_catalog(st, v, params)),
            // [ISO/IEC 9075:1992 §4.28] not transaction initiating.
            SqlStatement::SetSessionAuthorization(v) => return done(self.set_session_authorization(v, params)),
            // [ISO/IEC 9075-2:2023 §19.3 GR1] in no transaction, so begins none.
            SqlStatement::SetRole(role) => return done(self.set_role(role, params)),
            // [ISO/IEC 9075:1992 §15] the SQL-agent's, a client's.
            SqlStatement::Connection(_) => return fail(Code::Unsupported, AGENTS_CONNECTIONS),
            // [ISO/IEC 9075:1992 §4.28] not transaction initiating.
            SqlStatement::SetTimeZone(v) => return done(self.set_time_zone(st, v.as_ref(), params)),
            SqlStatement::SetSetting { name, value } => return done(self.set_named(st, name, value.as_ref(), params)),
            SqlStatement::SetDatabase { name, value } => return done(self.set_database(st, name, value.as_ref(), params)),
            SqlStatement::Index(index) => return self.index_statement(st, index),
            SqlStatement::User(user) => return self.user_statement(user),
            SqlStatement::Served(served) => return self.served_statement(served),
            // [ISO/IEC 9075:1992 §4.28] not transaction initiating.
            SqlStatement::GetDiagnostics(get) => return self.get_diagnostics(st, get, params),
            // [ISO/IEC 9075:1992 §13.11 GR4] for the session, in no
            // transaction.
            SqlStatement::DeclareTable(t) => return self.declare_table(st, t),
            SqlStatement::Insert(_)
            | SqlStatement::Merge(_)
            | SqlStatement::UpdateSearched { .. }
            | SqlStatement::DeleteSearched { .. }
            | SqlStatement::UpdatePositioned { .. }
            | SqlStatement::DeletePositioned { .. }
            | SqlStatement::Truncate { .. }
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
            | SqlStatement::Execute(_)
            | SqlStatement::Call(_)
            | SqlStatement::Return(_)
            | SqlStatement::Control(_)
            | SqlStatement::Signal(_) => self.ensure_transaction(st),
        };
        call.runs(&tx);
        self.may_run(st, stmt)?;
        tx.begin_statement(|| !self.cursors_open())?;
        let cx = StatementContext { st, tx: &tx };
        let ran = self.run_statement(cx, stmt, params, keeping);
        self.db.noted(settled(cx, stmt, ran))
    }

    /// [X3.135-1986 §8.4, §8.11] The positioned DELETE or UPDATE of
    /// `table` giving `set`, bound as `kind`, run in `cx` by `run` through
    /// the cursor its `<cursor name>` names: a dynamic cursor's, of a
    /// statement prepared that names it [ISO/IEC 9075:1992 §17.6 GR2]; of
    /// a prepared statement, bound as it keeps it (`keeping`, RULES.md D45).
    /// The positioned UPDATE or DELETE `stmt`, run as `positioned` runs it.
    fn positioned_statement(
        &self,
        cx: StatementContext<'_>,
        stmt: &SqlStatement,
        by: (&Params, Option<&Keeping<'_>>),
    ) -> Result<Completion> {
        if let SqlStatement::UpdatePositioned { table, set, cursor } = stmt {
            return self.positioned(cx, (table, set), (WriteKind::UpdatePositioned, cursor, Self::update_positioned), by);
        }
        if let SqlStatement::DeletePositioned { table, cursor } = stmt {
            return self.positioned(cx, (table, &[]), (WriteKind::DeletePositioned, cursor, Self::delete_positioned), by);
        }
        Err(SqlError::new(Code::Api, "[DESIGN] a positioned statement is an UPDATE or a DELETE"))
    }

    fn positioned(
        &self,
        cx: StatementContext<'_>,
        (table, set): (&TableName, &[SetClause]),
        (kind, cursor, run): (WriteKind, &(String, usize), Positioned),
        (params, keeping): (&Params, Option<&Keeping<'_>>),
    ) -> Result<Completion> {
        let w = self.write_bound(cx.st, params, keeping, |b| bind_write(b, kind, table, set, None))?;
        let local = params.local_cursor(&cursor.0).map(|key| Name::plain(&key));
        let at = (&local.unwrap_or_else(|| self.positioned_key(&cursor.0)), cursor.1);
        // [ISO/IEC 9075-2:2023 §15.6 GR11b, §15.7 GR17b] the INSTEAD OF
        // trigger of a view the cursor's row is of, in place of the change.
        if let Some(instead) = instead_for(cx.st, &w.bound.named, event_of(kind)) {
            let clause = if event_of(kind) == Event::Delete { "§8.4" } else { "§8.11" };
            return self.positioned_instead(cx, at, (&w, &instead, clause));
        }
        self.settle_cursors(cx.st, &w.bound.table);
        run(self, cx, at, &w)
    }

    /// A searched UPDATE or DELETE, `kind`, of `table` giving `set` where
    /// `where_clause` is true, bound and run in `cx` by `run`; of a
    /// prepared statement, bound as it keeps it (`keeping`, RULES.md D45).
    fn searched(
        &self,
        cx: StatementContext<'_>,
        (kind, table, set, where_clause): (WriteKind, &TableName, &[SetClause], Option<&SearchCondition>),
        run: fn(StatementContext<'_>, Written, Effects<'_>) -> Result<usize>,
        (params, keeping): (&Params, Option<&Keeping<'_>>),
    ) -> Result<Completion> {
        let bind = |b: Context<'_>| bind_write(b, kind, table, set, where_clause);
        let w = self.write_bound(cx.st, params, keeping, bind)?;
        // [ISO/IEC 9075-2:2023 §15.15 GR3c iv 2] the INSTEAD OF trigger of a
        // view under the one named, in place of the change; one of the view
        // named took it already (`instead_of`).
        if let Some(instead) = instead_for(cx.st, &w.bound.named, event_of(kind)) {
            return self.searched_instead(cx, w, &instead).map(|n| self.affected(n));
        }
        self.settle_cursors(cx.st, &w.bound.table);
        run(cx, w, self.effects(cx.st)).map(|n| self.affected(n))
    }

    /// Runs a statement that runs in the transaction `tx`: not COMMIT,
    /// ROLLBACK or another that `execute_parsed` runs itself; of a prepared
    /// statement, with the plans it keeps.
    pub(super) fn run_statement(
        &self,
        cx: StatementContext<'_>,
        stmt: &SqlStatement,
        params: &mut Params,
        keeping: Option<&Keeping<'_>>,
    ) -> Result<Completion> {
        let StatementContext { st, tx } = cx;
        // [ISO/IEC 9075-2:2023 §15.10, §15.13, §15.16] a view's INSTEAD OF
        // trigger in place of the change.
        if let Some(n) = self.instead_of(cx, stmt, params)? {
            return Ok(self.affected(n));
        }
        match stmt {
            SqlStatement::Insert(ins) => self.insert_statement(cx, ins, params, keeping).map(|n| self.affected(n)),
            SqlStatement::Merge(merge) => self.merge_completed(cx, merge, params, keeping),
            SqlStatement::UpdateSearched { table, set, where_clause } => {
                self.searched(cx, (WriteKind::UpdateSearched, table, set, where_clause.as_ref()), update_searched, (&*params, keeping))
            }
            SqlStatement::DeleteSearched { table, where_clause } => {
                self.searched(cx, (WriteKind::DeleteSearched, table, &[], where_clause.as_ref()), delete_searched, (&*params, keeping))
            }
            SqlStatement::UpdatePositioned { .. } | SqlStatement::DeletePositioned { .. } => {
                self.positioned_statement(cx, stmt, (params, keeping))
            }
            SqlStatement::Truncate { table, restart } => self.truncate_table(cx, (table, *restart), params),
            SqlStatement::Open(name) => self.open(cx, &cursor_named(name, params), params),
            SqlStatement::Fetch { cursor, orientation, targets } => {
                self.fetch(cx, &cursor_named(cursor, params), (orientation, targets), params)
            }
            SqlStatement::Close(name) => self.close(&cursor_named(name, params)),
            SqlStatement::Dynamic(d) => self.run_dynamic(cx, d, params),
            SqlStatement::Select { query, targets } => self.select_into(cx, query, targets, params),
            SqlStatement::Drop { object, behavior } => self.drop_object(st, tx, object, *behavior),
            SqlStatement::AlterTable { table, action, text } => self.alter_table(st, tx, (table, action, text)),
            SqlStatement::Schema { statement, text } => self.schema_statement(st, tx, statement, text),
            SqlStatement::Call(call) => self.call_statement(cx, call, params),
            SqlStatement::Control(_) | SqlStatement::Signal(_) => self.control_statement(cx, stmt, params),
            SqlStatement::Return(r) => super::routines::return_statement(r),
            // `execute_parsed` runs these itself, and never here.
            SqlStatement::Commit
            | SqlStatement::Rollback
            | SqlStatement::SetConstraints { .. }
            | SqlStatement::SetTransaction(_)
            | SqlStatement::SetSchema(_)
            | SqlStatement::SetCollation(_)
            | SqlStatement::SetCatalog(_)
            | SqlStatement::SetTimeZone(_)
            | SqlStatement::SetSessionAuthorization(_)
            | SqlStatement::SetRole(_)
            | SqlStatement::Connection(_)
            | SqlStatement::SetSetting { .. }
            | SqlStatement::SetDatabase { .. }
            | SqlStatement::Index(_)
            | SqlStatement::User(_)
            | SqlStatement::Served(_)
            | SqlStatement::Execute(_)
            | SqlStatement::DeclareTable(_) => Ok(Completion::Success),
            // [ISO/IEC 9075-2:2023 §15.21 GR6a, §9.18 GR6f] of a triggered
            // action's or a procedure body's own diagnostics area.
            SqlStatement::GetDiagnostics(get) => self.get_diagnostics(st, get, params),
        }
    }
}

/// [ISO/IEC 9075:1992 §15] Why a session runs no connection statement.
const AGENTS_CONNECTIONS: &str = "[DESIGN] CONNECT, SET CONNECTION and DISCONNECT are the SQL-agent's: a client runs them, as turborel's does, not a session (RULES.md D46)";

/// [X3.135-1986 §7.3 GR3b i] `ran`, of `stmt` in `cx`, a failure undone;
/// if that fails too, the damage is the error reported, and it names the
/// first one. [ISO/IEC
/// 9075-2:2023 §4.41.5; ISO/IEC 9075-4:2006 draft §4.10.4] Of a statement
/// that is not atomic, each statement within is undone alone as it fails,
/// so what those before it did stays.
fn settled(cx: StatementContext<'_>, stmt: &SqlStatement, ran: Result<Completion>) -> Result<Completion> {
    if !super::routines::atomic(stmt) {
        return ran;
    }
    ran.map_err(|e| cx.st.fail_statement(cx.tx, &e).err().unwrap_or(e))
}

/// [ISO/IEC 9075-4:2006 draft §14.1 SR15] The session's name of the cursor
/// `name` names where `params` are: of a local cursor of the compound
/// statement the statement is in, its own.
fn cursor_named(name: &str, params: &Params) -> Name {
    params.local_cursor(name).map_or_else(|| Name::plain(name), |key| Name::plain(&key))
}

/// The trigger event of a change of kind `kind` [ISO/IEC 9075-2:2023
/// §4.46.2].
fn event_of(kind: WriteKind) -> Event {
    match kind {
        WriteKind::UpdateSearched | WriteKind::UpdatePositioned => Event::Update,
        WriteKind::DeleteSearched | WriteKind::DeletePositioned => Event::Delete,
    }
}

/// [ISO/IEC 9075:1992 §18.1 GR3g] The table or view statement `stmt`
/// inserts into, changes, defines, alters, drops, indexes, or grants or
/// revokes privileges on: the table a syntax error or access rule
/// violation it raises is about, unless what raised it names another.
fn target_of(stmt: &SqlStatement) -> Option<&TableName> {
    match stmt {
        SqlStatement::Insert(ins) => Some(&ins.table),
        SqlStatement::Merge(merge) => Some(&merge.target),
        SqlStatement::UpdateSearched { table, .. }
        | SqlStatement::DeleteSearched { table, .. }
        | SqlStatement::UpdatePositioned { table, .. }
        | SqlStatement::DeletePositioned { table, .. }
        | SqlStatement::Truncate { table, .. }
        | SqlStatement::AlterTable { table, .. }
        | SqlStatement::Index(IndexStatement::Create { table, .. })
        | SqlStatement::Drop { object: DropObject::Table(table), .. }
        | SqlStatement::Drop { object: DropObject::View(table), .. } => Some(table),
        SqlStatement::Schema { statement, .. } => schema_target(statement),
        SqlStatement::DeclareTable(t) => Some(&t.name),
        SqlStatement::Dynamic(d) => dynamic::changed_table(d).map(|(table, _)| table),
        SqlStatement::Drop { .. }
        | SqlStatement::Control(_)
        | SqlStatement::Signal(_)
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
        | SqlStatement::SetCollation(_)
        | SqlStatement::SetTimeZone(_)
        | SqlStatement::SetSessionAuthorization(_)
        | SqlStatement::SetRole(_)
        | SqlStatement::Connection(_)
        | SqlStatement::Served(_)
        | SqlStatement::SetSetting { .. }
        | SqlStatement::SetDatabase { .. }
        | SqlStatement::User(_)
        | SqlStatement::Execute(_)
        | SqlStatement::Call(_)
        | SqlStatement::Return(_)
        | SqlStatement::SetConstraints { .. } => None,
    }
}

/// The table or view schema statement `s` defines, or grants or revokes
/// privileges on (`target_of`).
fn schema_target(s: &SchemaStatement) -> Option<&TableName> {
    match s {
        SchemaStatement::Grant(PrivilegeDefinition { table, object: PrivilegeObject::Table, .. })
        | SchemaStatement::Revoke(RevokeStatement { table, object: PrivilegeObject::Table, .. }) => Some(table),
        SchemaStatement::Table(t) => Some(&t.name),
        SchemaStatement::View(v) => Some(&v.name),
        SchemaStatement::Trigger(t) => Some(&t.table),
        SchemaStatement::Grant(_)
        | SchemaStatement::Revoke(_)
        | SchemaStatement::Domain(_)
        | SchemaStatement::AlterDomain(_)
        | SchemaStatement::Assertion(_)
        | SchemaStatement::Collation(_)
        | SchemaStatement::Sequence(_)
        | SchemaStatement::AlterSequence(_)
        | SchemaStatement::Type(_)
        | SchemaStatement::Routine(_)
        | SchemaStatement::Module(_)
        | SchemaStatement::Role(_) => None,
    }
}

/// [ISO/IEC 9075:1992 §18.1 GR1d, Table 22] How DYNAMIC_FUNCTION names
/// `stmt`, prepared: a positioned DELETE or UPDATE prepared is a
/// `<preparable dynamic delete statement: positioned>` or `<preparable
/// dynamic update statement: positioned>` [§17.6, §17.19, §17.20]; any
/// other as COMMAND_FUNCTION names it.
fn prepared_command(stmt: &SqlStatement) -> Command {
    if let SqlStatement::DeletePositioned { .. } = stmt {
        return commands::PREPARABLE_DELETE_CURSOR;
    }
    if let SqlStatement::UpdatePositioned { .. } = stmt {
        return commands::PREPARABLE_UPDATE_CURSOR;
    }
    command_of(stmt)
}

/// Whether `sql` begins as a `<get diagnostics statement>` does.
fn diagnostics_statement(sql: &str) -> bool {
    let tokens = lex(sql).unwrap_or_default();
    matches!(tokens.as_slice(), [Token { tok: Tok::KeyWord(kw::GET), .. }, Token { tok: Tok::KeyWord(kw::DIAGNOSTICS), .. }, ..])
}

/// How COMMAND_FUNCTION names schema statement `s` (`command_of`).
fn schema_command(s: &SchemaStatement) -> Command {
    match s {
        SchemaStatement::Assertion(_) => commands::CREATE_ASSERTION,
        SchemaStatement::Collation(_) => commands::CREATE_COLLATION,
        SchemaStatement::Domain(_) => commands::CREATE_DOMAIN,
        SchemaStatement::AlterDomain(_) => commands::ALTER_DOMAIN,
        SchemaStatement::Table(_) => commands::CREATE_TABLE,
        SchemaStatement::View(_) => commands::CREATE_VIEW,
        SchemaStatement::Grant(_) => commands::GRANT,
        SchemaStatement::Revoke(_) => commands::REVOKE,
        SchemaStatement::Role(RoleStatement::Create { .. }) => commands::CREATE_ROLE,
        SchemaStatement::Role(RoleStatement::Grant { .. }) => commands::GRANT_ROLE,
        SchemaStatement::Role(RoleStatement::Revoke { .. }) => commands::REVOKE_ROLE,
        SchemaStatement::Role(RoleStatement::Drop { .. }) => commands::DROP_ROLE,
        SchemaStatement::Sequence(_) => commands::CREATE_SEQUENCE,
        SchemaStatement::AlterSequence(_) => commands::ALTER_SEQUENCE,
        SchemaStatement::Type(_) => commands::CREATE_TYPE,
        SchemaStatement::Trigger(_) => commands::CREATE_TRIGGER,
        SchemaStatement::Routine(_) => commands::CREATE_ROUTINE,
        SchemaStatement::Module(_) => commands::CREATE_MODULE,
    }
}

/// [ISO/IEC 9075-2:2023 §23.1 GR2c, GR2d, Table 39] How COMMAND_FUNCTION
/// and COMMAND_FUNCTION_CODE name the drop of `object`.
fn drop_command(object: &DropObject) -> Command {
    match object {
        DropObject::Table(_) => commands::DROP_TABLE,
        DropObject::View(_) => commands::DROP_VIEW,
        DropObject::Schema { .. } => commands::DROP_SCHEMA,
        DropObject::Domain(_) => commands::DROP_DOMAIN,
        DropObject::Assertion(_) => commands::DROP_ASSERTION,
        DropObject::Collation(_) => commands::DROP_COLLATION,
        DropObject::Sequence(_) => commands::DROP_SEQUENCE,
        DropObject::Type(_) => commands::DROP_TYPE,
        DropObject::Trigger(_) => commands::DROP_TRIGGER,
        DropObject::Routine(_) => commands::DROP_ROUTINE,
        DropObject::Module(_) => commands::DROP_MODULE,
    }
}

/// [ISO/IEC 9075:1992 §18.1 GR1c, Table 22; ISO/IEC 9075-2:2023 §23.1
/// GR2c, GR2d, Table 39] How COMMAND_FUNCTION and COMMAND_FUNCTION_CODE
/// name `stmt`.
pub(in crate::db) fn command_of(stmt: &SqlStatement) -> Command {
    match stmt {
        SqlStatement::Control(c) => commands::control(c),
        SqlStatement::Signal(s) => commands::signal(s.resignal),
        SqlStatement::Close(_) => commands::CLOSE_CURSOR,
        SqlStatement::Schema { statement, .. } => schema_command(statement),
        SqlStatement::Commit => commands::COMMIT_WORK,
        SqlStatement::DeletePositioned { .. } => commands::DELETE_CURSOR,
        SqlStatement::DeleteSearched { .. } => commands::DELETE_WHERE,
        SqlStatement::Truncate { .. } => commands::TRUNCATE_TABLE,
        SqlStatement::AlterTable { .. } => commands::ALTER_TABLE,
        SqlStatement::Drop { object, .. } => drop_command(object),
        SqlStatement::Fetch { .. } => commands::FETCH,
        SqlStatement::GetDiagnostics(_) => commands::GET_DIAGNOSTICS,
        SqlStatement::Dynamic(d) => dynamic::command(d),
        SqlStatement::Execute(e) => dynamic::execute_command(e),
        SqlStatement::DeclareTable(_) => commands::DECLARE_TABLE,
        SqlStatement::Insert(_) => commands::INSERT,
        SqlStatement::Merge(_) => commands::MERGE,
        SqlStatement::Open(_) => commands::OPEN,
        SqlStatement::Rollback => commands::ROLLBACK_WORK,
        SqlStatement::Select { .. } => commands::SELECT_ROW,
        SqlStatement::SetTransaction(_) => commands::SET_TRANSACTION,
        SqlStatement::SetSchema(_) => commands::SET_SCHEMA,
        SqlStatement::SetCollation(_) => commands::SET_COLLATION,
        SqlStatement::SetCatalog(_) => commands::SET_CATALOG,
        SqlStatement::SetTimeZone(_) => commands::SET_TIME_ZONE,
        SqlStatement::SetSessionAuthorization(_) => commands::SET_SESSION_AUTHORIZATION,
        SqlStatement::SetRole(_) => commands::SET_ROLE,
        SqlStatement::Connection(ConnectionStatement::Connect { .. } | ConnectionStatement::ConnectDefault) => commands::CONNECT,
        SqlStatement::Connection(ConnectionStatement::Set(_)) => commands::SET_CONNECTION,
        SqlStatement::Connection(ConnectionStatement::Disconnect(_)) => commands::DISCONNECT,
        SqlStatement::SetConstraints { .. } => commands::SET_CONSTRAINT,
        SqlStatement::UpdatePositioned { .. } => commands::UPDATE_CURSOR,
        SqlStatement::UpdateSearched { .. } => commands::UPDATE_WHERE,
        SqlStatement::SetSetting { .. } => commands::SET_SETTING,
        SqlStatement::SetDatabase { .. } => commands::SET_DATABASE,
        SqlStatement::Index(IndexStatement::Create { .. }) => commands::CREATE_INDEX,
        SqlStatement::Index(IndexStatement::Drop { .. }) => commands::DROP_INDEX,
        SqlStatement::Index(IndexStatement::Alter { .. }) => commands::ALTER_INDEX,
        SqlStatement::User(UserStatement::Create { .. }) => commands::CREATE_USER,
        SqlStatement::User(UserStatement::Alter { .. } | UserStatement::RoleCreator { .. }) => commands::ALTER_USER,
        SqlStatement::User(UserStatement::Drop { .. }) => commands::DROP_USER,
        SqlStatement::User(UserStatement::Owner { .. } | UserStatement::Members { .. }) => commands::ALTER_DATABASE,
        SqlStatement::Served(ServedStatement::End { .. }) => commands::END_CONNECTION,
        SqlStatement::Served(ServedStatement::Cancel { .. }) => commands::CANCEL_CONNECTION,
        SqlStatement::Call(_) => commands::CALL,
        SqlStatement::Return(_) => commands::RETURN,
    }
}

//! \[EXTENSION\] [ISO/IEC 9075:1992 §17] Prepared statements: a
//! `<preparable statement>` [§17.6] prepared once, its `<dynamic parameter
//! specification>`s' types deduced and its result described [§17.8, §17.9
//! GR3, GR4]; then executed [§17.10], or, a `<dynamic select statement>`,
//! opened as a cursor [§17.13, §17.14], as often as asked, each time with values
//! for its parameters [§17.9 GR5].
//!
//! A query, an INSERT and a searched or positioned UPDATE or DELETE keep
//! their plan between executions, a query opened as a dynamic cursor's
//! too, bound again only when what it was bound with changed
//! (`prepared/plans.rs`, RULES.md D45); each execution fills in its values
//! and chooses the access paths by them. The statement is checked
//! against the definitions as they are when an execution binds it: what it
//! was prepared against may have changed [§17.6 GR10].
//!
//! The same preparation serves PREPARE in SQL text (`db/dynamic.rs`).

mod plans;

use std::sync::Arc;

pub(super) use plans::{Keeping, KeptPlan, Plans};

use super::dynamic::Name;
use super::{Completion, Cursor, Session, State};
use crate::ast::{SqlStatement, TableName};
use crate::binder::{self, OutColumn, WriteKind, bind_write};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::lexer::{Tok, Token, kw, lex};
use crate::params::{ParameterType, Params};
use crate::settings::FUNCTION_CALLS;
use crate::value::Value;

/// What a prepared statement is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A `<dynamic select statement>`: a `<cursor specification>`.
    Query,
    /// A `<schema definition>`, processed as `Session::direct` does.
    Schema,
    /// Any other `<preparable statement>`.
    Statement,
}

/// \[EXTENSION\] [ISO/IEC 9075:1992 §17.6] A prepared statement. Its
/// clones share the plans it keeps (RULES.md D45).
#[derive(Debug, Clone)]
pub struct Prepared {
    sql: String,
    kind: Kind,
    parameters: Vec<ParameterType>,
    columns: Vec<OutColumn>,
    plans: Arc<Plans>,
    /// Whether it is a `<dynamic single row select statement>` [§17.6]: a
    /// `<query specification>` alone.
    single_row: bool,
    /// Of a positioned DELETE or UPDATE, the cursor its `<cursor name>`
    /// names [§17.6 GR2], and the table it changes if it names none
    /// [§17.19 SR1, §17.20 SR1].
    cursor: Option<Name>,
    implicit: Option<TableName>,
}

impl Prepared {
    /// Its text.
    pub fn sql(&self) -> &str {
        &self.sql
    }

    /// [§17.9 GR4] Its `<dynamic parameter specification>`s' types, in the
    /// order written, each possibly null \[GR4e ii\].
    pub fn parameters(&self) -> &[ParameterType] {
        &self.parameters
    }

    /// [§17.9 GR3] The columns of its result, of a `<dynamic select
    /// statement>`; none of another.
    pub fn columns(&self) -> &[OutColumn] {
        &self.columns
    }

    /// Whether it is a `<dynamic select statement>`, opened as a cursor.
    pub fn is_query(&self) -> bool {
        self.kind == Kind::Query
    }

    /// \[DESIGN\] How many plans its executions, its clones' too, made: one
    /// kept while what it was made with holds, another each time that
    /// changed, and one for each execution whose plan holds a value it read
    /// (RULES.md D45). Only a query, an INSERT, and a searched or
    /// positioned UPDATE or DELETE make one.
    pub fn plans_made(&self) -> u64 {
        self.plans.made()
    }

    /// Whether it is a `<dynamic single row select statement>`, which
    /// EXECUTE runs [§17.10 GR2].
    pub(super) fn is_single_row(&self) -> bool {
        self.single_row
    }

    /// The cursor it changes a row through, if it does [§17.6 GR2].
    pub(super) fn cursor(&self) -> Option<&Name> {
        self.cursor.as_ref()
    }

    /// What it keeps between its executions, shared with its clones.
    pub(super) fn plans(&self) -> &Plans {
        &self.plans
    }
}

/// Who prepares a statement: the API (`Session::prepare`, a server's
/// protocol), or SQL text's PREPARE or EXECUTE IMMEDIATE, of §17's rules
/// in full (RULES.md D58).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Preparer {
    /// `Session::prepare`: a statement with a `<comment>` is taken, as
    /// clients send them, and a positioned statement's cursor that is not
    /// there is looked for as it runs.
    Api,
    /// PREPARE or EXECUTE IMMEDIATE [§17.6, §17.11].
    Sql,
}

/// [ISO/IEC 9075:1992 §17.6 GR4] `e`, raised preparing a statement: a
/// statement that breaks a Format, a Syntax Rule or an Access Rule is of
/// class 37 here, not of direct SQL's 2A [§20.1 GR5].
pub(super) fn in_dynamic(e: SqlError) -> SqlError {
    if e.sqlstate() == SqlState::SYNTAX_ERROR_IN_DIRECT_STATEMENT { e.with_state(SqlState::DYNAMIC_SYNTAX_ERROR) } else { e }
}

/// [ISO/IEC 9075:1992 §17.6 GR4q] Whether `sql`, made of `tokens`, has a
/// `<comment>` in a `<separator>` between them.
fn has_comment(sql: &str, tokens: &[Token]) -> bool {
    let chars: Vec<char> = sql.chars().collect();
    let mut from = 0;
    for t in tokens {
        if chars.get(from..t.start).is_some_and(|gap| gap.windows(2).any(|w| w == [COMMENT_START, COMMENT_START])) {
            return true;
        }
        from = t.end;
    }
    false
}

/// [ISO/IEC 9075:1992 §5.2] The character a `<comment introducer>`, two of
/// it, is made of.
const COMMENT_START: char = '-';

/// [§17.6 GR4a] A statement that is not a `<preparable statement>`
/// (SQLSTATE 37000).
fn not_preparable(what: &str) -> SqlError {
    SqlError::new(Code::SyntaxRule, format!("[ISO/IEC 9075:1992 §17.6 GR4a] {what} is not a <preparable statement>"))
        .with_state(SqlState::DYNAMIC_SYNTAX_ERROR)
}

/// A dynamic SQL error of SQLSTATE `state`.
fn dynamic(state: SqlState, msg: String) -> SqlError {
    SqlError::new(Code::Dynamic, msg).with_state(state)
}

/// What `tokens` are, by the first, as `Session::direct` tells.
fn kind_of(tokens: &[Token]) -> Kind {
    let mut words = tokens.iter().map(|t| &t.tok);
    match (words.next(), words.next()) {
        // [ISO/IEC 9075:1992 §7.10] a query, a <joined table> of a table's
        // name too, MODULE's: no other statement begins so.
        (Some(Tok::KeyWord(kw::SELECT | kw::VALUES | kw::TABLE) | Tok::Delimiter("(") | Tok::Identifier(_) | Tok::Introducer(_)), _)
        | (Some(Tok::KeyWord(kw::MODULE)), Some(Tok::Delimiter("."))) => Kind::Query,
        (Some(Tok::KeyWord(kw::CREATE)), Some(Tok::KeyWord(kw::SCHEMA))) => Kind::Schema,
        _ => Kind::Statement,
    }
}

impl Session {
    /// \[EXTENSION\] [ISO/IEC 9075:1992 §17.6] Prepares `sql`, a
    /// `<preparable statement>`: checks it against the definitions,
    /// deduces each `?`'s type from where it stands \[GR5\], and describes
    /// its result.
    ///
    /// # Errors
    ///
    /// Those of the statement's Format, Syntax and Access Rules, of SQLSTATE
    /// 37000 \[GR4a\]: so too if it is not a `<preparable statement>`, or a
    /// `?` takes no type where it stands \[GR4\]; of a positioned DELETE or
    /// UPDATE that names no table, SQLSTATE 34000 if no cursor of its name
    /// is declared or allocated; of any, 3C000 if more than one is \[GR2\].
    /// \[DESIGN\] A `<comment>` is taken, which SQL's PREPARE refuses
    /// \[GR4q\] (RULES.md D58).
    pub fn prepare(&self, sql: &str) -> Result<Prepared> {
        let made = self.prepared(sql).map_err(in_dynamic);
        self.diagnosed(made, |_| SqlState::SUCCESSFUL_COMPLETION)
    }

    /// `prepare`'s work.
    fn prepared(&self, sql: &str) -> Result<Prepared> {
        let call = self.enter()?;
        // [ISO/IEC 9075:1992 §18.1 Table 22] a <prepare statement>.
        self.emptied(("PREPARE", ""));
        self.prepared_with(call.state(), sql, Preparer::Api)
    }

    /// `sql` prepared as `prepare` prepares it, the diagnostics area left
    /// as it is: of EXECUTE IMMEDIATE [ISO/IEC 9075:1992 §17.11 GR4].
    pub(super) fn prepared_quietly(&self, sql: &str) -> Result<Prepared> {
        let call = self.enter()?;
        self.prepared_with(call.state(), sql, Preparer::Sql).map_err(in_dynamic)
    }

    /// `sql` prepared against the definitions `st`, by `by`.
    pub(super) fn prepared_with(&self, st: &State, sql: &str, by: Preparer) -> Result<Prepared> {
        let tokens = lex(sql)?;
        if by == Preparer::Sql && has_comment(sql, &tokens) {
            let msg = "[ISO/IEC 9075:1992 §17.6 GR4q] a statement prepared has no <comment>";
            return Err(SqlError::new(Code::SyntaxRule, msg).with_state(SqlState::DYNAMIC_SYNTAX_ERROR));
        }
        let kind = kind_of(&tokens);
        let (cursor, implicit) = self.positioned_reference(st, &tokens, by)?;
        let mut parser = crate::parser::Parser::for_prepare(sql, st.settings.is_on(FUNCTION_CALLS))?
            .in_catalog(&st.catalog_name())
            .implying(implicit.clone());
        let mut single_row = false;
        let (columns, params) = match kind {
            Kind::Query => {
                let spec = parser.cursor_specification()?;
                // [§17.6] a <dynamic single row select statement> ::= <query specification>.
                single_row = spec.query.spec().is_some()
                    && spec.order_by.is_empty()
                    && spec.updatability.is_none()
                    && matches!(tokens.first().map(|t| &t.tok), Some(Tok::KeyWord(kw::SELECT)));
                let params = Params::preparing(parser.dynamic_parameters());
                (self.compile(st, &spec, &params)?.columns, params)
            }
            Kind::Schema => {
                parser.schema()?;
                (Vec::new(), Params::preparing(0))
            }
            Kind::Statement => {
                let stmt = parser.sql_statement()?;
                let params = Params::preparing(parser.dynamic_parameters());
                self.bind_only(st, &stmt, &params)?;
                (Vec::new(), params)
            }
        };
        let parameters = params.dynamic_types().into_iter().collect::<Option<Vec<_>>>().ok_or_else(|| {
            SqlError::new(
                Code::SyntaxRule,
                "[ISO/IEC 9075:1992 §17.6 GR4] a <dynamic parameter specification> takes no type where it stands",
            )
            .with_state(SqlState::DYNAMIC_SYNTAX_ERROR)
        })?;
        Ok(Prepared { sql: sql.to_owned(), kind, parameters, columns, plans: Arc::default(), single_row, cursor, implicit })
    }

    /// `stmt`, a `<preparable statement>` other than a query, bound as it
    /// would be run, without running it: so each `?` takes its type.
    fn bind_only(&self, st: &State, stmt: &SqlStatement, params: &Params) -> Result<()> {
        match stmt {
            SqlStatement::Insert(ins) => {
                let target = self.insert_target(st, ins, None)?;
                if let crate::ast::InsertSource::Query(query) = &ins.source {
                    self.bound_insert(st, &target, query, params)?;
                }
            }
            SqlStatement::UpdateSearched { table, set, where_clause } => {
                self.bound(st, params, |cx| bind_write(cx, WriteKind::UpdateSearched, table, set, where_clause.as_ref()))?;
            }
            SqlStatement::DeleteSearched { table, where_clause } => {
                self.bound(st, params, |cx| bind_write(cx, WriteKind::DeleteSearched, table, &[], where_clause.as_ref()))?;
            }
            SqlStatement::UpdatePositioned { table, set, .. } => {
                self.bound(st, params, |cx| bind_write(cx, WriteKind::UpdatePositioned, table, set, None))?;
            }
            SqlStatement::DeletePositioned { table, .. } => {
                self.bound(st, params, |cx| bind_write(cx, WriteKind::DeletePositioned, table, &[], None))?;
            }
            // [§17.6 GR5u]
            SqlStatement::SetSchema(v) | SqlStatement::SetCatalog(v) | SqlStatement::SetSessionAuthorization(v) => {
                drop(binder::value_specification_as(v, &self.auth(), params, ParameterType::Varying)?);
            }
            SqlStatement::SetTimeZone(Some(v)) => drop(self.bound(st, params, |cx| binder::bind_value(cx, v))?),
            // Of a module's cursors and targets: FETCH and SELECT ... INTO
            // name parameters, which a prepared statement has none of, so
            // only OPEN and CLOSE reach here.
            SqlStatement::Open(_) | SqlStatement::Fetch { .. } | SqlStatement::Close(_) | SqlStatement::Select { .. } => {
                return Err(not_preparable("a statement of a module's cursor"));
            }
            SqlStatement::GetDiagnostics(_) => return Err(not_preparable("a <get diagnostics statement>")),
            SqlStatement::Dynamic(_) | SqlStatement::Execute(_) => return Err(not_preparable("an <SQL dynamic statement>")),
            SqlStatement::DeclareTable(_) => return Err(not_preparable("a <temporary table declaration>")),
            SqlStatement::Commit
            | SqlStatement::Rollback
            | SqlStatement::SetConstraints { .. }
            | SqlStatement::SetTransaction(_)
            | SqlStatement::SetTimeZone(None)
            | SqlStatement::Connection(_)
            | SqlStatement::SetSetting { .. }
            | SqlStatement::SetDatabase { .. }
            | SqlStatement::Drop { .. }
            | SqlStatement::AlterTable { .. }
            | SqlStatement::Schema { .. }
            | SqlStatement::Index(_)
            | SqlStatement::User(_)
            | SqlStatement::Served(_) => {}
        }
        Ok(())
    }

    /// [§17.9 GR5a] `values`, one for each of `p`'s parameters.
    fn values_for(p: &Prepared, values: &[Value]) -> Result<Params> {
        if values.len() != p.parameters.len() {
            let (given, taken) = (values.len(), p.parameters.len());
            return Err(dynamic(
                SqlState::USING_CLAUSE_MISMATCH,
                format!("[ISO/IEC 9075:1992 §17.9 GR5a] {given} values for {taken} <dynamic parameter specification>s"),
            ));
        }
        Ok(Params::executing(values.to_vec()))
    }

    /// \[EXTENSION\] [ISO/IEC 9075:1992 §17.10] Executes `p`, not a query,
    /// with `values` for its parameters, each cast to its type [§17.9 GR5];
    /// as `execute` runs a statement, or `direct` a schema definition.
    ///
    /// # Errors
    ///
    /// SQLSTATE 07003 if `p` is a query \[GR2\]; 07001 if `values` are not one
    /// for each parameter [§17.9 GR5a]; CAST's [§17.9 GR5f]; the
    /// statement's.
    pub fn execute_prepared(&self, p: &Prepared, values: &[Value]) -> Result<Completion> {
        if p.kind == Kind::Query {
            let msg = "[ISO/IEC 9075:1992 §17.10 GR2] a <cursor specification> is opened, not executed".to_owned();
            return Err(dynamic(SqlState::CURSOR_SPECIFICATION_NOT_EXECUTED, msg));
        }
        Self::values_for(p, values)?;
        self.run_prepared(p, values.to_vec(), "EXECUTE")
    }

    /// `p`, not a query, run with `values` for its parameters, as `via`,
    /// EXECUTE or EXECUTE IMMEDIATE, runs it: as `execute` runs a
    /// statement, its positioned DELETE or UPDATE through the cursor found
    /// as it was prepared [§17.6 GR2]; or a schema definition as `direct`
    /// runs it.
    pub(super) fn run_prepared(&self, p: &Prepared, values: Vec<Value>, via: &'static str) -> Result<Completion> {
        if p.kind == Kind::Schema {
            return self.direct(&p.sql).map(|_| Completion::Success);
        }
        let mut params = Params::executing(values).implying(p.implicit.clone());
        self.inner.borrow_mut().dynamic.positioned.clone_from(&p.cursor);
        let done = self.execute_as(&p.sql, &mut params, Some((via, &p.plans)));
        self.inner.borrow_mut().dynamic.positioned = None;
        done
    }

    /// \[EXTENSION\] [ISO/IEC 9075:1992 §17.14] Opens `p`, a query, as an
    /// unnamed cursor, with `values` for its parameters, as `open_cursor`
    /// opens one.
    ///
    /// # Errors
    ///
    /// SQLSTATE 07005 if `p` is not a query [§17.13 GR2]; 07001 if `values`
    /// are not one for each parameter [§17.9 GR5a]; CAST's; the cursor's.
    pub fn open_prepared(&self, p: &Prepared, values: &[Value]) -> Result<Cursor> {
        if p.kind != Kind::Query {
            let msg = "[ISO/IEC 9075:1992 §17.13 GR2] a prepared statement opened as a cursor is a <cursor specification>".to_owned();
            return Err(dynamic(SqlState::NOT_A_CURSOR_SPECIFICATION, msg));
        }
        let params = Self::values_for(p, values)?;
        // [ISO/IEC 9075:1992 §18.1 Table 22] a <dynamic open statement>.
        self.open_with(("DYNAMIC OPEN", "SELECT"), |call| self.open_kept(call, p, &params))
    }
}

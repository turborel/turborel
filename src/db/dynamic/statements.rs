//! [ISO/IEC 9075:1992 §17.6-§17.8, §17.10, §17.11] PREPARE, DEALLOCATE
//! PREPARE, DESCRIBE, EXECUTE and EXECUTE IMMEDIATE in SQL text: a
//! session's prepared statements, by name.
//!
//! EXECUTE and EXECUTE IMMEDIATE run the statement prepared as
//! `Session::execute_prepared` runs one, as a statement of its own: the
//! diagnostics area names EXECUTE, or EXECUTE IMMEDIATE, and the statement
//! prepared as its DYNAMIC_FUNCTION [§18.1 GR1c, GR1d]; and whether it
//! begins a transaction is the prepared statement's to say [§4.22.6].

use super::descriptor::Item;
use super::using::Targets;
use super::{Name, dynamic_error, value_of};
use crate::ast::{DynamicName, ExecuteStatement, ExtendedName, UsingClause, ValueSpecification};
use crate::db::prepared::in_dynamic;
use crate::db::{Completion, Prepared, Session, State, cursor_error};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::params::Params;
use crate::value::{DataType, Value};

/// [ISO/IEC 9075:1992 §17.7 GR1, §17.8 GR1, §17.10 GR1, §17.13 GR1,
/// §17.14 GR1] No statement named `name` is prepared, as `rule` says.
pub(super) fn not_prepared(name: &Name, rule: &str) -> SqlError {
    dynamic_error(SqlState::INVALID_STATEMENT_NAME, format!("[ISO/IEC 9075:1992 {rule}] no statement {name} is prepared"))
}

/// `e`, raised preparing the text a parameter or literal holds, without
/// its position, which is in that text, not in the statement written.
fn unplaced(e: SqlError) -> SqlError {
    SqlError { pos: None, ..e }
}

impl Session {
    /// The name `name` gives a prepared statement, as `rule` checks it.
    fn statement_key(&self, name: &DynamicName, params: &Params, rule: &str) -> Result<Name> {
        self.dynamic_name(name, params, (SqlState::INVALID_STATEMENT_NAME, rule))
    }

    /// The statement prepared as `key`, as `rule` says it must be.
    pub(super) fn prepared_as(&self, key: &Name, rule: &str) -> Result<Prepared> {
        self.inner.borrow().dynamic.statements.get(key).cloned().ok_or_else(|| not_prepared(key, rule))
    }

    /// [ISO/IEC 9075:1992 §17.6 SR2, §17.11 SR1] The value of an `<SQL
    /// statement variable>`, a character string.
    fn statement_text(&self, v: &ValueSpecification, params: &Params, rule: &str) -> Result<String> {
        match value_of(self, v, params)? {
            (Value::Char(text), _) => Ok(text),
            (_, _) => Err(SqlError::new(
                Code::SyntaxRule,
                format!("[ISO/IEC 9075:1992 {rule}] an <SQL statement variable> is a character string"),
            )),
        }
    }

    /// [ISO/IEC 9075:1992 §17.6] PREPARE: the statement the variable holds,
    /// prepared \[GR7\] as `Session::prepare` prepares one, named; one of
    /// the name prepared before is deallocated first \[GR6\].
    ///
    /// # Errors
    ///
    /// SQLSTATE 26000 if the extended name is no `<identifier>` \[GR8\];
    /// 37000 if the statement is not a `<preparable statement>`, or breaks
    /// a rule of GR4; 07005 if the name is a dynamic cursor's, and the
    /// statement is not a `<cursor specification>` \[GR9b ii\]; those of
    /// DEALLOCATE PREPARE.
    pub(super) fn prepare_named(&self, st: &State, name: &DynamicName, source: &ValueSpecification, params: &Params) -> Result<Completion> {
        let key = self.statement_key(name, params, "§17.6 GR8")?;
        let text = self.statement_text(source, params, "§17.6 SR2")?;
        if self.inner.borrow().dynamic.statements.contains_key(&key) {
            self.deallocate(&key)?;
        }
        let prepared = self.prepared_with(st, &text).map_err(|e| unplaced(in_dynamic(e)))?;
        let declared = self.inner.borrow().cursors.iter().any(|(n, c)| n.scope.is_none() && c.statement.as_ref() == Some(&key));
        if declared && !prepared.is_query() {
            let msg = format!("[ISO/IEC 9075:1992 §17.6 GR9b ii] statement {key} is a dynamic cursor's, so a <cursor specification>");
            return Err(dynamic_error(SqlState::NOT_A_CURSOR_SPECIFICATION, msg));
        }
        self.inner.borrow_mut().dynamic.statements.insert(key, prepared);
        Ok(Completion::Success)
    }

    /// [ISO/IEC 9075:1992 §17.7] DEALLOCATE PREPARE.
    ///
    /// # Errors
    ///
    /// As `deallocate`.
    pub(super) fn deallocate_prepare(&self, name: &DynamicName, params: &Params) -> Result<Completion> {
        let key = self.statement_key(name, params, "§17.7 GR1")?;
        self.deallocate(&key).map(|()| Completion::Success)
    }

    /// [ISO/IEC 9075:1992 §17.7 GR3] The statement prepared as `key`
    /// destroyed, and the cursors allocated for it; and, of a `<cursor
    /// specification>`, the statements prepared that change a row through
    /// a cursor of it.
    ///
    /// # Errors
    ///
    /// SQLSTATE 26000 if none is prepared so \[GR1\]; 24000 if a cursor of it
    /// is open \[GR2\].
    fn deallocate(&self, key: &Name) -> Result<()> {
        let mut inner = self.inner.borrow_mut();
        if !inner.dynamic.statements.contains_key(key) {
            return Err(not_prepared(key, "§17.7 GR1"));
        }
        let of_it: Vec<Name> = inner.cursors.iter().filter(|(_, c)| c.statement.as_ref() == Some(key)).map(|(n, _)| n.clone()).collect();
        if let Some(open) = of_it.iter().find(|n| inner.cursors.get(*n).is_some_and(|c| c.open.is_some())) {
            return cursor_error(&open.name, format!("[ISO/IEC 9075:1992 §17.7 GR2] statement {key} is of cursor {open}, which is open"));
        }
        inner.dynamic.statements.remove(key);
        inner.cursors.retain(|n, c| n.scope.is_none() || c.statement.as_ref() != Some(key));
        inner.dynamic.statements.retain(|_, p| p.cursor().is_none_or(|c| !of_it.contains(c)));
        Ok(())
    }

    /// [ISO/IEC 9075:1992 §17.8] DESCRIBE: the SQL descriptor area set as
    /// §17.9 GR3 or GR4 says, of the statement's `<select list>` columns,
    /// OUTPUT, or of its `<dynamic parameter specification>`s, INPUT.
    ///
    /// # Errors
    ///
    /// SQLSTATE 26000 if no statement of the name is prepared \[GR1\]; 33000
    /// if no area of the descriptor's name is allocated [§17.9 GR2].
    pub(super) fn describe(&self, input: bool, name: &DynamicName, descriptor: &ExtendedName, params: &Params) -> Result<Completion> {
        let key = self.statement_key(name, params, "§17.8 GR1")?;
        let prepared = self.prepared_as(&key, "§17.8 GR1")?;
        let catalog = self.catalog_now()?;
        let items: Vec<Item> = if input {
            prepared.parameters().iter().map(|t| Item::described(t.data_type(), None, true, &catalog)).collect()
        } else {
            prepared.columns().iter().map(|c| Item::described(c.data_type, c.name.as_deref(), !c.not_null, &catalog)).collect()
        };
        self.describe_into(descriptor, params, items)
    }

    /// [ISO/IEC 9075:1992 §17.10, §17.11] EXECUTE or EXECUTE IMMEDIATE `e`,
    /// as a statement of its own, its condition placed in the diagnostics
    /// area, which names it as `command` [§18.1 GR1c].
    ///
    /// # Errors
    ///
    /// As `execute_named` or `execute_immediate`.
    pub(in crate::db) fn execute_dynamic(&self, e: &ExecuteStatement, command: &'static str, params: &mut Params) -> Result<Completion> {
        self.emptied((command, ""));
        let done = match e {
            ExecuteStatement::Prepared { name, result, parameters } => {
                self.execute_named(name, (result.as_ref(), parameters.as_ref()), params)
            }
            ExecuteStatement::Immediate(source) => self.execute_immediate(source, params),
        };
        self.diagnosed(done, |c| c.sqlstate())
    }

    /// [ISO/IEC 9075:1992 §17.10] EXECUTE: the statement prepared as `name`
    /// run with the values of its `<parameter using clause>` \[GR6, GR8\];
    /// a `<dynamic single row select statement>`'s row put where its
    /// `<result using clause>` says \[GR7, GR8a\].
    ///
    /// # Errors
    ///
    /// SQLSTATE 26000 if no statement of the name is prepared \[GR1\]; 07003
    /// if it is a `<dynamic select statement>` of more than one `<query
    /// specification>` \[GR2\]; 07004 if it has dynamic parameters and no
    /// parameter using clause \[GR4\]; 07007 if it is a single row select
    /// and no result using clause is written \[GR5\]; those of §17.9; the
    /// statement's.
    fn execute_named(
        &self,
        name: &DynamicName,
        (result, parameters): (Option<&UsingClause>, Option<&UsingClause>),
        params: &mut Params,
    ) -> Result<Completion> {
        let key = self.statement_key(name, params, "§17.10 GR1")?;
        let prepared = self.prepared_as(&key, "§17.10 GR1")?;
        let types: Vec<DataType> = prepared.parameters().iter().map(|t| t.data_type()).collect();
        if !prepared.is_query() {
            // [§17.9 GR6] a statement of no row: of degree 0.
            if let Some(result) = result {
                self.using_targets(result, &[], params, ("§8.10 GR8", "§8.10 GR9"))?;
            }
            let values = self.using_values(parameters, &types, params, "§17.10 GR4")?;
            return self.run_prepared(&prepared, values, "EXECUTE");
        }
        if !prepared.is_single_row() {
            let msg =
                "[ISO/IEC 9075:1992 §17.10 GR2] a <cursor specification> of more than a <query specification> is opened, not executed";
            return Err(dynamic_error(SqlState::CURSOR_SPECIFICATION_NOT_EXECUTED, msg.to_owned()));
        }
        let Some(result) = result else {
            let msg = "[ISO/IEC 9075:1992 §17.10 GR5] a <dynamic single row select statement> needs a <result using clause>";
            return Err(dynamic_error(SqlState::USING_CLAUSE_REQUIRED_RESULTS, msg.to_owned()));
        };
        let targets = self.using_targets(result, prepared.columns(), params, ("§8.10 GR8", "§8.10 GR9"))?;
        let values = self.using_values(parameters, &types, params, "§17.10 GR4")?;
        self.single_row(&prepared, values, &targets, params)
    }

    /// [ISO/IEC 9075:1992 §17.10 GR8a, §13.5] The one row of the `<dynamic
    /// single row select statement>` `prepared`, with `values` for its
    /// parameters, put where `targets` says; no data if it has none.
    fn single_row(&self, prepared: &Prepared, values: Vec<Value>, targets: &Targets<'_>, params: &mut Params) -> Result<Completion> {
        let executing = Params::executing(values);
        let mut rows = self.open_with(("EXECUTE", "SELECT"), |call| self.open_kept(call, prepared, &executing))?;
        // [§13.5 GR2b] no row: no data.
        let Some(row) = rows.fetch()? else { return Ok(Completion::NoData) };
        if rows.fetch()?.is_some() {
            return Err(SqlError::new(Code::Cardinality, "[ISO/IEC 9075:1992 §13.5 GR2a] the result has more than one row"));
        }
        let columns = rows.columns.clone();
        drop(rows);
        self.put_row(targets, &columns, &row, params)
    }

    /// [ISO/IEC 9075:1992 §17.11] EXECUTE IMMEDIATE: the statement the
    /// variable holds, prepared and executed \[GR4\], and forgotten.
    ///
    /// # Errors
    ///
    /// SQLSTATE 37000 if it is not a `<preparable statement>`, or a query,
    /// or has a `<comment>` or a `<dynamic parameter specification>`
    /// \[GR3\]; the statement's.
    fn execute_immediate(&self, source: &ValueSpecification, params: &Params) -> Result<Completion> {
        let text = self.statement_text(source, params, "§17.11 SR1")?;
        let prepared = self.prepared_quietly(&text).map_err(unplaced)?;
        if prepared.is_query() || !prepared.parameters().is_empty() {
            let msg = "[ISO/IEC 9075:1992 §17.11 GR3a, GR3c] a statement executed immediately is no query, and has no <dynamic parameter specification>";
            return Err(SqlError::new(Code::SyntaxRule, msg).with_state(SqlState::DYNAMIC_SYNTAX_ERROR));
        }
        self.run_prepared(&prepared, Vec::new(), "EXECUTE IMMEDIATE")
    }
}

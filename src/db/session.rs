//! \[EXTENSION\] [ISO/IEC 9075:1992 §14.1, §16.2, §4.28, §4.30] The SET
//! statements of a session: SET TRANSACTION, the access mode and isolation
//! level of its next transaction, and SET SCHEMA, the default schema of
//! unqualified names. Split from db.rs (RULES.md R-SIZE-2).

use std::sync::Arc;

use super::{Session, State};
use crate::ast::{AccessMode, IsolationLevel, SqlStatement, TableName, TransactionModes, ValueSpecification};
use crate::binder::{self, qualify};
use crate::error::{Code, Result, SqlError, SqlState, fail, fail_with};
use crate::lexer::{Tok, authorization_identifier, delim, lex};
use crate::params::{ParameterType, Params};
use crate::txn::{Reading, Stop};
use crate::value::{DataType, Value};

/// [ISO/IEC 9075:1992 §4.28] The access mode and isolation level of a
/// transaction: READ WRITE and SERIALIZABLE unless SET TRANSACTION said
/// otherwise [§12.3 GR6c iii 2B].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransactionMode {
    /// Its access mode.
    pub access: AccessMode,
    /// The isolation level asked for [§14.1 GR5]: SERIALIZABLE runs as
    /// itself; REPEATABLE READ as snapshot isolation; READ COMMITTED and
    /// READ UNCOMMITTED with a snapshot for each statement (`Reading`).
    pub isolation: IsolationLevel,
}

impl TransactionMode {
    /// \[EXTENSION\] How a transaction of these modes reads [§14.1 GR5]:
    /// none of the phenomena its level would not show [§4.28, Table 9].
    pub(crate) fn reading(self) -> Reading {
        match self.isolation {
            IsolationLevel::Serializable => Reading::Serializable,
            IsolationLevel::RepeatableRead => Reading::Snapshot,
            IsolationLevel::ReadCommitted | IsolationLevel::ReadUncommitted => Reading::Statement,
        }
    }
}

impl Default for TransactionMode {
    fn default() -> TransactionMode {
        TransactionMode { access: AccessMode::ReadWrite, isolation: IsolationLevel::Serializable }
    }
}

impl Session {
    /// [ISO/IEC 9075:1992 §14.1] SET TRANSACTION: the modes of the next
    /// transaction. The isolation level is SERIALIZABLE if none is written
    /// \[SR2\]; the access mode READ ONLY under READ UNCOMMITTED, else READ
    /// WRITE, if none is written \[SR4\]. A `<diagnostics size>` is checked
    /// \[SR5, GR2\]: a statement reports one condition, its error, never
    /// more than any limit of at least 1 allows \[GR6, GR7\].
    ///
    /// # Errors
    ///
    /// SQLCODE -917 if a transaction is active \[GR1\]; -103 if the
    /// `<number of conditions>` is not exact with scale 0 \[SR5\]; -301 if it
    /// is less than 1 \[GR2\].
    pub(super) fn set_transaction(&self, modes: &TransactionModes, params: &Params) -> Result<()> {
        if self.inner.borrow().tx.is_some() {
            return fail(Code::TransactionState, "[ISO/IEC 9075:1992 §14.1 GR1] SET TRANSACTION while a transaction is active".to_owned());
        }
        if let Some(size) = &modes.diagnostics {
            let (n, data_type) = binder::value_specification(size, &self.auth(), params)?;
            if !matches!(data_type, DataType::Exact { scale: 0, .. }) {
                return fail(
                    Code::SyntaxRule,
                    "[ISO/IEC 9075:1992 §14.1 SR5] the <number of conditions> is exact numeric with scale 0".to_owned(),
                );
            }
            if n.as_f64().is_none_or(|n| n < 1.0_f64) {
                return fail_with(
                    Code::Data,
                    SqlState::INVALID_CONDITION_NUMBER,
                    format!("[ISO/IEC 9075:1992 §14.1 GR2] the <number of conditions> {n} is less than 1"),
                );
            }
        }
        let isolation = modes.isolation.unwrap_or(IsolationLevel::Serializable);
        let read_only = AccessMode::ReadOnly;
        let access = modes.access.unwrap_or(if isolation == IsolationLevel::ReadUncommitted { read_only } else { AccessMode::ReadWrite });
        self.next.set(TransactionMode { access, isolation });
        Ok(())
    }

    /// The modes of the active transaction, if one is.
    pub fn transaction_mode(&self) -> Option<TransactionMode> {
        self.inner.borrow().tx.as_ref().map(|_| self.mode.get())
    }

    /// The modes the transaction it begins now takes: those SET TRANSACTION
    /// gave since the last ended, or the defaults [§12.3 GR6c iii 2].
    pub(super) fn begin_mode(&self) {
        self.mode.set(self.next.take());
    }

    /// [ISO/IEC 9075:1992 §13.6-§13.10 GR1, §12.3 GR6c v] In a READ ONLY
    /// transaction, no statement changes data but a temporary table's, nor
    /// a schema.
    ///
    /// # Errors
    ///
    /// SQLCODE -917 if `stmt` would.
    pub(super) fn may_run(&self, st: &State, stmt: &SqlStatement) -> Result<()> {
        let temporary =
            |table: &TableName| st.catalog.table(&qualify(table, &self.default_schema())).is_some_and(|t| t.temporary.is_some());
        let (changes, rule) = match stmt {
            SqlStatement::Insert(ins) => (!temporary(&ins.table), "§13.8 GR1"),
            SqlStatement::UpdateSearched { table, .. } => (!temporary(table), "§13.10 GR1"),
            SqlStatement::UpdatePositioned { table, .. } => (!temporary(table), "§13.9 GR1"),
            SqlStatement::DeleteSearched { table, .. } => (!temporary(table), "§13.7 GR1"),
            SqlStatement::DeletePositioned { table, .. } => (!temporary(table), "§13.6 GR1"),
            SqlStatement::Drop { .. } | SqlStatement::AlterTable { .. } | SqlStatement::Schema { .. } => (true, "§12.3 GR6c v"),
            SqlStatement::Open(_)
            | SqlStatement::Fetch { .. }
            | SqlStatement::Close(_)
            | SqlStatement::Select { .. }
            | SqlStatement::Commit
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
            | SqlStatement::GetDiagnostics(_)
            | SqlStatement::DeclareTable(_) => (false, ""),
        };
        if changes && self.mode.get().access == AccessMode::ReadOnly {
            return fail(Code::TransactionState, format!("[ISO/IEC 9075:1992 {rule}] the transaction is READ ONLY"));
        }
        Ok(())
    }

    /// [ISO/IEC 9075:1992 §16.2] SET SCHEMA: the value of `v`, without
    /// leading or trailing spaces \[GR1\], a `<schema name>`, becomes the
    /// default schema of the unqualified names of the statements it runs
    /// after \[GR3b\]: an `<identifier>`, a regular one in upper case, or
    /// one qualified by the name of the catalog, there being one, which SET
    /// CATALOG would set [GR3a, §16.1], which is not provided. The schema need not
    /// exist yet.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if `v` is not a character string \[SR1\]; -301 if its
    /// value is not a `<schema name>` (SQLSTATE 3F000) \[GR2\], or names
    /// another catalog (3D000).
    pub(super) fn set_schema(&self, st: &State, v: &ValueSpecification, params: &Params) -> Result<()> {
        // [ISO/IEC 9075:1992 §17.6 GR5u] a `?` is CHARACTER VARYING.
        let (value, data_type) = binder::value_specification_as(v, &self.auth(), params, ParameterType::Varying)?;
        if !data_type.is_character() {
            return fail(Code::SyntaxRule, "[ISO/IEC 9075:1992 §16.2 SR1] SET SCHEMA takes a character string".to_owned());
        }
        let invalid = || {
            fail_with(
                Code::Data,
                SqlState::INVALID_SCHEMA_NAME,
                format!("[ISO/IEC 9075:1992 §16.2 GR2] {} is not a <schema name>", value.to_literal()),
            )
        };
        let Value::Char(s) = &value else { return invalid() };
        let Ok(tokens) = lex(s.trim_matches(' ')) else { return invalid() };
        let toks: Vec<&Tok> = tokens.iter().map(|t| &t.tok).collect();
        let schema = match toks.as_slice() {
            [Tok::Identifier(schema), Tok::End] => schema,
            [Tok::Identifier(catalog), Tok::Delimiter(delim::PERIOD), Tok::Identifier(schema), Tok::End] => {
                let ours = st.settings.get(crate::settings::CATALOG_NAME).to_string();
                if *catalog != ours.trim_end() {
                    return fail_with(
                        Code::Data,
                        SqlState::INVALID_CATALOG_NAME,
                        format!("[ISO/IEC 9075:1992 §16.2 GR3a] the one catalog is {}, not {catalog}", ours.trim_end()),
                    );
                }
                schema
            }
            _ => return invalid(),
        };
        self.schema.replace(schema.clone());
        Ok(())
    }

    /// [ISO/IEC 9075:1992 §16.1] SET CATALOG `v`: of the one catalog, the
    /// default catalog already; a name of another is no valid `<catalog
    /// name>` [§5.4 SR20] \[GR2\]. It begins no transaction.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if `v` is no character string \[SR1\]; SQLSTATE 3D000
    /// if its value, trimmed, is not the one catalog's name.
    pub(super) fn set_catalog(&self, st: &State, v: &ValueSpecification, params: &Params) -> Result<()> {
        let (value, data_type) = binder::value_specification_as(v, &self.auth(), params, ParameterType::Varying)?;
        if !data_type.is_character() {
            return fail(Code::SyntaxRule, "[ISO/IEC 9075:1992 §16.1 SR1] SET CATALOG takes a character string".to_owned());
        }
        let ours = st.settings.get(crate::settings::CATALOG_NAME).to_string();
        let named = |s: &str| {
            lex(s.trim_matches(' '))
                .ok()
                .and_then(|t| if let [one, end] = t.as_slice() { (end.tok == Tok::End).then(|| one.tok.clone()) } else { None })
        };
        if let Value::Char(s) = &value
            && named(s) == Some(Tok::Identifier(ours.trim_end().to_owned()))
        {
            return Ok(());
        }
        fail_with(
            Code::Data,
            SqlState::INVALID_CATALOG_NAME,
            format!("[ISO/IEC 9075:1992 §16.1 GR2, §5.4 SR20] {} is not the one catalog, {}", value.to_literal(), ours.trim_end()),
        )
    }

    /// [ISO/IEC 9075:1992 §16.4] SET SESSION AUTHORIZATION `v`: the
    /// session's authorization identifier the identifier `v`'s value,
    /// trimmed, writes. A session sets the one it began with; one begun
    /// by the database's owner, any other (RULES.md D43) \[GR4\]. It begins
    /// no transaction.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if `v` is not a character string \[SR1\]; SQLSTATE
    /// 25000 in a transaction \[GR1\]; 28000 if its value is no
    /// `<authorization identifier>` \[GR3\], or the session was not begun
    /// by the owner and names another \[GR5\].
    pub(super) fn set_session_authorization(&self, v: &ValueSpecification, params: &Params) -> Result<()> {
        let (value, data_type) = binder::value_specification_as(v, &self.auth(), params, ParameterType::Varying)?;
        if !data_type.is_character() {
            return fail(Code::SyntaxRule, "[ISO/IEC 9075:1992 §16.4 SR1] SET SESSION AUTHORIZATION takes a character string".to_owned());
        }
        if self.transaction().is_some() {
            return fail(Code::TransactionState, "[ISO/IEC 9075:1992 §16.4 GR1] SET SESSION AUTHORIZATION in a transaction".to_owned());
        }
        let refused = |why: String| fail_with(Code::Privilege, SqlState::INVALID_AUTHORIZATION, why);
        let named = if let Value::Char(s) = &value { authorization_identifier(s.trim_matches(' ')).ok() } else { None };
        let Some(name) = named else {
            return refused(format!("[ISO/IEC 9075:1992 §16.4 GR3] {} is not an <authorization identifier>", value.to_literal()));
        };
        let owner = self.db.owner()?;
        if name != self.started && owner.as_deref() != Some(self.started.as_str()) {
            let msg = format!(
                "[ISO/IEC 9075:1992 §16.4 GR5] a session begun by {} sets no authorization identifier but its own: only the database's owner's does",
                self.started
            );
            return refused(msg);
        }
        self.auth.replace(name);
        Ok(())
    }

    /// \[EXTENSION\] SET of a setting (RULES.md D36): this session's own
    /// value of the setting `name`, one a session may set, the value of
    /// `value`, or, for none (DEFAULT), the database's again. Like SET
    /// SCHEMA it begins no transaction, and ROLLBACK does not undo it; it
    /// takes effect as the setting says (`Effect`).
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if there is no such setting, or it is the database's
    /// alone; -301 if the value is not of its kind or range.
    pub(super) fn set_named(&self, st: &State, name: &str, value: Option<&ValueSpecification>, params: &Params) -> Result<()> {
        let settings = &st.settings;
        let Some(definition) = settings.key(name).and_then(|k| settings.definition(k)) else {
            return fail(Code::SyntaxRule, format!("[DESIGN] there is no setting {name}"));
        };
        if !definition.per_session {
            let msg = format!("[DESIGN] {} is the database's, which a session does not set for itself", definition.name);
            return fail(Code::SyntaxRule, msg);
        }
        let given = value.map(|v| binder::value_specification(v, &self.auth(), params)).transpose()?;
        if let Some((v, _)) = &given {
            crate::settings::accepted(&definition.name, &definition.kind, v).map_err(|e| SqlError { code: Code::Data, ..e })?;
        }
        self.own.borrow_mut().set(settings, name, given.as_ref().map(|(v, _)| v))
    }

    /// \[EXTENSION\] SET DATABASE (RULES.md D36, D43): the database's value
    /// of the setting `name`, any setting, the value of `value`, or, for
    /// none (DEFAULT), its default again; by the owner, outside a
    /// transaction, as `Database::set_setting` sets it, kept with the
    /// definitions.
    ///
    /// # Errors
    ///
    /// Those of `as_owner`; SQLCODE -103 if there is no such setting, or
    /// it is fixed once the database is created; -301 if the value is not
    /// of its kind or range, or names what is not registered; the other
    /// errors of `Database::set_setting`.
    pub(super) fn set_database(&self, st: &State, name: &str, value: Option<&ValueSpecification>, params: &Params) -> Result<()> {
        self.as_owner("SET DATABASE")?;
        let settings = &st.settings;
        let Some(definition) = settings.key(name).and_then(|k| settings.definition(k)) else {
            return fail(Code::SyntaxRule, format!("[DESIGN] there is no setting {name}"));
        };
        if definition.effect == crate::settings::Effect::Creation && st.created {
            return fail(Code::SyntaxRule, format!("[DESIGN] {} is fixed when the database is created", definition.name));
        }
        let Some(given) = value else { return self.db.reset_setting(&definition.name) };
        let (v, _) = binder::value_specification(given, &self.auth(), params)?;
        crate::settings::accepted(&definition.name, &definition.kind, &v).map_err(|e| SqlError { code: Code::Data, ..e })?;
        // A value the database refuses, as a name nothing is registered
        // under, is the statement's data exception, not the API's misuse.
        self.db.set_setting(&definition.name, &v).map_err(|e| if e.code == Code::Api { SqlError { code: Code::Data, ..e } } else { e })
    }

    /// \[DESIGN\] What cancels this session's statements (RULES.md D44): a
    /// handle any thread may hold, whose `Stop::cancel` stops the statement
    /// running then, or the fetch of its cursor, which fails (SQLCODE -920)
    /// and is undone; its transaction goes on.
    pub fn canceller(&self) -> Arc<Stop> {
        Arc::clone(&self.stop)
    }

    /// [ISO/IEC 9075:1992 §4.30] The default schema of its unqualified
    /// names: its authorization identifier's until SET SCHEMA sets another.
    pub fn default_schema(&self) -> String {
        self.schema.borrow().clone()
    }
}

//! [ISO/IEC 9075:1992 §17.12-§17.20] Dynamic cursors: a `<dynamic declare
//! cursor>`'s, declared through `Session::declare_cursor` as any cursor
//! is, of a `<statement name>`; one an `<allocate cursor statement>`
//! allocates, of an `<extended cursor name>`; their OPEN, FETCH and CLOSE,
//! and the positioned DELETE and UPDATE through them. Each is kept with
//! the session's other cursors, its specification the statement prepared
//! for it, read as it is opened [§17.12 GR1, §17.13 GR3].

use super::{Name, dynamic_error};
use crate::ast::{DynamicName, ExtendedName, FetchOrientation, SetClause, TableName, UsingClause};
use crate::binder::{WriteKind, bind_write};
use crate::db::{Completion, DeclaredCursor, Session, StatementContext, rule};
use crate::error::{Result, SqlState};
use crate::lexer::{Tok, delim, lex};
use crate::params::Params;
use crate::value::DataType;

/// [ISO/IEC 9075:1992 §17.13 GR4, GR5; §17.14 GR2] No cursor of the
/// extended name `name` is allocated, or one is, as `rule` says.
fn invalid_cursor(rule: &str, msg: &str) -> crate::error::SqlError {
    dynamic_error(SqlState::INVALID_CURSOR_NAME, format!("[ISO/IEC 9075:1992 {rule}] {msg}"))
}

/// [ISO/IEC 9075:1992 §17.12] The `<statement name>` of a `<dynamic declare
/// cursor>`, whose `<cursor specification>`, as `declare_cursor` keeps it,
/// is `specification`: an `<identifier>` alone, which no `<cursor
/// specification>` is.
pub(crate) fn declared_statement(specification: &str) -> Option<String> {
    let tokens = lex(specification).ok()?;
    let words: Vec<&Tok> = tokens.iter().map(|t| &t.tok).filter(|t| **t != Tok::Delimiter(delim::SEMICOLON)).collect();
    match words.as_slice() {
        [Tok::Identifier(name), Tok::End] => Some(name.clone()),
        _ => None,
    }
}

impl Session {
    /// The cursor `name` names: a `<cursor name>`'s, or an extended one's,
    /// allocated \[`rule`\].
    fn cursor_key(&self, name: &DynamicName, params: &Params, rule: &str) -> Result<Name> {
        let key = self.dynamic_name(name, params, (SqlState::INVALID_CURSOR_NAME, rule))?;
        if key.scope.is_some() && !self.inner.borrow().cursors.contains_key(&key) {
            return Err(invalid_cursor(rule, &format!("no cursor '{key}' is allocated")));
        }
        Ok(key)
    }

    /// [ISO/IEC 9075:1992 §17.13] ALLOCATE CURSOR: a cursor of the
    /// extended name, INSENSITIVE and SCROLL as written, of the statement
    /// prepared as the extended statement name \[GR6\].
    ///
    /// # Errors
    ///
    /// SQLSTATE 26000 if no statement of that name is prepared \[GR1\];
    /// 07005 if it is not a `<cursor specification>` \[GR2\]; 34000 if the
    /// cursor's name is no `<identifier>` \[GR4\] or a cursor's allocated
    /// \[GR5\].
    pub(super) fn allocate_cursor(
        &self,
        cursor: &ExtendedName,
        (insensitive, scroll): (bool, bool),
        statement: &ExtendedName,
        params: &Params,
    ) -> Result<Completion> {
        let of = self.extended(statement, params, (SqlState::INVALID_STATEMENT_NAME, "§17.13 GR1"))?;
        if !self.prepared_as(&of, "§17.13 GR1")?.is_query() {
            let msg = format!("[ISO/IEC 9075:1992 §17.13 GR2] statement {of} is not a <cursor specification>");
            return Err(dynamic_error(SqlState::NOT_A_CURSOR_SPECIFICATION, msg));
        }
        let key = self.extended(cursor, params, (SqlState::INVALID_CURSOR_NAME, "§17.13 GR4"))?;
        let mut inner = self.inner.borrow_mut();
        if inner.cursors.contains_key(&key) {
            return Err(invalid_cursor("§17.13 GR5", &format!("cursor '{key}' is allocated already")));
        }
        let allocated =
            DeclaredCursor { specification: String::new(), insensitive, scroll, updatable: None, open: None, statement: Some(of) };
        inner.cursors.insert(key, allocated);
        Ok(Completion::Success)
    }

    /// [ISO/IEC 9075:1992 §17.14] A dynamic OPEN, of the cursor `cursor`
    /// names.
    ///
    /// # Errors
    ///
    /// SQLSTATE 34000 if no cursor of the extended name is allocated
    /// \[GR2\]; as `open_dynamic`.
    pub(super) fn dynamic_open(
        &self,
        cx: StatementContext<'_>,
        cursor: &DynamicName,
        using: Option<&UsingClause>,
        params: &Params,
    ) -> Result<Completion> {
        let key = self.cursor_key(cursor, params, "§17.14 GR2")?;
        self.open_dynamic(cx, &key, using, params)
    }

    /// [ISO/IEC 9075:1992 §17.14] OPEN of the dynamic cursor `key`, with the
    /// values `using` gives its statement's parameters \[GR5\], as OPEN
    /// opens a cursor \[GR6\].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if no cursor of the name is declared, or is a dynamic
    /// cursor [SR1]; 24000 if it is open; SQLSTATE 26000 if its statement
    /// is not prepared \[GR1\]; 07004 if it has parameters, and no `<using clause>`
    /// is written \[GR3\]; those of §17.9 and of OPEN.
    pub(crate) fn open_dynamic(
        &self,
        cx: StatementContext<'_>,
        key: &Name,
        using: Option<&UsingClause>,
        params: &Params,
    ) -> Result<Completion> {
        let (statement, flags) = {
            let inner = self.inner.borrow();
            let c = inner.cursors.get(key).ok_or_else(|| rule(format!("[X3.135-1986 §8.8 SR1] no cursor {key} is declared")))?;
            let Some(statement) = c.statement.clone() else {
                return Err(rule(format!(
                    "[ISO/IEC 9075:1992 §17.14 SR1] cursor {key} is not a dynamic cursor: it takes no <using clause>"
                )));
            };
            (statement, (c.insensitive, c.scroll))
        };
        // A query: PREPARE refuses another of a declared cursor's name
        // [§17.6 GR9b ii], ALLOCATE CURSOR one for another [§17.13 GR2].
        let prepared = self.prepared_as(&statement, "§17.14 GR1")?;
        let types: Vec<DataType> = prepared.parameters().iter().map(|t| t.data_type()).collect();
        let values = self.using_values(using, &types, params, "§17.14 GR3")?;
        self.open_text(cx, key, (prepared.sql(), flags), &Params::executing(values))
    }

    /// [ISO/IEC 9075:1992 §17.15] A dynamic FETCH: the cursor moved as
    /// `orientation` says, its row put where `using` says [§17.9 GR6, GR7].
    ///
    /// # Errors
    ///
    /// SQLSTATE 34000 if no cursor of the extended name is allocated; those
    /// of §17.9 and of FETCH.
    pub(super) fn dynamic_fetch(
        &self,
        cx: StatementContext<'_>,
        cursor: &DynamicName,
        (orientation, using): (&FetchOrientation, &UsingClause),
        params: &mut Params,
    ) -> Result<Completion> {
        let key = self.cursor_key(cursor, params, "§17.15 SR4")?;
        let (targets, fetched) =
            self.fetch_row(cx, (&key, orientation), params, |columns, p| self.using_targets(using, columns, p, ("§8.6 GR8", "§8.6 GR10")))?;
        match fetched {
            None => Ok(Completion::NoData),
            Some((columns, row)) => self.put_row(&targets, &columns, &row, params),
        }
    }

    /// [ISO/IEC 9075:1992 §17.16] CLOSE of an extended cursor name.
    ///
    /// # Errors
    ///
    /// SQLSTATE 34000 if no cursor of the name is allocated; those of CLOSE.
    pub(super) fn dynamic_close(&self, cursor: &ExtendedName, params: &Params) -> Result<Completion> {
        let key = self.cursor_key(&DynamicName::Extended(cursor.clone()), params, "§17.16 SR2")?;
        self.close(&key)
    }

    /// [ISO/IEC 9075:1992 §17.17] DELETE of the row the extended cursor is
    /// on, as a positioned DELETE deletes it \[GR1\].
    ///
    /// # Errors
    ///
    /// SQLSTATE 34000 if no cursor of the name is allocated; those of the
    /// positioned DELETE.
    pub(super) fn dynamic_delete(
        &self,
        cx: StatementContext<'_>,
        table: &TableName,
        cursor: &ExtendedName,
        params: &Params,
    ) -> Result<Completion> {
        let key = self.cursor_key(&DynamicName::Extended(cursor.clone()), params, "§17.17 SR2")?;
        let w = self.bound(cx.st, params, |b| bind_write(b, WriteKind::DeletePositioned, table, &[], None))?;
        self.settle_cursors(cx.st, &w.table);
        self.delete_positioned(cx, (&key, cursor.at), &w)
    }

    /// [ISO/IEC 9075:1992 §17.18] UPDATE of the row the extended cursor is
    /// on, as a positioned UPDATE updates it \[GR1\].
    ///
    /// # Errors
    ///
    /// SQLSTATE 34000 if no cursor of the name is allocated; those of the
    /// positioned UPDATE.
    pub(super) fn dynamic_update(
        &self,
        cx: StatementContext<'_>,
        (table, set): (&TableName, &[SetClause]),
        cursor: &ExtendedName,
        params: &Params,
    ) -> Result<Completion> {
        let key = self.cursor_key(&DynamicName::Extended(cursor.clone()), params, "§17.18 SR2")?;
        let w = self.bound(cx.st, params, |b| bind_write(b, WriteKind::UpdatePositioned, table, set, None))?;
        self.settle_cursors(cx.st, &w.table);
        self.update_positioned(cx, (&key, cursor.at), &w)
    }
}

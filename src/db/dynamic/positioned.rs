//! [ISO/IEC 9075:1992 §17.6 GR2, §17.19, §17.20] A positioned DELETE or
//! UPDATE prepared: the cursor its `<cursor name>` names, a dynamic
//! cursor's or an extended one's, found as it is prepared; and, if it
//! names no table, the table of that cursor's specification.

use super::statements::not_prepared;
use super::{Name, dynamic_error};
use crate::ast::{Scope, TableName};
use crate::binder;
use crate::catalog::QualifiedName;
use crate::db::prepared::Preparer;
use crate::db::{CursorRows, OpenCursor, Session, State, rule};
use crate::error::{Result, SqlState};
use crate::lexer::{Tok, Token, delim, kw};
use crate::parser::Parser;
use crate::settings::FUNCTION_CALLS;

/// Where the table of a cursor's specification is found.
enum Specification {
    /// Of an updatable cursor open: the table it changes.
    Open(QualifiedName),
    /// Else its `<cursor specification>`'s text: the statement prepared for
    /// it, or as it was declared.
    Text(String),
}

/// The `<table name>` of the table `named`, as a statement would write it.
fn written(named: QualifiedName) -> TableName {
    TableName { auth: Some(named.auth), ident: named.ident, at: 0 }
}

impl Session {
    /// [ISO/IEC 9075:1992 §17.6 GR2, §17.11 GR2, §17.19 SR1, §17.20 SR1]
    /// Of a statement being prepared, made of `tokens`, a positioned DELETE
    /// or UPDATE: the cursor its `<cursor name>` names; and, if it names no
    /// table, the table of that cursor's specification.
    ///
    /// # Errors
    ///
    /// SQLSTATE 34000 if no cursor of the name is declared or allocated
    /// \[GR2b\]; 3C000 if more than one is \[GR2a\]; of one that names no
    /// table, 26000 if the cursor's statement is not prepared, SQLCODE -103
    /// if its specification reads no updatable table [§17.19 SR2, §13.6
    /// SR3]. \[DESIGN\] Prepared by the API, `by`, one that names its table
    /// and no cursor of whose name is there is taken, its cursor found as
    /// it runs, as it was before SQL text prepared statements (RULES.md
    /// D58).
    pub(in crate::db) fn positioned_reference(
        &self,
        st: &State,
        tokens: &[Token],
        by: Preparer,
    ) -> Result<(Option<Name>, Option<TableName>)> {
        let words: Vec<&Tok> = tokens.iter().map(|t| &t.tok).filter(|t| **t != Tok::Delimiter(delim::SEMICOLON)).collect();
        let positioned = matches!(words.first(), Some(Tok::KeyWord(kw::DELETE | kw::UPDATE)));
        let [.., Tok::KeyWord(kw::CURRENT), Tok::KeyWord(kw::OF), Tok::Identifier(cursor), Tok::End] = words.as_slice() else {
            return Ok((None, None));
        };
        if !positioned {
            return Ok((None, None));
        }
        let no_table = matches!(words.get(1), Some(Tok::KeyWord(kw::WHERE | kw::SET)));
        let (key, specification) = match self.dynamic_cursor_named(cursor) {
            Ok(found) => found,
            Err(e) => {
                let missing = e.sqlstate() == SqlState::INVALID_CURSOR_NAME;
                return if missing && by == Preparer::Api && !no_table { Ok((None, None)) } else { Err(e) };
            }
        };
        if !no_table {
            return Ok((Some(key), None));
        }
        let text = match specification? {
            Specification::Open(named) => return Ok((Some(key), Some(written(named)))),
            Specification::Text(text) => text,
        };
        let spec = Parser::for_prepare(&text, st.settings.is_on(FUNCTION_CALLS))?.in_catalog(&st.catalog_name()).cursor_specification()?;
        let named = binder::updatable_in(&spec, &*st.catalog, &self.default_schema()).ok_or_else(|| {
            rule(format!(
                "[ISO/IEC 9075:1992 §17.19 SR2, §17.20 SR2, §13.6 SR3] cursor {key} is not updatable: it names no table to change"
            ))
        })?;
        Ok((Some(key), Some(written(named))))
    }

    /// [ISO/IEC 9075:1992 §17.6 GR2] The one cursor of the name `name`, one
    /// declared or allocated of either scope, and where its specification's
    /// table is found, if its statement is prepared.
    fn dynamic_cursor_named(&self, name: &str) -> Result<(Name, Result<Specification>)> {
        let inner = self.inner.borrow();
        let found: Vec<(Name, Result<Specification>)> = [None, Some(Scope::Local), Some(Scope::Global)]
            .into_iter()
            .map(|scope| Name { scope, name: name.to_owned() })
            .filter_map(|key| {
                let c = inner.cursors.get(&key)?;
                let specification = match (&c.open, &c.statement) {
                    (Some(OpenCursor { rows: CursorRows::Updatable(u), .. }), _) => Ok(Specification::Open(u.named.clone())),
                    (_, Some(s)) => inner
                        .dynamic
                        .statements
                        .get(s)
                        .map(|p| Specification::Text(p.sql().to_owned()))
                        .ok_or_else(|| not_prepared(s, "§17.14 GR1")),
                    (_, None) => Ok(Specification::Text(c.specification.clone())),
                };
                Some((key, specification))
            })
            .collect();
        let mut found = found.into_iter();
        match (found.next(), found.next()) {
            (Some(one), None) => Ok(one),
            (None, _) => Err(dynamic_error(
                SqlState::INVALID_CURSOR_NAME,
                format!("[ISO/IEC 9075:1992 §17.6 GR2b] no cursor {name} is declared or allocated"),
            )),
            (Some(_), Some(_)) => Err(dynamic_error(
                SqlState::AMBIGUOUS_CURSOR_NAME,
                format!("[ISO/IEC 9075:1992 §17.6 GR2a] more than one cursor, declared or allocated, is named {name}"),
            )),
        }
    }
}

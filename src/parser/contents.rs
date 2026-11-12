//! [ISO/IEC 9075-2:2023 §11.3] The `<table contents source>` of a `<table
//! definition>`: a `<table element list>`, which may hold a `<like
//! clause>`, or an `<as subquery clause>`. INCLUDING, EXCLUDING, DEFAULTS,
//! DATA and GENERATED are `<non-reserved word>`s [§5.2], identifiers
//! elsewhere; LIKE, IDENTITY and NO are reserved.

use std::mem;

use super::Parser;
use crate::ast::{AsSubquery, LikeClause, TableElement};
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};

/// The `<non-reserved word>` of a `<like option>` that copies.
const INCLUDING: &str = "INCLUDING";
/// The `<non-reserved word>` of a `<like option>` that does not.
const EXCLUDING: &str = "EXCLUDING";
/// The `<non-reserved word>` of a `<column default option>`.
const DEFAULTS: &str = "DEFAULTS";
/// The `<non-reserved word>` of a `<generation option>`.
const GENERATED: &str = "GENERATED";
/// The `<non-reserved word>` of `<with or without data>`.
const DATA: &str = "DATA";

/// What a `<table contents source>` holds: its `<table element>`s, or its
/// `<as subquery clause>`.
pub(super) type Contents = (Vec<TableElement>, Option<Box<AsSubquery>>);

impl Parser {
    /// [ISO/IEC 9075-2:2023 §11.3] `<table contents source>` ::= `<table
    /// element list>` | `<as subquery clause>`.
    pub(super) fn table_contents(&mut self) -> Result<Contents> {
        if self.at_as_subquery() {
            return Ok((Vec::new(), Some(Box::new(self.as_subquery_clause()?))));
        }
        Ok((self.table_element_list()?, None))
    }

    /// Whether an `<as subquery clause>` is next: AS, or a `<column name
    /// list>` and then AS, which no `<table element list>` is, as each of
    /// its column definitions writes more than a name.
    fn at_as_subquery(&self) -> bool {
        if self.is_kw(kw::AS) {
            return true;
        }
        if !self.is_delim(delim::LEFT_PAREN) {
            return false;
        }
        let mut n = 1_usize;
        loop {
            if !matches!(self.peek_at(n), Tok::Identifier(_)) {
                return false;
            }
            let after = self.peek_at(n.saturating_add(1));
            if matches!(after, Tok::Delimiter(d) if *d == delim::RIGHT_PAREN) {
                return matches!(self.peek_at(n.saturating_add(2)), Tok::KeyWord(k) if *k == kw::AS);
            }
            if !matches!(after, Tok::Delimiter(d) if *d == delim::COMMA) {
                return false;
            }
            n = n.saturating_add(2);
        }
    }

    /// [ISO/IEC 9075-2:2023 §11.3] `<as subquery clause>` ::= \[ (
    /// `<column name list>` ) \] AS `<table subquery>` `<with or without
    /// data>`, where `<with or without data>` ::= WITH NO DATA | WITH
    /// DATA.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the query has a parameter \[SR4\].
    fn as_subquery_clause(&mut self) -> Result<AsSubquery> {
        let columns = if self.accept_delim(delim::LEFT_PAREN) {
            let mut names = vec![self.column_name()?];
            while self.accept_delim(delim::COMMA) {
                names.push(self.column_name()?);
            }
            self.expect_delim(delim::RIGHT_PAREN)?;
            Some(names)
        } else {
            None
        };
        let at = self.here();
        self.expect_kw(kw::AS)?;
        // <table subquery> ::= <subquery> ::= ( <query expression> ). [SR4]
        // no parameter: a name a host parameter's is a column's here.
        self.expect_delim(delim::LEFT_PAREN)?;
        let (before, params) = (self.dynamic_parameters(), mem::take(&mut self.params));
        let query = self.query_expression();
        self.params = params;
        let query = query?;
        if self.dynamic_parameters() > before {
            return Err(self.rule("[ISO/IEC 9075-2:2023 §11.3 SR4] the query of an <as subquery clause> has no dynamic parameter"));
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        self.expect_kw(kw::WITH)?;
        let with_data = !self.accept_kw(kw::NO);
        if !self.accept_word(DATA) {
            return Err(self.syntax("expected WITH DATA or WITH NO DATA [ISO/IEC 9075-2:2023 §11.3]"));
        }
        Ok(AsSubquery { columns, query, with_data, at })
    }

    /// [ISO/IEC 9075-2:2023 §11.3] `<like clause>` ::= LIKE `<table name>`
    /// \[`<like options>`\], LIKE taken; `<like option>` ::= {INCLUDING |
    /// EXCLUDING} {IDENTITY | DEFAULTS | GENERATED}.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if an option is written twice \[SR9b iv 1\].
    pub(super) fn like_clause(&mut self, at: usize) -> Result<TableElement> {
        let table = self.table_name()?;
        let mut like = LikeClause { table, at, identity: None, defaults: None, generated: None };
        loop {
            let option_at = self.here();
            let including = if self.accept_word(INCLUDING) {
                true
            } else if self.accept_word(EXCLUDING) {
                false
            } else {
                break;
            };
            let (slot, option) = if self.accept_kw(kw::IDENTITY) {
                (&mut like.identity, "<identity option>")
            } else if self.accept_word(DEFAULTS) {
                (&mut like.defaults, "<column default option>")
            } else if self.accept_word(GENERATED) {
                (&mut like.generated, "<generation option>")
            } else {
                return Err(self.syntax("expected IDENTITY, DEFAULTS or GENERATED [ISO/IEC 9075-2:2023 §11.3]"));
            };
            if slot.replace(including).is_some() {
                return Err(crate::error::SqlError::at(
                    crate::error::Code::SyntaxRule,
                    option_at,
                    format!("[ISO/IEC 9075-2:2023 §11.3 SR9b iv 1] a <like clause> writes its {option} once"),
                ));
            }
        }
        Ok(TableElement::Like(like))
    }
}

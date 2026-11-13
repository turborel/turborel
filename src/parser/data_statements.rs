//! The data statements of a procedure [X3.135-1986 §8; ISO/IEC 9075:1992
//! §13]: OPEN, CLOSE, positioned and searched DELETE and UPDATE, and their
//! SET clauses; TRUNCATE TABLE [ISO/IEC 9075-2:2023 §14.10].

use super::Parser;
use super::generated::RESTART;
use crate::ast::{DynamicStatement, SetClause, SqlStatement, UpdateSource, ValueExpression};
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};

/// [ISO/IEC 9075-2:2023 §14.10] The word that begins a `<truncate table
/// statement>`: SQL:2023 reserves it, SQL-92 does not, so it stays a
/// `<regular identifier>` elsewhere.
pub(crate) const TRUNCATE: &str = "TRUNCATE";

/// Whether the tokens `first` and `second` begin a `<truncate table
/// statement>`, not a query: TRUNCATE, then TABLE, which, reserved, no
/// table's name is followed by in a `<joined table>`.
pub(crate) fn begins_truncate(first: Option<&Tok>, second: Option<&Tok>) -> bool {
    matches!((first, second), (Some(Tok::Identifier(w)), Some(Tok::KeyWord(kw::TABLE))) if w == TRUNCATE)
}

impl Parser {
    /// [X3.135-1986 §8.1, §8.8] CLOSE or OPEN `<cursor name>`; [ISO/IEC
    /// 9075:1992 §17.14, §17.16] of an `<extended cursor name>`, or OPEN
    /// with a `<using clause>`.
    pub(super) fn open_or_close(&mut self) -> Result<SqlStatement> {
        let open = self.is_kw(kw::OPEN);
        self.advance();
        if let Some(dynamic) = self.dynamic_open_or_close(open)? {
            return Ok(dynamic);
        }
        let at = self.here();
        let cursor = self.identifier()?;
        if !open {
            return Ok(SqlStatement::Close(cursor));
        }
        Ok(self.dynamic_open_using(&cursor, at)?.unwrap_or(SqlStatement::Open(cursor)))
    }

    /// [X3.135-1986 §8.4, §8.5] DELETE FROM `<table name>`
    ///     {WHERE CURRENT OF `<cursor name>` | \[WHERE `<search condition>`\]}
    pub(super) fn delete_statement(&mut self) -> Result<SqlStatement> {
        self.expect_kw(kw::DELETE)?;
        // [ISO/IEC 9075:1992 §17.19] a <preparable dynamic delete statement:
        // positioned> of no table.
        if self.dynamic.is_some() && self.is_kw(kw::WHERE) {
            let (table, cursor) = self.preparable_positioned()?;
            return Ok(SqlStatement::DeletePositioned { table, cursor });
        }
        self.expect_kw(kw::FROM)?;
        let table = self.table_name()?;
        if !self.accept_kw(kw::WHERE) {
            return Ok(SqlStatement::DeleteSearched { table, where_clause: None });
        }
        if self.accept_kw(kw::CURRENT) {
            self.expect_kw(kw::OF)?;
            if let Some(cursor) = self.dynamic_current_of()? {
                return Ok(SqlStatement::Dynamic(Box::new(DynamicStatement::DeletePositioned { table, cursor })));
            }
            let at = self.here();
            return Ok(SqlStatement::DeletePositioned { table, cursor: (self.identifier()?, at) });
        }
        Ok(SqlStatement::DeleteSearched { table, where_clause: Some(self.search_condition()?) })
    }

    /// The statement a word, no key word, begins, if one does, read to its
    /// end: \[EXTENSION\] CANCEL CONNECTION (`parser/served.rs`), or
    /// TRUNCATE TABLE.
    pub(super) fn word_statement(&mut self) -> Result<Option<SqlStatement>> {
        let stmt = if self.at_cancel_connection() {
            self.served_statement()?
        } else if self.at_truncate() {
            self.truncate_statement()?
        } else {
            return Ok(None);
        };
        self.end()?;
        Ok(Some(stmt))
    }

    /// Whether a `<truncate table statement>` begins here: TRUNCATE, then
    /// TABLE.
    fn at_truncate(&self) -> bool {
        let Tok::Identifier(word) = self.peek() else { return false };
        word == TRUNCATE && self.next_is_kw(kw::TABLE)
    }

    /// [ISO/IEC 9075-2:2023 §14.10] TRUNCATE TABLE `<target table>`
    ///     \[`<identity column restart option>`\], CONTINUE IDENTITY if none
    ///     is written \[SR5\].
    fn truncate_statement(&mut self) -> Result<SqlStatement> {
        self.advance();
        self.expect_kw(kw::TABLE)?;
        let table = self.table_name()?;
        let restart = self.accept_word(RESTART);
        if restart || self.accept_kw(kw::CONTINUE) {
            self.expect_kw(kw::IDENTITY)?;
        }
        Ok(SqlStatement::Truncate { table, restart })
    }

    /// [X3.135-1986 §8.11, §8.12] UPDATE `<table name>` SET `<set clause>` \[{,`<set clause>`}...\]
    ///     {WHERE CURRENT OF `<cursor name>` | \[WHERE `<search condition>`\]}
    pub(super) fn update_statement(&mut self) -> Result<SqlStatement> {
        self.expect_kw(kw::UPDATE)?;
        // [ISO/IEC 9075:1992 §17.20] a <preparable dynamic update statement:
        // positioned> of no table.
        if self.dynamic.is_some() && self.is_kw(kw::SET) {
            let set = self.set_clause_list()?;
            let (table, cursor) = self.preparable_positioned()?;
            return Ok(SqlStatement::UpdatePositioned { table, set, cursor });
        }
        let table = self.table_name()?;
        let set = self.set_clause_list()?;
        if !self.accept_kw(kw::WHERE) {
            return Ok(SqlStatement::UpdateSearched { table, set, where_clause: None });
        }
        if self.accept_kw(kw::CURRENT) {
            self.expect_kw(kw::OF)?;
            if let Some(cursor) = self.dynamic_current_of()? {
                return Ok(SqlStatement::Dynamic(Box::new(DynamicStatement::UpdatePositioned { table, set, cursor })));
            }
            let at = self.here();
            return Ok(SqlStatement::UpdatePositioned { table, set, cursor: (self.identifier()?, at) });
        }
        Ok(SqlStatement::UpdateSearched { table, set, where_clause: Some(self.search_condition()?) })
    }

    /// SET `<set clause>` \[{, `<set clause>`}...\]
    fn set_clause_list(&mut self) -> Result<Vec<SetClause>> {
        self.expect_kw(kw::SET)?;
        let mut set = vec![self.set_clause()?];
        while self.accept_delim(delim::COMMA) {
            set.push(self.set_clause()?);
        }
        Ok(set)
    }

    /// [X3.135-1986 §8.11, §8.12] `<set clause>` ::= `<object column>` =
    /// `<update source>`, which is `<value expression>` | NULL | DEFAULT
    /// [ISO/IEC 9075:1992 §13.9]; `<object column>` ::= `<column name>`;
    /// [ISO/IEC 9075-2:2023 §14.15] `<update target>` ::= `<object column>`
    /// \[`[` `<simple value specification>` `]`\].
    fn set_clause(&mut self) -> Result<SetClause> {
        let at = self.here();
        let column = self.identifier()?;
        let element = if self.accept_delim(delim::LEFT_BRACKET) {
            let index = ValueExpression::Value(self.value_specification()?);
            self.expect_delim(delim::RIGHT_BRACKET)?;
            Some(index)
        } else {
            None
        };
        self.expect_delim(delim::EQUALS)?;
        let value = if self.accept_kw(kw::NULL) {
            UpdateSource::Null
        } else if self.accept_kw(kw::DEFAULT) {
            UpdateSource::Default
        } else {
            UpdateSource::Value(self.value_expression()?)
        };
        Ok(SetClause { column, element, value, at })
    }
}

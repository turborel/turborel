//! The data statements of a procedure [X3.135-1986 §8; ISO/IEC 9075:1992
//! §13]: OPEN, CLOSE, positioned and searched DELETE and UPDATE, and their
//! SET clauses. Split from parser.rs (RULES.md R-SIZE-2); the methods extend
//! the same `Parser`.

use super::Parser;
use crate::ast::{SetClause, SqlStatement, UpdateSource};
use crate::error::Result;
use crate::lexer::{delim, kw};

impl Parser {
    /// [X3.135-1986 §8.1, §8.8] CLOSE or OPEN `<cursor name>`; [ISO/IEC
    /// 9075:1992 §17.14, §17.16] of an `<extended cursor name>`, or OPEN
    /// with a `<using clause>`, refused.
    pub(super) fn open_or_close(&mut self) -> Result<SqlStatement> {
        let open = self.is_kw(kw::OPEN);
        self.advance();
        if let Some(refused) = self.dynamic_open_or_close(open)? {
            return Ok(refused);
        }
        let cursor = self.identifier()?;
        if !open {
            return Ok(SqlStatement::Close(cursor));
        }
        Ok(if self.dynamic_open_using()? { SqlStatement::Commit } else { SqlStatement::Open(cursor) })
    }

    /// [X3.135-1986 §8.4, §8.5] DELETE FROM `<table name>`
    ///     {WHERE CURRENT OF `<cursor name>` | \[WHERE `<search condition>`\]}
    pub(super) fn delete_statement(&mut self) -> Result<SqlStatement> {
        self.expect_kw(kw::DELETE)?;
        // [ISO/IEC 9075:1992 §17.19] a <preparable dynamic delete statement:
        // positioned> of no table: refused.
        if self.dynamic.is_some() && self.is_kw(kw::WHERE) {
            return self.preparable_positioned();
        }
        self.expect_kw(kw::FROM)?;
        let table = self.table_name()?;
        if !self.accept_kw(kw::WHERE) {
            return Ok(SqlStatement::DeleteSearched { table, where_clause: None });
        }
        if self.accept_kw(kw::CURRENT) {
            self.expect_kw(kw::OF)?;
            if self.dynamic_current_of()? {
                return Ok(SqlStatement::Commit);
            }
            return Ok(SqlStatement::DeletePositioned { table, cursor: self.identifier()? });
        }
        Ok(SqlStatement::DeleteSearched { table, where_clause: Some(self.search_condition()?) })
    }

    /// [X3.135-1986 §8.11, §8.12] UPDATE `<table name>` SET `<set clause>` \[{,`<set clause>`}...\]
    ///     {WHERE CURRENT OF `<cursor name>` | \[WHERE `<search condition>`\]}
    pub(super) fn update_statement(&mut self) -> Result<SqlStatement> {
        self.expect_kw(kw::UPDATE)?;
        // [ISO/IEC 9075:1992 §17.20] a <preparable dynamic update statement:
        // positioned> of no table: refused.
        if self.dynamic.is_some() && self.is_kw(kw::SET) {
            self.advance();
            self.set_clause()?;
            while self.accept_delim(delim::COMMA) {
                self.set_clause()?;
            }
            return self.preparable_positioned();
        }
        let table = self.table_name()?;
        self.expect_kw(kw::SET)?;
        let mut set = vec![self.set_clause()?];
        while self.accept_delim(delim::COMMA) {
            set.push(self.set_clause()?);
        }
        if !self.accept_kw(kw::WHERE) {
            return Ok(SqlStatement::UpdateSearched { table, set, where_clause: None });
        }
        if self.accept_kw(kw::CURRENT) {
            self.expect_kw(kw::OF)?;
            if self.dynamic_current_of()? {
                return Ok(SqlStatement::Commit);
            }
            return Ok(SqlStatement::UpdatePositioned { table, set, cursor: self.identifier()? });
        }
        Ok(SqlStatement::UpdateSearched { table, set, where_clause: Some(self.search_condition()?) })
    }

    /// [X3.135-1986 §8.11, §8.12] `<set clause>` ::= `<object column>` =
    /// `<update source>`, which is `<value expression>` | NULL | DEFAULT
    /// [ISO/IEC 9075:1992 §13.9]; `<object column>` ::= `<column name>`
    fn set_clause(&mut self) -> Result<SetClause> {
        let at = self.here();
        let column = self.identifier()?;
        self.expect_delim(delim::EQUALS)?;
        let value = if self.accept_kw(kw::NULL) {
            UpdateSource::Null
        } else if self.accept_kw(kw::DEFAULT) {
            UpdateSource::Default
        } else {
            UpdateSource::Value(self.value_expression()?)
        };
        Ok(SetClause { column, value, at })
    }
}

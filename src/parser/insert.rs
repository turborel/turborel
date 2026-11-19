//! The `<insert statement>` [X3.135-1986 §8.7].

use super::Parser;
use crate::ast::{InsertSource, InsertStatement};
use crate::error::Result;
use crate::lexer::{delim, kw};

impl Parser {
    /// INSERT \[WITH `<XML lexically scoped options>`\] INTO `<table name>`
    /// {\[(`<insert column list>`)\] \[`<override clause>`\] `<query
    /// expression>` | DEFAULT VALUES} [ISO/IEC 9075:1992 §13.8; ISO/IEC
    /// 9075-2:2023 §14.11; ISO/IEC 9075-14:2006 draft §14.4]
    ///
    /// # Errors
    ///
    /// The text is not an `<insert statement>`.
    pub fn insert_statement(&mut self) -> Result<InsertStatement> {
        self.expect_kw(kw::INSERT)?;
        let xml = self.with_scoped_options()?;
        self.expect_kw(kw::INTO)?;
        let table = self.table_name()?;
        let columns = if self.is_delim(delim::LEFT_PAREN) { Some(self.insert_column_list()?) } else { None };
        // [ISO/IEC 9075-2:2023 §14.11] [<override clause>]
        let overriding = self.override_clause()?;
        // [ISO/IEC 9075:1992 §13.8] DEFAULT VALUES, with no column list.
        if columns.is_none() && overriding.is_none() && self.accept_kw(kw::DEFAULT) {
            self.expect_kw(kw::VALUES)?;
            self.end()?;
            return Ok(InsertStatement { table, columns, overriding, source: InsertSource::DefaultValues, xml });
        }
        let query = self.query_expression()?;
        // [ISO/IEC 9075-2:2023 §15.11] whose statements run before the
        // insert's own query is read (RULES.md D189).
        query.admit_deltas();
        self.end()?;
        Ok(InsertStatement { table, columns, overriding, source: InsertSource::Query(Box::new(query)), xml })
    }
}

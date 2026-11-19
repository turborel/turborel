//! [ISO/IEC 9075-2:2023 §14.12] The `<merge statement>`: MERGE INTO a
//! target table, USING a table reference, ON a search condition, and its
//! WHEN clauses, matched and not matched, in any order and number
//! \[Features F312, F313, F314\].
//!
//! The binder checks the Syntax Rules. This module reads the Format alone,
//! so a source that is a joined table is read here and refused there
//! \[SR11\].

use super::Parser;
use crate::ast::{MatchedAction, MergeInsert, MergeStatement, MergeWhen, SqlStatement, ValuesRow};
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};

/// [ISO/IEC 9075-2:2023 §14.12] The word after WHEN in a `<merge when
/// matched clause>` and after WHEN NOT in a `<merge when not matched
/// clause>`: a `<non-reserved word>` of SQL:2023, so no key word here.
const MATCHED: &str = "MATCHED";

impl Parser {
    /// [ISO/IEC 9075-2:2023 §14.12] `<merge statement>` ::= MERGE
    /// \[WITH `<XML lexically scoped options>`\] [ISO/IEC 9075-14:2006
    /// draft §14.5] INTO
    /// `<target table>`, a `<table name>` or ONLY ( `<table name>` ), \[\[AS\] `<merge correlation name>`\] USING `<table
    /// reference>` ON `<search condition>` `<merge operation
    /// specification>`, from MERGE, which begins here, to the end of its
    /// last `<merge when clause>`.
    pub(super) fn merge_statement(&mut self) -> Result<SqlStatement> {
        // MERGE, an identifier the caller has seen begin the statement;
        // [ISO/IEC 9075-14:2006 draft §14.5] then WITH's options, if written.
        self.advance();
        let xml = self.with_scoped_options()?;
        self.expect_kw(kw::INTO)?;
        let (target, only) = self.target_table()?;
        let correlation = self.merge_correlation()?;
        self.expect_kw(kw::USING)?;
        let source = self.table_reference()?;
        // [ISO/IEC 9075-2:2023 §14.12] whose statement runs before the
        // merge reads its source (RULES.md D175).
        if let crate::ast::TableReference::Delta(d) = &source {
            d.rows.admit();
        }
        self.expect_kw(kw::ON)?;
        let on = self.search_condition()?;
        // <merge operation specification> ::= <merge when clause>...: one
        // or more, matched and not matched in any order.
        let mut clauses = vec![self.merge_when_clause()?];
        while self.is_kw(kw::WHEN) {
            clauses.push(self.merge_when_clause()?);
        }
        Ok(SqlStatement::Merge(Box::new(MergeStatement { target, only, correlation, source, on, clauses, property: None, xml })))
    }

    /// \[\[AS\] `<merge correlation name>`\], if written, and where it
    /// begins. With no AS, a name is any `<identifier>` at hand, since
    /// USING, which follows otherwise, is reserved.
    fn merge_correlation(&mut self) -> Result<Option<(String, usize)>> {
        let at = self.here();
        if self.accept_kw(kw::AS) {
            let named_at = self.here();
            return Ok(Some((self.identifier()?, named_at)));
        }
        let named = match self.peek() {
            Tok::Identifier(_) | Tok::Introducer(_) | Tok::LargeObjectLength(_) => true,
            Tok::KeyWord(_)
            | Tok::ExactNumeric(_)
            | Tok::ApproximateNumeric(_)
            | Tok::CharacterString(_)
            | Tok::BinaryString(_)
            | Tok::Delimiter(_)
            | Tok::End => false,
        };
        if !named {
            return Ok(None);
        }
        Ok(Some((self.identifier()?, at)))
    }

    /// `<merge when clause>` ::= `<merge when matched clause>` | `<merge
    /// when not matched clause>`, from WHEN.
    fn merge_when_clause(&mut self) -> Result<MergeWhen> {
        let at = self.here();
        self.expect_kw(kw::WHEN)?;
        let not = self.accept_kw(kw::NOT);
        self.expect_word(MATCHED)?;
        let condition = if self.accept_kw(kw::AND) { Some(self.search_condition()?) } else { None };
        self.expect_kw(kw::THEN)?;
        if not {
            let insert = self.merge_insert_specification()?;
            return Ok(MergeWhen::NotMatched { condition, insert, at });
        }
        let action = self.merge_update_or_delete()?;
        Ok(MergeWhen::Matched { condition, action, at })
    }

    /// `<merge update or delete specification>` ::= UPDATE SET `<set clause
    /// list>` | DELETE.
    fn merge_update_or_delete(&mut self) -> Result<MatchedAction> {
        if self.accept_kw(kw::DELETE) {
            return Ok(MatchedAction::Delete);
        }
        if !self.accept_kw(kw::UPDATE) {
            return Err(self.syntax("expected UPDATE or DELETE"));
        }
        Ok(MatchedAction::Update(self.set_clause_list()?))
    }

    /// `<merge insert specification>` ::= INSERT \[(`<insert column
    /// list>`)\] \[`<override clause>`\] VALUES `<merge insert value
    /// list>`.
    fn merge_insert_specification(&mut self) -> Result<MergeInsert> {
        let at = self.here();
        if !self.accept_kw(kw::INSERT) {
            return Err(self.syntax("expected INSERT"));
        }
        let columns = if self.is_delim(delim::LEFT_PAREN) { Some(self.insert_column_list()?) } else { None };
        let overriding = self.override_clause()?;
        self.expect_kw(kw::VALUES)?;
        let values = self.merge_insert_value_list()?;
        Ok(MergeInsert { columns, overriding, values, at })
    }

    /// `<merge insert value list>` ::= ( `<merge insert value element>`
    /// \[{, `<merge insert value element>`}...\] ). Each element is a
    /// `<value expression>` or a `<contextually typed value
    /// specification>`, as an element of a row of a `<table value
    /// constructor>` is, so it is read as one. Unlike such a row, the list
    /// is always in parentheses, even of one element.
    fn merge_insert_value_list(&mut self) -> Result<ValuesRow> {
        let at = self.here();
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut elements = vec![self.row_element()?];
        while self.accept_delim(delim::COMMA) {
            elements.push(self.row_element()?);
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(ValuesRow { elements, at })
    }
}

#[cfg(test)]
mod tests;

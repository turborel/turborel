//! The `<search or cycle clause>` of a `<with list element>` [ISO/IEC
//! 9075-2:2023 §7.18]: a `<search clause>`, a `<cycle clause>`, or both
//! in that order.

use super::Parser;
use crate::ast::{CycleClause, SearchClause, SearchOrder};
use crate::error::Result;
use crate::lexer::{delim, kw};

// [ISO/IEC 9075-2:2023 §5.2, §7.18] Words of a <search or cycle clause>
// that SQL-92 does not reserve: identifiers, read as these only there.
/// SEARCH, which begins a `<search clause>`.
pub(super) const SEARCH: &str = "SEARCH";
/// CYCLE, which begins a `<cycle clause>`.
pub(super) const CYCLE: &str = "CYCLE";
/// DEPTH, of DEPTH FIRST.
const DEPTH: &str = "DEPTH";
/// BREADTH, of BREADTH FIRST.
const BREADTH: &str = "BREADTH";

impl Parser {
    /// [ISO/IEC 9075-2:2023 §7.18] `<search or cycle clause>` ::= `<search
    /// clause>` | `<cycle clause>` | `<search clause>` `<cycle clause>`:
    /// the clauses written, none if neither is next.
    pub(super) fn search_or_cycle_clause(&mut self) -> Result<(Option<SearchClause>, Option<CycleClause>)> {
        let search = if self.is_word(SEARCH) { Some(self.search_clause()?) } else { None };
        let cycle = if self.is_word(CYCLE) { Some(self.cycle_clause()?) } else { None };
        Ok((search, cycle))
    }

    /// [ISO/IEC 9075-2:2023 §7.18] `<search clause>` ::= SEARCH `<recursive
    /// search order>` SET `<sequence column>`, `<recursive search order>`
    /// ::= {DEPTH | BREADTH} FIRST BY `<column name list>`.
    fn search_clause(&mut self) -> Result<SearchClause> {
        let at = self.here();
        self.advance();
        let order = if self.accept_word(DEPTH) {
            SearchOrder::Depth
        } else if self.accept_word(BREADTH) {
            SearchOrder::Breadth
        } else {
            return Err(self.syntax("expected DEPTH or BREADTH [ISO/IEC 9075-2:2023 §7.18]"));
        };
        self.expect_kw(kw::FIRST)?;
        self.expect_kw(kw::BY)?;
        let columns = self.column_names()?;
        self.expect_kw(kw::SET)?;
        let sequence = self.identifier()?;
        Ok(SearchClause { order, columns, sequence, at })
    }

    /// [ISO/IEC 9075-2:2023 §7.18] `<cycle clause>` ::= CYCLE `<cycle
    /// column list>` SET `<cycle mark column>` \[TO `<cycle mark value>`
    /// DEFAULT `<non-cycle mark value>`\] USING `<path column>`.
    fn cycle_clause(&mut self) -> Result<CycleClause> {
        let at = self.here();
        self.advance();
        let columns = self.column_names()?;
        self.expect_kw(kw::SET)?;
        let mark = self.identifier()?;
        let marks = if self.accept_kw(kw::TO) {
            let to = self.value_expression()?;
            self.expect_kw(kw::DEFAULT)?;
            Some((to, self.value_expression()?))
        } else {
            None
        };
        self.expect_kw(kw::USING)?;
        let path = self.identifier()?;
        Ok(CycleClause { columns, mark, marks, path, at })
    }

    /// `<column name>` \[{, `<column name>`}...\], with no parentheses.
    fn column_names(&mut self) -> Result<Vec<String>> {
        let mut names = vec![self.identifier()?];
        while self.accept_delim(delim::COMMA) {
            names.push(self.identifier()?);
        }
        Ok(names)
    }
}

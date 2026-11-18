//! [ISO/IEC 9075-2:2023 §7.17] What follows a `<query expression body>`:
//! its `<order by clause>`, `<result offset clause>` and `<fetch first
//! clause>`. OFFSET, PERCENT, ROW and TIES, which SQL-92 does not reserve,
//! are these words only here. \[EXTENSION\] APPROX and EXACT, a target
//! accuracy and vector hints are the vector design document's
//! (`ordered/approx.rs`).

mod approx;

use crate::ast::{FetchFirst, FetchQuantity, OrderedQuery, QueryExpression, RowCount, SortSpecification};
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};
use crate::logical::Search;
use crate::parser::Parser;
use crate::parser::window::{ROW, TIES};

/// OFFSET, before an `<offset row count>`.
const OFFSET: &str = "OFFSET";
/// PERCENT, after the value of a `<fetch first percentage>`.
const PERCENT: &str = "PERCENT";
/// How far past OFFSET ROW or ROWS is, after a count of one token.
const UNSIGNED_UNIT: usize = 2;
/// How far past OFFSET ROW or ROWS is, after a sign and a count.
const SIGNED_UNIT: usize = 3;

impl Parser {
    /// `body`, then its \[`<order by clause>`\] \[`<result offset
    /// clause>`\] \[`<fetch first clause>`\]: as it is if none is written.
    pub(in crate::parser) fn ordered(&mut self, body: QueryExpression) -> Result<QueryExpression> {
        let order_by = self.order_by_clause()?;
        let offset = self.result_offset_clause()?;
        let fetch = self.fetch_first_clause()?;
        if order_by.is_empty() && offset.is_none() && fetch.is_none() {
            return Ok(body);
        }
        Ok(QueryExpression::Ordered(Box::new(OrderedQuery { body, order_by, offset, fetch })))
    }

    /// `<order by clause>` ::= ORDER BY `<sort specification list>`: its
    /// sort specifications, none if it is not written.
    fn order_by_clause(&mut self) -> Result<Vec<SortSpecification>> {
        let mut order_by = Vec::new();
        if self.accept_kw(kw::ORDER) {
            self.expect_kw(kw::BY)?;
            order_by.push(self.sort_specification()?);
            while self.accept_delim(delim::COMMA) {
                order_by.push(self.sort_specification()?);
            }
        }
        Ok(order_by)
    }

    /// Whether a `<result offset clause>` begins here: OFFSET, a value,
    /// signed or not, then ROW or ROWS. Otherwise OFFSET is a name, a
    /// correlation name for one.
    pub(in crate::parser) fn at_result_offset(&self) -> bool {
        if !self.is_word(OFFSET) {
            return false;
        }
        let signed = matches!(self.peek_at(1), Tok::Delimiter(d) if *d == delim::PLUS || *d == delim::MINUS);
        let unit = if signed { SIGNED_UNIT } else { UNSIGNED_UNIT };
        matches!(self.peek_at(unit), Tok::KeyWord(k) if *k == kw::ROWS) || matches!(self.peek_at(unit), Tok::Identifier(w) if w == ROW)
    }

    /// `<result offset clause>` ::= OFFSET `<offset row count>` {ROW |
    /// ROWS}, if written.
    fn result_offset_clause(&mut self) -> Result<Option<RowCount>> {
        if !self.at_result_offset() {
            return Ok(None);
        }
        self.advance();
        let count = self.row_count()?;
        self.row_or_rows()?;
        Ok(Some(count))
    }

    /// `<fetch first clause>` ::= FETCH {FIRST | NEXT} \[`<fetch first
    /// quantity>`\] {ROW | ROWS} {ONLY | WITH TIES}, if written.
    fn fetch_first_clause(&mut self) -> Result<Option<FetchFirst>> {
        let at = self.here();
        if !self.accept_kw(kw::FETCH) {
            return Ok(None);
        }
        let mut search = self.search_mode();
        if !self.accept_kw(kw::FIRST) {
            self.expect_kw(kw::NEXT)?;
        }
        let quantity = if self.is_kw(kw::ROWS) || self.is_word(ROW) {
            None
        } else {
            let count = self.row_count()?;
            Some(if self.accept_word(PERCENT) { FetchQuantity::Percent(count) } else { FetchQuantity::Rows(count) })
        };
        self.row_or_rows()?;
        let ties = if self.accept_kw(kw::WITH) {
            self.expect_word(TIES)?;
            true
        } else {
            self.expect_kw(kw::ONLY)?;
            false
        };
        let target = self.target_accuracy_clause()?;
        let accuracy_at = target.map(|(_, place)| place);
        if let (Search::Approximate(asked), Some((written, _))) = (&mut search, target) {
            *asked = written;
        }
        let hints = self.vector_hints()?;
        Ok(Some(FetchFirst { quantity, ties, at, search, accuracy_at, hints }))
    }

    /// An `<offset row count>`, `<fetch first row count>` or the value of a
    /// `<fetch first percentage>`: a `<simple value specification>`, or
    /// \[EXTENSION\] a `<dynamic parameter specification>`, as NTILE's
    /// count is.
    fn row_count(&mut self) -> Result<RowCount> {
        let at = self.here();
        Ok(RowCount { value: self.number_of_rows()?, at })
    }

    /// ROW or ROWS.
    fn row_or_rows(&mut self) -> Result<()> {
        if self.accept_kw(kw::ROWS) {
            return Ok(());
        }
        self.expect_word(ROW)
    }
}

// Unit tests: each clause and each form of its count (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;

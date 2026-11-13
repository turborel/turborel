//! [ISO/IEC 9075-2:2023 §7.13] The `<group by clause>`: its `<set
//! quantifier>` and its `<grouping element list>`, of `<ordinary grouping
//! set>`s, ROLLUP, CUBE, GROUPING SETS and the `<empty grouping set>`.
//! ROLLUP, CUBE and GROUPING, which SQL:2023 reserves and SQL-92 does not,
//! and SETS, which neither reserves, are key words only here, ROLLUP and
//! CUBE before a parenthesis, GROUPING before SETS, so a column may have any
//! of those names.

use super::Parser;
use crate::ast::{GroupBy, GroupingColumn, GroupingElement};
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};

/// The words of a `<group by clause>` that SQL-92 does not reserve.
pub(super) const ROLLUP: &str = "ROLLUP";
pub(super) const CUBE: &str = "CUBE";
pub(super) const GROUPING: &str = "GROUPING";
const SETS: &str = "SETS";

impl Parser {
    /// [ISO/IEC 9075-2:2023 §7.13] `<group by clause>` ::= GROUP BY
    /// \[`<set quantifier>`\] `<grouping element list>`, GROUP BY taken.
    pub(super) fn group_by(&mut self) -> Result<GroupBy> {
        let distinct = self.accept_kw(kw::DISTINCT);
        if !distinct {
            self.accept_kw(kw::ALL);
        }
        Ok(GroupBy { distinct, elements: self.grouping_elements()? })
    }

    /// A `<grouping element list>`, or the `<grouping set list>` of GROUPING
    /// SETS, whose `<grouping set>`s are of the same forms.
    fn grouping_elements(&mut self) -> Result<Vec<GroupingElement>> {
        let mut elements = vec![self.grouping_element()?];
        while self.accept_delim(delim::COMMA) {
            elements.push(self.grouping_element()?);
        }
        Ok(elements)
    }

    /// A `<grouping element>` ::= `<ordinary grouping set>` | `<rollup
    /// list>` | `<cube list>` | `<grouping sets specification>` | `<empty
    /// grouping set>`.
    fn grouping_element(&mut self) -> Result<GroupingElement> {
        let paren = matches!(self.peek_at(1), Tok::Delimiter(delim::LEFT_PAREN));
        if paren && self.is_word(ROLLUP) {
            self.advance();
            return Ok(GroupingElement::Rollup(self.ordinary_sets()?));
        }
        if paren && self.is_word(CUBE) {
            self.advance();
            return Ok(GroupingElement::Cube(self.ordinary_sets()?));
        }
        if self.is_word(GROUPING) && matches!(self.peek_at(1), Tok::Identifier(s) if s == SETS) {
            self.advance();
            self.advance();
            self.expect_delim(delim::LEFT_PAREN)?;
            let sets = self.nested(Parser::grouping_elements)?;
            self.expect_delim(delim::RIGHT_PAREN)?;
            return Ok(GroupingElement::Sets(sets));
        }
        if self.is_delim(delim::LEFT_PAREN) && matches!(self.peek_at(1), Tok::Delimiter(delim::RIGHT_PAREN)) {
            self.advance();
            self.advance();
            return Ok(GroupingElement::Empty);
        }
        Ok(GroupingElement::Ordinary(self.ordinary_set()?))
    }

    /// The parenthesized `<ordinary grouping set list>` of ROLLUP or CUBE.
    fn ordinary_sets(&mut self) -> Result<Vec<Vec<GroupingColumn>>> {
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut sets = vec![self.ordinary_set()?];
        while self.accept_delim(delim::COMMA) {
            sets.push(self.ordinary_set()?);
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(sets)
    }

    /// An `<ordinary grouping set>` ::= `<grouping column reference>` |
    /// ( `<grouping column reference list>` ).
    fn ordinary_set(&mut self) -> Result<Vec<GroupingColumn>> {
        if !self.accept_delim(delim::LEFT_PAREN) {
            return Ok(vec![self.grouping_column()?]);
        }
        let mut columns = vec![self.grouping_column()?];
        while self.accept_delim(delim::COMMA) {
            columns.push(self.grouping_column()?);
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(columns)
    }

    /// [ISO/IEC 9075-2:2023 §7.13] A `<grouping column reference>` ::=
    /// `<column reference>` \[`<collate clause>`\].
    pub(super) fn grouping_column(&mut self) -> Result<GroupingColumn> {
        Ok(GroupingColumn { column: self.column_specification()?, collate: self.collate_clause()? })
    }
}

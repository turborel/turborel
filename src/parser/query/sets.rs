//! [X3.135-1986 §8.3; ISO/IEC 9075:1992 §7.10] The set operators of a
//! `<query expression body>` and a `<query term>` after their first
//! operand, and a `<query expression>` in parentheses.
//!
//! \[DESIGN\] Each is read apart from the production that reads the first
//! operand, which is where a nesting of queries recurs: so the frames each
//! level adds to the stack hold none of their locals, and a statement
//! nested to `MAX_NESTING_DEPTH` is read within half the default stack
//! (RULES.md R-CF-6).

use crate::ast::{Correspondence, QueryExpression, SetOperation, SetOperator};
use crate::error::Result;
use crate::lexer::{delim, kw};
use crate::parser::Parser;

impl Parser {
    /// [X3.135-1986 §8.3; ISO/IEC 9075:1992 §7.10] The rest of a `<query
    /// expression body>` whose first `<query term>` is `left`: UNION or
    /// EXCEPT and another, each.
    pub(super) fn unions(&mut self, mut left: QueryExpression) -> Result<QueryExpression> {
        loop {
            let at = self.here();
            let op = if self.accept_kw(kw::UNION) {
                SetOperator::Union
            } else if self.accept_kw(kw::EXCEPT) {
                SetOperator::Except
            } else {
                return Ok(left);
            };
            self.deepen()?;
            let (all, corresponding) = self.set_quantifiers()?;
            let right = self.query_term()?;
            left = QueryExpression::SetOp(Box::new(SetOperation { op, all, corresponding, left, right, at }));
        }
    }

    /// [ISO/IEC 9075:1992 §7.10] The rest of a `<query term>` whose first
    /// `<query primary>` is `left`: INTERSECT and another, each.
    pub(super) fn intersections(&mut self, mut left: QueryExpression) -> Result<QueryExpression> {
        loop {
            let at = self.here();
            if !self.accept_kw(kw::INTERSECT) {
                return Ok(left);
            }
            self.deepen()?;
            let (all, corresponding) = self.set_quantifiers()?;
            let right = self.query_primary()?;
            left = QueryExpression::SetOp(Box::new(SetOperation { op: SetOperator::Intersect, all, corresponding, left, right, at }));
        }
    }

    /// [ISO/IEC 9075:1992 §7.10] A `<query primary>` of a `<query
    /// expression>` in parentheses.
    pub(super) fn parenthesized_query(&mut self) -> Result<QueryExpression> {
        self.advance();
        let q = self.query_expression()?;
        self.closed_query(q)
    }

    /// `q`, read in parentheses, and the ")" that closes them.
    fn closed_query(&mut self, q: QueryExpression) -> Result<QueryExpression> {
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(q)
    }

    /// \[ALL\] \[`<corresponding spec>`\] after a set operator, where
    /// `<corresponding spec>` ::= CORRESPONDING \[BY (`<corresponding column
    /// list>`)\] [ISO/IEC 9075:1992 §7.10].
    pub(super) fn set_quantifiers(&mut self) -> Result<(bool, Correspondence)> {
        let all = self.accept_kw(kw::ALL);
        if !self.accept_kw(kw::CORRESPONDING) {
            return Ok((all, Correspondence::Position));
        }
        if !self.accept_kw(kw::BY) {
            return Ok((all, Correspondence::Names));
        }
        Ok((all, Correspondence::By(self.name_list()?)))
    }
}

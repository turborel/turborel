//! [ISO/IEC 9075-2:2023 §10.9] The `<filter clause>` of an `<aggregate
//! function>`. FILTER, which SQL:2023 reserves and SQL-92 does not, is
//! read only after a set function's closing parenthesis where `( WHERE`
//! follows it, so a select item may still be named FILTER.

use super::Parser;
use crate::ast::{SetArg, ValueExpression};
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};

/// [ISO/IEC 9075-2:2023 §10.9] FILTER, no key word of SQL-92.
pub(super) const FILTER: &str = "FILTER";

impl Parser {
    /// `primary` with the `<filter clause>` ::= FILTER ( WHERE `<search
    /// condition>` ) that may follow it, if it is an `<aggregate
    /// function>`: a set function but GROUPING, which is none.
    pub(super) fn filtered(&mut self, primary: ValueExpression) -> Result<ValueExpression> {
        let ValueExpression::SetFunction(mut sf) = primary else { return Ok(primary) };
        let follows = self.is_word(FILTER)
            && matches!(self.peek_at(1), Tok::Delimiter(delim::LEFT_PAREN))
            && matches!(self.peek_at(2), Tok::KeyWord(kw::WHERE));
        if !follows || matches!(sf.arg, SetArg::Grouping(_)) {
            return Ok(ValueExpression::SetFunction(sf));
        }
        self.advance();
        self.advance();
        self.advance();
        sf.filter = Some(Box::new(self.search_condition()?));
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(ValueExpression::SetFunction(sf))
    }
}

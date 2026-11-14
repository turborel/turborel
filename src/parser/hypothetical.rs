//! [ISO/IEC 9075-2:2023 §10.9] The `<hypothetical set function>`. The
//! `<rank function type>`s, which SQL:2023 reserves and SQL-92 does not,
//! are its names only where a parenthesis, at least one value and the
//! closing parenthesis are followed by WITHIN GROUP, so a function of one
//! of those names stays callable, and RANK ( ) OVER is the window function.

use super::Parser;
use super::listagg::WITHIN;
use crate::ast::{SetArg, SetFunctionSpecification, ValueExpression};
use crate::error::Result;
use crate::functions::RANK_FUNCTION_TYPES;
use crate::lexer::{Tok, delim, kw};

impl Parser {
    /// The name of a hypothetical set function, if one is at hand: a
    /// `<rank function type>`, then values between parentheses, then
    /// WITHIN GROUP.
    pub(super) fn at_hypothetical(&self) -> Option<&'static str> {
        let Tok::Identifier(name) = self.peek() else { return None };
        let (found, _) = RANK_FUNCTION_TYPES.into_iter().find(|(n, _)| name == n)?;
        let opened = matches!(self.peek_at(1), Tok::Delimiter(delim::LEFT_PAREN));
        if !opened || matches!(self.peek_at(2), Tok::Delimiter(delim::RIGHT_PAREN)) {
            return None;
        }
        let closed = self.closing(1)?;
        let within = matches!(self.peek_at(closed.saturating_add(1)), Tok::Identifier(w) if w == WITHIN);
        (within && matches!(self.peek_at(closed.saturating_add(2)), Tok::KeyWord(kw::GROUP))).then_some(found)
    }

    /// How far ahead the ")" is that closes the "(" `open` places ahead, if
    /// one does. Each token is looked at once.
    fn closing(&self, open: usize) -> Option<usize> {
        let (mut depth, mut at) = (0_usize, open);
        loop {
            match self.peek_at(at) {
                Tok::End => return None,
                Tok::Delimiter(delim::LEFT_PAREN) => depth = depth.saturating_add(1),
                Tok::Delimiter(delim::RIGHT_PAREN) => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        return Some(at);
                    }
                }
                Tok::Delimiter(_)
                | Tok::Identifier(_)
                | Tok::KeyWord(_)
                | Tok::ExactNumeric(_)
                | Tok::ApproximateNumeric(_)
                | Tok::CharacterString(_)
                | Tok::BinaryString(_)
                | Tok::Introducer(_) => {}
            }
            at = at.saturating_add(1);
        }
    }

    /// [ISO/IEC 9075-2:2023 §10.9] `<hypothetical set function>` ::= `<rank
    /// function type>` ( `<hypothetical set function value expression
    /// list>` ) `<within group specification>`, of the function `name` at
    /// hand.
    pub(super) fn hypothetical_set_function(&mut self, name: &'static str) -> Result<ValueExpression> {
        let at = self.here();
        self.advance();
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut values = vec![self.nested(Parser::value_expression)?];
        while self.accept_delim(delim::COMMA) {
            values.push(self.nested(Parser::value_expression)?);
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        let order_by = self.within_group_specification(name)?;
        let arg = SetArg::Hypothetical(values);
        Ok(ValueExpression::SetFunction(SetFunctionSpecification { name, arg, order_by, listagg: None, filter: None, at }))
    }
}

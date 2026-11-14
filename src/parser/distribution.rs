//! [ISO/IEC 9075-2:2023 §10.9] The `<inverse distribution function>`.
//! PERCENTILE_CONT and PERCENTILE_DISC, which SQL:2023 reserves and SQL-92
//! does not, are its names only before a parenthesis, so a column may
//! still be named either.

use super::Parser;
use crate::ast::{SetArg, SetFunctionSpecification, ValueExpression};
use crate::error::Result;
use crate::functions::{PERCENTILE_CONT, PERCENTILE_DISC};
use crate::lexer::{Tok, delim};

impl Parser {
    /// The ordered set function at hand, if one is: a rank function of
    /// values before WITHIN GROUP, a hypothetical set function; or
    /// PERCENTILE_CONT or PERCENTILE_DISC before "(".
    pub(super) fn ordered_set_function(&mut self) -> Option<Result<ValueExpression>> {
        if let Some(name) = self.at_hypothetical() {
            return Some(self.hypothetical_set_function(name));
        }
        let name = self.at_inverse_distribution()?;
        Some(self.inverse_distribution_function(name))
    }

    /// The name of an inverse distribution function, if one is at hand:
    /// its `<inverse distribution function type>`, then "(".
    fn at_inverse_distribution(&self) -> Option<&'static str> {
        let Tok::Identifier(name) = self.peek() else { return None };
        let found = [PERCENTILE_CONT, PERCENTILE_DISC].into_iter().find(|n| name == n)?;
        matches!(self.peek_at(1), Tok::Delimiter(delim::LEFT_PAREN)).then_some(found)
    }

    /// [ISO/IEC 9075-2:2023 §10.9] `<inverse distribution function>` ::=
    /// `<inverse distribution function type>` ( `<inverse distribution
    /// function argument>` ) `<within group specification>`, of the
    /// function `name` at hand.
    fn inverse_distribution_function(&mut self, name: &'static str) -> Result<ValueExpression> {
        let at = self.here();
        self.advance();
        self.expect_delim(delim::LEFT_PAREN)?;
        let fraction = self.nested(Parser::value_expression)?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        let order_by = self.within_group_specification(name)?;
        let arg = SetArg::Fraction(Box::new(fraction));
        Ok(ValueExpression::SetFunction(SetFunctionSpecification { name, arg, order_by, listagg: None, filter: None, at }))
    }
}

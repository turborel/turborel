//! The syntax of Feature T621 [ISO/IEC 9075-2:2023 §6.31, §10.9]: the
//! numeric value functions LN, EXP, POWER, SQRT, FLOOR and CEIL or
//! CEILING, and the set functions STDDEV_POP, STDDEV_SAMP, VAR_POP,
//! VAR_SAMP and the `<binary set function>`s; of Features T441, T622,
//! T623 and T624, ABS, MOD, the `<trigonometric function>`s, LOG and
//! LOG10. Their names, which SQL:2023 reserves and SQL-92 does not, are
//! read as these only before "(", so a column may still be named one.

use super::Parser;
use crate::ast::{SetArg, SetFunctionSpecification, ValueExpression};
use crate::error::Result;
use crate::functions::{Numeric, PAIRED, Statistic};
use crate::lexer::{Tok, delim, kw};

impl Parser {
    /// A numeric value function or a statistical set function, if its
    /// name and "(" are next.
    pub(super) fn numeric_or_statistical(&mut self) -> Option<Result<ValueExpression>> {
        let Tok::Identifier(name) = self.peek() else { return None };
        if !matches!(self.peek_at(1), Tok::Delimiter(delim::LEFT_PAREN)) {
            return None;
        }
        if let Some(numeric) = Numeric::named(name) {
            return Some(self.numeric_function(numeric.name(), numeric.operands()));
        }
        let statistic = Statistic::named(name)?;
        Some(self.statistical_set_function(statistic).map(ValueExpression::SetFunction))
    }

    /// [§6.31] `<natural logarithm>` ::= LN ( `<numeric value
    /// expression>` ), and alike EXP, SQRT, FLOOR, { CEIL | CEILING }, ABS,
    /// LOG10 and each `<trigonometric function name>`; `<power function>`
    /// ::= POWER ( `<numeric value expression base>` , `<numeric value
    /// expression exponent>` ), and alike MOD and LOG: the function `name`
    /// of `operands` operands.
    fn numeric_function(&mut self, name: &'static str, operands: usize) -> Result<ValueExpression> {
        let at = self.here();
        self.advance();
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut args = vec![self.value_expression()?];
        while args.len() < operands {
            self.expect_delim(delim::COMMA)?;
            args.push(self.value_expression()?);
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(ValueExpression::Function { name, args, at })
    }

    /// [§10.9] `<general set function>` ::= `<set function type>` (
    /// `<value expression>` ) of STDDEV_POP, STDDEV_SAMP, VAR_POP or
    /// VAR_SAMP, which takes no `<set quantifier>` \[SR2\]; or `<binary
    /// set function>` ::= `<binary set function type>` ( `<dependent
    /// variable expression>` , `<independent variable expression>` ), its
    /// argument the pair of them.
    fn statistical_set_function(&mut self, statistic: Statistic) -> Result<SetFunctionSpecification> {
        let at = self.here();
        self.advance();
        self.expect_delim(delim::LEFT_PAREN)?;
        if !statistic.binary() && (self.is_kw(kw::DISTINCT) || self.is_kw(kw::ALL)) {
            return Err(self.rule(format!(
                "[ISO/IEC 9075-2:2023 §10.9 SR2] {} takes no <set quantifier>: neither DISTINCT nor ALL",
                statistic.name()
            )));
        }
        let first = self.value_expression()?;
        let argument = if statistic.binary() {
            self.expect_delim(delim::COMMA)?;
            let second = self.value_expression()?;
            ValueExpression::Function { name: PAIRED, args: vec![first, second], at }
        } else {
            first
        };
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(SetFunctionSpecification {
            name: statistic.name(),
            arg: SetArg::All(Box::new(argument)),
            order_by: Vec::new(),
            listagg: None,
            json: None,
            filter: None,
            semantics: None,
            returning: None,
            at,
        })
    }
}

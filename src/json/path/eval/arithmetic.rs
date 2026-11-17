//! The signs and arithmetic operators of an SQL/JSON path expression
//! evaluated [ISO/IEC 9075-2:2023 §9.46 GR11h, GR11i, GR11j]: computed as
//! SQL computes, by the `<numeric value expression>`s and MOD the rules
//! name (`value::arith`, `Value::negate`, `eval/numbers.rs`).
//!
//! [GR11j] As printed, the rule of `<JSON additive expression>` unwraps
//! neither operand in lax mode, as GR11i does those of `<JSON
//! multiplicative expression>`; it is followed as printed, so `lax $ + 1`
//! of `[1]` is an exception.

use super::{Evaluator, exception, item, number, numbers, singleton_number};
use crate::error::{Result, SqlState};
use crate::json::Item;
use crate::json::path::ast::{Expr, Operator, Sign};
use crate::value::{ArithOp, arith};

impl Evaluator<'_> {
    /// [§9.46 GR11h] `<JSON unary expression>`: `sign` applied to each item
    /// `operand` gives, each a number.
    pub(super) fn signed(&self, sign: Sign, operand: &Expr) -> Result<Vec<Item>> {
        let base = self.lax_unwrap(self.wff(operand)?);
        if base.iter().any(|each| number(each).is_none()) {
            return Err(exception(
                SqlState::SQL_JSON_NUMBER_NOT_FOUND,
                "[ISO/IEC 9075-2:2023 §9.46 GR11h ii] an operand of a sign is not a number",
            ));
        }
        match sign {
            Sign::Plus => Ok(base),
            Sign::Minus => Ok(base.iter().filter_map(number).map(|n| item(n.value().negate())).collect()),
        }
    }

    /// [§9.46 GR11i, GR11j] `<JSON multiplicative expression>` or `<JSON
    /// additive expression>`: `op` applied to the numbers `left` and
    /// `right` give, a singleton numeric value each.
    pub(super) fn arithmetic(&self, left: &Expr, op: Operator, right: &Expr) -> Result<Vec<Item>> {
        let (first, second) = (self.wff(left)?, self.wff(right)?);
        let (first, second, cited) = match op {
            Operator::Multiply | Operator::Divide | Operator::Modulo => (self.lax_unwrap(first), self.lax_unwrap(second), "GR11i iii 1"),
            Operator::Add | Operator::Subtract => (first, second, "GR11j ii 1"),
        };
        let (Some(x), Some(y)) = (singleton_number(&first), singleton_number(&second)) else {
            return Err(exception(
                SqlState::SINGLETON_SQL_JSON_ITEM_REQUIRED,
                &format!("[ISO/IEC 9075-2:2023 §9.46 {cited}] an operand of an arithmetic operator is not a singleton numeric value"),
            ));
        };
        let result = match op {
            Operator::Multiply => arith(ArithOp::Mul, &x.value(), &y.value())?,
            Operator::Divide => arith(ArithOp::Div, &x.value(), &y.value())?,
            Operator::Modulo => numbers::modulo(x, y)?,
            Operator::Add => arith(ArithOp::Add, &x.value(), &y.value())?,
            Operator::Subtract => arith(ArithOp::Sub, &x.value(), &y.value())?,
        };
        Ok(vec![item(result)])
    }
}

#[cfg(test)]
mod tests;

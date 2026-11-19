//! The `<case expression>` [ISO/IEC 9075:1992 §6.9; ISO/IEC 9075-2:2023
//! §6.12]: NULLIF, COALESCE, and CASE, simple or searched.

use super::Parser;
use crate::ast::{CaseExpression, Predicate, SearchCondition, ValueExpression, WhenOperand};
use crate::error::Result;
use crate::lexer::{delim, kw};

impl Parser {
    /// [ISO/IEC 9075:1992 §6.9] `<case expression>`: NULLIF, COALESCE, or
    /// CASE ... END, simple or searched.
    pub(super) fn case_expression(&mut self) -> Result<CaseExpression> {
        if self.accept_kw(kw::NULLIF) {
            self.expect_delim(delim::LEFT_PAREN)?;
            let first = self.value_expression()?;
            self.expect_delim(delim::COMMA)?;
            let second = self.value_expression()?;
            self.expect_delim(delim::RIGHT_PAREN)?;
            return Ok(CaseExpression::NullIf(first, second));
        }
        if self.accept_kw(kw::COALESCE) {
            // COALESCE (<value expression> { , <value expression> }...): two at least.
            self.expect_delim(delim::LEFT_PAREN)?;
            let mut values = vec![self.value_expression()?];
            self.expect_delim(delim::COMMA)?;
            values.push(self.value_expression()?);
            while self.accept_delim(delim::COMMA) {
                values.push(self.value_expression()?);
            }
            self.expect_delim(delim::RIGHT_PAREN)?;
            return Ok(CaseExpression::Coalesce(values));
        }
        self.expect_kw(kw::CASE)?;
        let case = if self.is_kw(kw::WHEN) {
            let mut whens = Vec::new();
            while self.accept_kw(kw::WHEN) {
                let condition = self.search_condition()?;
                self.expect_kw(kw::THEN)?;
                whens.push((condition, self.result()?));
            }
            CaseExpression::Searched { whens, otherwise: self.else_clause()? }
        } else {
            let operand = self.case_operand()?;
            let mut whens = Vec::new();
            self.expect_kw(kw::WHEN)?;
            loop {
                // [ISO/IEC 9075-2:2023 §6.12] `<when operand list>`: one
                // `<when operand>` or more, separated by commas.
                let mut list = vec![self.when_operand(&operand)?];
                while self.accept_delim(delim::COMMA) {
                    list.push(self.when_operand(&operand)?);
                }
                self.expect_kw(kw::THEN)?;
                whens.push((list, self.result()?));
                if !self.accept_kw(kw::WHEN) {
                    break;
                }
            }
            overlapping_alike(&whens).map_err(|m| self.rule(m))?;
            CaseExpression::Simple { operand, whens, otherwise: self.else_clause()? }
        };
        self.expect_kw(kw::END)?;
        Ok(case)
    }

    /// `<result>` ::= `<result expression>` | NULL: none for NULL.
    fn result(&mut self) -> Result<Option<ValueExpression>> {
        if self.accept_kw(kw::NULL) { Ok(None) } else { self.value_expression().map(Some) }
    }

    /// \[`<else clause>`\] ::= ELSE `<result>`: none if NULL or not written.
    fn else_clause(&mut self) -> Result<Option<ValueExpression>> {
        if self.accept_kw(kw::ELSE) { self.result() } else { Ok(None) }
    }
}

/// [ISO/IEC 9075-2:2023 §6.12 SR2c] Of a simple CASE's `whens`, each
/// `<when operand>` an `<overlaps predicate part 2>` or none is: an
/// `<overlaps predicate part 1>` is compared by OVERLAPS alone, and a
/// `<row value predicand>` never by it.
fn overlapping_alike(whens: &[(Vec<WhenOperand>, Option<ValueExpression>)]) -> std::result::Result<(), &'static str> {
    let overlaps = |w: &WhenOperand| match w {
        WhenOperand::Condition(SearchCondition::Predicate(p, _)) => matches!(**p, Predicate::Overlaps(..)),
        WhenOperand::Condition(_) | WhenOperand::Value(..) => false,
    };
    let (some, all) = whens.iter().flat_map(|(list, _)| list).fold((false, true), |(some, all), w| {
        let o = overlaps(w);
        (some || o, all && o)
    });
    if some && !all {
        return Err("[ISO/IEC 9075-2:2023 §6.12 SR2c] a simple CASE's <when operand>s are each OVERLAPS or none is");
    }
    Ok(())
}

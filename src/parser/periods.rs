//! The grammar of periods [ISO/IEC 9075-2:2023 §11.3, §11.27, §11.28]:
//! PERIOD FOR, a `<non-reserved word>` here as every word after SQL-92's
//! is (`lexer.rs`), told from a column named PERIOD by the FOR after it.
//! And the `<period predicate>`s [§8.20]: PERIOD ( start , end ), told
//! from a column by the parenthesis after it, and EQUALS, CONTAINS,
//! PRECEDES, SUCCEEDS and IMMEDIATELY, each an operator only before what
//! may begin an operand, so a column or an alias may still be so named.

use super::Parser;
use crate::ast::{PeriodDefinition, PeriodName, PeriodOperand, PeriodOperator, Portion, Predicate, SYSTEM_TIME};
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};

/// The `<non-reserved word>` that begins a period's specification.
const PERIOD: &str = "PERIOD";

// [ISO/IEC 9075-2:2023 §8.20] The words of the `<period predicate>`s but
// OVERLAPS, a key word of SQL-92.
/// `<period equals predicate part 2>`.
const EQUALS: &str = "EQUALS";
/// `<period contains predicate part 2>`.
const CONTAINS: &str = "CONTAINS";
/// `<period precedes predicate part 2>`.
const PRECEDES: &str = "PRECEDES";
/// `<period succeeds predicate part 2>`.
const SUCCEEDS: &str = "SUCCEEDS";
/// The immediately preceding and succeeding predicates' first word.
const IMMEDIATELY: &str = "IMMEDIATELY";

/// [ISO/IEC 9075-2:2023 §14.9, §14.14] The word after FOR of FOR PORTION OF.
const PORTION: &str = "PORTION";

impl Parser {
    /// Whether PERIOD FOR is next: a `<system or application time period
    /// specification>`.
    pub(super) fn at_period(&self) -> bool {
        matches!(self.peek(), Tok::Identifier(w) if w == PERIOD) && self.next_is_kw(kw::FOR)
    }

    /// [ISO/IEC 9075-2:2023 §11.3] `<system or application time period
    /// specification>` ::= PERIOD FOR SYSTEM_TIME | PERIOD FOR
    /// `<application time period name>`, at PERIOD.
    ///
    /// # Errors
    ///
    /// SQLCODE -102 if no period's name follows.
    pub(super) fn period_name(&mut self) -> Result<PeriodName> {
        self.accept_word(PERIOD);
        self.expect_kw(kw::FOR)?;
        if self.accept_word(SYSTEM_TIME) {
            return Ok(PeriodName::SystemTime);
        }
        Ok(PeriodName::Application(self.identifier()?))
    }

    /// [ISO/IEC 9075-2:2023 §11.3] `<table period definition>` ::= `<system
    /// or application time period specification>` ( `<period begin column
    /// name>` , `<period end column name>` ), at PERIOD.
    ///
    /// # Errors
    ///
    /// SQLCODE -102 if it is not written so.
    pub(super) fn period_definition(&mut self) -> Result<PeriodDefinition> {
        let at = self.here();
        let name = self.period_name()?;
        self.expect_delim(delim::LEFT_PAREN)?;
        let start_at = self.here();
        let start = (self.identifier()?, start_at);
        self.expect_delim(delim::COMMA)?;
        let end_at = self.here();
        let end = (self.identifier()?, end_at);
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(PeriodDefinition { name, start, end, at })
    }

    /// Whether PERIOD ( is next: a `<period predicand>` written.
    pub(super) fn at_written_period(&self) -> bool {
        matches!(self.peek(), Tok::Identifier(w) if w == PERIOD) && matches!(self.peek_at(1), Tok::Delimiter(d) if *d == delim::LEFT_PAREN)
    }

    /// Whether a `<period predicate part 2>` is next after one value:
    /// OVERLAPS, or a word of `period_word` before an operand.
    pub(super) fn at_period_operator(&self) -> bool {
        match self.peek() {
            Tok::KeyWord(w) => *w == kw::OVERLAPS,
            Tok::Identifier(w) => self.period_word(w),
            Tok::Delimiter(_)
            | Tok::ExactNumeric(_)
            | Tok::ApproximateNumeric(_)
            | Tok::CharacterString(_)
            | Tok::BinaryString(_)
            | Tok::Introducer(_)
            | Tok::LargeObjectLength(_)
            | Tok::End => false,
        }
    }

    /// Whether `word`, next, begins a `<period predicate part 2>`: EQUALS,
    /// CONTAINS, PRECEDES or SUCCEEDS before an operand, or IMMEDIATELY
    /// before PRECEDES or SUCCEEDS.
    fn period_word(&self, word: &str) -> bool {
        if word == IMMEDIATELY {
            return matches!(self.peek_at(1), Tok::Identifier(w) if w == PRECEDES || w == SUCCEEDS);
        }
        [EQUALS, CONTAINS, PRECEDES, SUCCEEDS].contains(&word) && self.period_operand_after()
    }

    /// Whether what follows the word next may begin an operand: as after
    /// MEMBER, or a datetime literal or value function.
    fn period_operand_after(&self) -> bool {
        let datetime = [kw::DATE, kw::TIME, kw::TIMESTAMP, kw::CURRENT_DATE, kw::CURRENT_TIME, kw::CURRENT_TIMESTAMP];
        self.operand_after() || matches!(self.peek_at(1), Tok::KeyWord(w) if datetime.contains(w))
    }

    /// [ISO/IEC 9075-2:2023 §8.20] PERIOD ( `<period start value>` ,
    /// `<period end value>` ).
    ///
    /// # Errors
    ///
    /// SQLCODE -102 if it is not written so.
    pub(super) fn written_period(&mut self) -> Result<PeriodOperand> {
        self.accept_word(PERIOD);
        self.expect_delim(delim::LEFT_PAREN)?;
        let start = self.common_value_expression()?;
        self.expect_delim(delim::COMMA)?;
        let end = self.common_value_expression()?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(PeriodOperand::Written(Box::new(start), Box::new(end)))
    }

    /// [ISO/IEC 9075-2:2023 §8.20] `<period predicate>`, after its
    /// `<period predicand 1>` `left`: its part 2, whose operand is PERIOD (
    /// start , end ) or a value.
    ///
    /// # Errors
    ///
    /// SQLCODE -102 if it is not written so.
    pub(super) fn period_predicate(&mut self, left: PeriodOperand) -> Result<Predicate> {
        let op = self.period_operator()?;
        let right =
            if self.at_written_period() { self.written_period()? } else { PeriodOperand::Value(Box::new(self.common_value_expression()?)) };
        Ok(Predicate::Period(op, left, right))
    }

    /// The operator of a `<period predicate part 2>`.
    fn period_operator(&mut self) -> Result<PeriodOperator> {
        if self.accept_kw(kw::OVERLAPS) {
            return Ok(PeriodOperator::Overlaps);
        }
        let immediately = self.accept_word(IMMEDIATELY);
        let words = [
            (PRECEDES, PeriodOperator::Precedes, PeriodOperator::ImmediatelyPrecedes),
            (SUCCEEDS, PeriodOperator::Succeeds, PeriodOperator::ImmediatelySucceeds),
        ];
        for (word, op, immediate) in words {
            if self.accept_word(word) {
                return Ok(if immediately { immediate } else { op });
            }
        }
        if immediately {
            return Err(self.syntax("expected PRECEDES or SUCCEEDS after IMMEDIATELY"));
        }
        if self.accept_word(EQUALS) {
            return Ok(PeriodOperator::Equals);
        }
        self.expect_word(CONTAINS)?;
        Ok(PeriodOperator::Contains)
    }

    /// [ISO/IEC 9075-2:2023 §14.9, §14.14] FOR PORTION OF `<application
    /// time period name>` FROM `<point in time 1>` TO `<point in time 2>`,
    /// if FOR begins it.
    pub(super) fn for_portion(&mut self) -> Result<Option<Box<Portion>>> {
        let at = self.here();
        if !self.accept_kw(kw::FOR) {
            return Ok(None);
        }
        self.expect_word(PORTION)?;
        self.expect_kw(kw::OF)?;
        let period = self.identifier()?;
        self.expect_kw(kw::FROM)?;
        let from = self.value_expression()?;
        self.expect_kw(kw::TO)?;
        let to = self.value_expression()?;
        Ok(Some(Box::new(Portion { period, from, to, at })))
    }
}

/// [ISO/IEC 9075-2:2023 §11.3 SR2e v 2 B, §11.27 GR2b] The period
/// constraint of a period of start column `start` and end column `end`,
/// named `name`: CONSTRAINT `name` CHECK ( `start` < `end` ), each name
/// delimited.
///
/// # Errors
///
/// None that a name can cause: each is delimited, its quotes doubled.
pub(crate) fn period_constraint(name: &str, (start, end): (&str, &str)) -> Result<crate::ast::TableConstraint> {
    let quoted = |n: &str| format!("\"{}\"", n.replace('"', "\"\""));
    let text = format!("CONSTRAINT {} CHECK ({} < {})", quoted(name), quoted(start), quoted(end));
    let mut parser = Parser::new(&text)?;
    let constraint = parser.table_constraint()?;
    parser.end()?;
    Ok(constraint)
}

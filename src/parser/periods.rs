//! The grammar of periods [ISO/IEC 9075-2:2023 §11.3, §11.27, §11.28]:
//! PERIOD FOR, a `<non-reserved word>` here as every word after SQL-92's
//! is (`lexer.rs`), told from a column named PERIOD by the FOR after it.

use super::Parser;
use crate::ast::{PeriodDefinition, PeriodName};
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};

/// The `<non-reserved word>` that begins a period's specification.
const PERIOD: &str = "PERIOD";

/// [ISO/IEC 9075-2:2023 §11.3] The name of the system-time period.
const SYSTEM_TIME: &str = "SYSTEM_TIME";

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

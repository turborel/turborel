//! [ISO/IEC 9075-2:2023 §14.17, §14.18] The locator statements, FREE
//! LOCATOR and HOLD LOCATOR (Feature T561), read as words: SQL:2023
//! reserves neither FREE nor HOLD, SQL-92 does not, so each stays a
//! `<regular identifier>` elsewhere.

use super::Parser;
use crate::ast::SqlStatement;
use crate::error::Result;
use crate::lexer::{Tok, delim};

/// The word that begins a `<free locator statement>`.
pub(crate) const FREE: &str = "FREE";
/// The word that begins a `<hold locator statement>`.
pub(crate) const HOLD: &str = "HOLD";
/// The word after either.
const LOCATOR: &str = "LOCATOR";

/// Whether the tokens `first` and `second` begin a locator statement:
/// FREE or HOLD, then LOCATOR.
pub(crate) fn begins_locator_statement(first: Option<&Tok>, second: Option<&Tok>) -> bool {
    matches!((first, second), (Some(Tok::Identifier(w)), Some(Tok::Identifier(l))) if (w == FREE || w == HOLD) && l == LOCATOR)
}

impl Parser {
    /// Whether a locator statement begins here.
    pub(super) fn at_locator_statement(&self) -> bool {
        begins_locator_statement(Some(self.peek()), self.toks.get(self.pos.saturating_add(1)).map(|t| &t.tok))
    }

    /// [ISO/IEC 9075-2:2023 §14.17, §14.18] FREE LOCATOR or HOLD LOCATOR
    /// `<locator reference>` \[{ , `<locator reference>` }...\]: each a
    /// `<dynamic parameter specification>`, a locator's four-octet integer
    /// [§4.37.5]. No host parameter nor embedded variable is declared a
    /// locator here \[SR1, SR2\].
    pub(super) fn locator_statement(&mut self) -> Result<SqlStatement> {
        let hold = matches!(self.peek(), Tok::Identifier(w) if w == HOLD);
        self.advance();
        self.advance();
        let mut references = Vec::new();
        loop {
            if !self.is_delim(delim::QUESTION) {
                let rule = if hold { "§14.18 SR1" } else { "§14.17 SR1, SR2" };
                return Err(self.rule(format!(
                    "[ISO/IEC 9075-2:2023 {rule}] a <locator reference> is a <dynamic parameter specification>, as no parameter or variable is a locator's"
                )));
            }
            references.push(self.value_specification()?);
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        Ok(SqlStatement::Locator { hold, references })
    }
}

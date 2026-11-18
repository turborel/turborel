//! [ISO/IEC 9075-2:2023 §17.5 to §17.8] The SQL-transaction statements
//! that end a transaction or name a savepoint: COMMIT and ROLLBACK, each
//! with AND \[NO\] CHAIN, ROLLBACK with a `<savepoint clause>`, the
//! `<savepoint statement>` and the `<release savepoint statement>`.
//! SAVEPOINT, RELEASE and CHAIN are no key words of SQL-92, so each is
//! read as a word and stays a `<regular identifier>` elsewhere.

use super::Parser;
use crate::ast::{SavepointStatement, SqlStatement};
use crate::error::{Result, SqlError};
use crate::lexer::{Tok, kw};

/// The word of a `<savepoint statement>`, a `<release savepoint
/// statement>` and a `<savepoint clause>` [§17.5, §17.6, §17.8], and of a
/// `<savepoint level indication>` [§11.60].
pub(crate) const SAVEPOINT: &str = "SAVEPOINT";
/// The word that begins a `<release savepoint statement>` [§17.6].
const RELEASE: &str = "RELEASE";
/// The word of AND \[NO\] CHAIN [§17.7, §17.8].
const CHAIN: &str = "CHAIN";

/// Whether the tokens `first` and `second` begin a savepoint's statement:
/// SAVEPOINT, then a `<savepoint name>`, or RELEASE, then SAVEPOINT.
pub(crate) fn begins_savepoint_statement(first: Option<&Tok>, second: Option<&Tok>) -> bool {
    let (Some(Tok::Identifier(word)), Some(next)) = (first, second) else { return false };
    let named = matches!(next, Tok::Identifier(_));
    let released = next == &Tok::Identifier(SAVEPOINT.to_owned());
    (word == SAVEPOINT && named) || (word == RELEASE && released)
}

impl Parser {
    /// Whether a savepoint's statement begins here.
    pub(super) fn at_savepoint_statement(&self) -> bool {
        begins_savepoint_statement(Some(self.peek()), Some(self.peek_at(1)))
    }

    /// [§17.5] SAVEPOINT `<savepoint name>`, or [§17.6] RELEASE SAVEPOINT
    /// `<savepoint name>`, its first word next.
    pub(super) fn savepoint_statement(&mut self) -> Result<SqlStatement> {
        let release = self.accept_word(RELEASE);
        self.expect_word(SAVEPOINT)?;
        let name = self.identifier()?;
        let statement = if release { SavepointStatement::Release(name) } else { SavepointStatement::Establish(name) };
        Ok(SqlStatement::Savepoint(statement))
    }

    /// [X3.135-1986 §8.2, §8.9] COMMIT WORK or ROLLBACK WORK, next;
    /// [ISO/IEC 9075:1992 §14.3, §14.4] WORK optional; [ISO/IEC
    /// 9075-2:2023 §17.7, §17.8] AND NO CHAIN, implicit if not written
    /// [§17.7 SR1, §17.8 SR2], and of ROLLBACK a `<savepoint clause>`.
    pub(super) fn transaction_end(&mut self) -> Result<SqlStatement> {
        let commit = self.is_kw(kw::COMMIT);
        self.advance();
        self.accept_kw(kw::WORK);
        let chain_at = self.here();
        let chain = self.chain_clause()?;
        let savepoint = if commit { None } else { self.savepoint_clause()? };
        match (chain, savepoint) {
            (true, Some(_)) => {
                Err(SqlError::at(crate::error::Code::SyntaxRule, chain_at, "[ISO/IEC 9075-2:2023 §17.8 SR1] AND CHAIN with TO SAVEPOINT"))
            }
            (true, None) => {
                let rule = if commit { "§17.7 CR1" } else { "§17.8 CR2" };
                let msg = format!(
                    "[ISO/IEC 9075-2:2023 {rule}] AND CHAIN needs Feature T261, “Chained transactions”, which turborel does not support"
                );
                Err(super::routines::not_supported(chain_at, &msg))
            }
            (false, Some(name)) => Ok(SqlStatement::Savepoint(SavepointStatement::RollbackTo(name))),
            (false, None) => Ok(if commit { SqlStatement::Commit } else { SqlStatement::Rollback }),
        }
    }

    /// AND \[NO\] CHAIN, if written: whether AND CHAIN.
    fn chain_clause(&mut self) -> Result<bool> {
        if !self.accept_kw(kw::AND) {
            return Ok(false);
        }
        let no = self.accept_kw(kw::NO);
        self.expect_word(CHAIN)?;
        Ok(!no)
    }

    /// [§17.8] `<savepoint clause>` ::= TO SAVEPOINT `<savepoint
    /// specifier>`, if written: its `<savepoint name>`.
    fn savepoint_clause(&mut self) -> Result<Option<String>> {
        if !self.accept_kw(kw::TO) {
            return Ok(None);
        }
        self.expect_word(SAVEPOINT)?;
        Ok(Some(self.identifier()?))
    }
}

// Unit tests: each form read, AND CHAIN refused, the words still
// identifiers elsewhere (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;

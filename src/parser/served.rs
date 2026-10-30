//! \[EXTENSION\] Statements of the connections a server serves (RULES.md
//! D42): one ends a connection, the other cancels what it runs, each
//! named by its ID in INFORMATION_SCHEMA.CONNECTIONS. ISO/IEC 9075:1992
//! has none. In its notation:
//!
//! ```text
//! <end connection statement> ::=
//!       END CONNECTION <connection identifier>
//! <cancel connection statement> ::=
//!       CANCEL CONNECTION <connection identifier>
//! <connection identifier> ::= <unsigned integer>
//! ```
//!
//! END and CONNECTION are `<reserved word>`s; CANCEL is not one, and stays
//! a `<regular identifier>` everywhere else, matched as a word here alone
//! (as the levels of isolation are, `parser/session.rs`). Split from
//! parser.rs (RULES.md R-SIZE-2).

use super::Parser;
use crate::ast::{ServedStatement, SqlStatement};
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Tok, kw};

/// \[EXTENSION\] The word that begins a `<cancel connection statement>`: a
/// `<regular identifier>` elsewhere, as no standard reserves it.
pub(crate) const CANCEL: &str = "CANCEL";

impl Parser {
    /// Whether a `<cancel connection statement>` begins here: CANCEL, then
    /// CONNECTION.
    pub(super) fn at_cancel_connection(&self) -> bool {
        let Tok::Identifier(word) = self.peek() else { return false };
        word == CANCEL && self.next_is_kw(kw::CONNECTION)
    }

    /// `<end connection statement>` or `<cancel connection statement>`,
    /// as the first word, END or CANCEL (`at_cancel_connection`), says.
    pub(super) fn served_statement(&mut self) -> Result<SqlStatement> {
        let end = self.is_kw(kw::END);
        self.advance();
        self.expect_kw(kw::CONNECTION)?;
        let id = self.connection_identifier()?;
        Ok(SqlStatement::Served(if end { ServedStatement::End { id } } else { ServedStatement::Cancel { id } }))
    }

    /// `<connection identifier>` ::= `<unsigned integer>`: its value, and
    /// where it begins.
    fn connection_identifier(&mut self) -> Result<(u64, usize)> {
        let at = self.here();
        let Tok::ExactNumeric(digits) = self.peek().clone() else {
            return Err(self.syntax("expected a <connection identifier>, an <unsigned integer>"));
        };
        if !digits.chars().all(|c| c.is_ascii_digit()) {
            return Err(self.syntax("expected a <connection identifier>, an <unsigned integer>"));
        }
        let id = digits.parse().map_err(|_| {
            SqlError::at(Code::Unsupported, at, format!("[DESIGN] {digits} is larger than the largest connection identifier, {}", u64::MAX))
        })?;
        self.advance();
        Ok((id, at))
    }
}

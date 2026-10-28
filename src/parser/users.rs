//! \[EXTENSION\] Statements of users (RULES.md D43). ISO/IEC 9075:1992 has
//! none: which identifiers are authorization identifiers is
//! implementation-defined [§5.4; Annex B, item 41]. In its notation:
//!
//! ```text
//! <user definition> ::=
//!       CREATE USER <authorization identifier> [ WITH ] PASSWORD <character string literal>
//! <alter user statement> ::=
//!       ALTER USER <authorization identifier> [ WITH ] PASSWORD <character string literal>
//! <drop user statement> ::=
//!       DROP USER <authorization identifier>
//! ```
//!
//! An `<authorization identifier>` is an `<identifier>` [§5.4]. Split from
//! parser.rs (RULES.md R-SIZE-2).

use super::Parser;
use crate::ast::{SqlStatement, UserStatement};
use crate::error::Result;
use crate::lexer::{Tok, kw};

impl Parser {
    /// `<user definition>`, after CREATE USER.
    pub(super) fn user_definition(&mut self) -> Result<SqlStatement> {
        let name = self.identifier()?;
        let password = self.password()?;
        Ok(SqlStatement::User(UserStatement::Create { name, password }))
    }

    /// `<alter user statement>` or `<drop user statement>`, after ALTER
    /// USER or DROP USER, as `alter` says.
    pub(super) fn alter_or_drop_user(&mut self, alter: bool) -> Result<SqlStatement> {
        let name = self.identifier()?;
        if !alter {
            return Ok(SqlStatement::User(UserStatement::Drop { name }));
        }
        let password = self.password()?;
        Ok(SqlStatement::User(UserStatement::Alter { name, password }))
    }

    /// `[ WITH ] PASSWORD <character string literal>`: the literal's value.
    fn password(&mut self) -> Result<String> {
        self.accept_kw(kw::WITH);
        if !self.accept_word("PASSWORD") {
            return Err(self.syntax("expected PASSWORD"));
        }
        let Tok::CharacterString(password) = self.peek().clone() else {
            return Err(self.syntax("expected a <character string literal>"));
        };
        self.advance();
        Ok(password)
    }
}

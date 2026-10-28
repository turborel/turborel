//! [ISO/IEC 9075:1992 §15, §16.4] The statements of connections, an
//! SQL-agent's, which a client runs (`turborel_client`), and SET SESSION
//! AUTHORIZATION, a session's. Split from parser.rs (RULES.md R-SIZE-2);
//! the methods extend the same `Parser`.

use super::Parser;
use crate::ast::{ConnectionStatement, Disconnected, SqlStatement};
use crate::error::Result;
use crate::lexer::{Tok, kw, lex};

impl Parser {
    /// [ISO/IEC 9075:1992 §15] The connection statement `src` is, if it is
    /// one: CONNECT, SET CONNECTION or DISCONNECT, read whole.
    ///
    /// # Errors
    ///
    /// `src` begins as one and is not one: a syntax error.
    pub fn connection_statement(src: &str) -> Result<Option<ConnectionStatement>> {
        let tokens = lex(src)?;
        let words: Vec<&Tok> = tokens.iter().take(2).map(|t| &t.tok).collect();
        let connection = matches!(
            words.as_slice(),
            [Tok::KeyWord(kw::CONNECT | kw::DISCONNECT), ..] | [Tok::KeyWord(kw::SET), Tok::KeyWord(kw::CONNECTION)]
        );
        if !connection {
            return Ok(None);
        }
        let mut parser = Parser::new(src)?;
        let SqlStatement::Connection(statement) = parser.sql_statement()? else { return Ok(None) };
        Ok(Some(statement))
    }

    /// [ISO/IEC 9075:1992 §15.1] `<connect statement>` ::= CONNECT TO
    /// `<connection target>` ::= `<SQL-server name>` \[AS `<connection
    /// name>`\] \[USER `<user name>`\] | DEFAULT, each name a `<simple value
    /// specification>`.
    pub(super) fn connect_statement(&mut self) -> Result<SqlStatement> {
        self.expect_kw(kw::CONNECT)?;
        self.expect_kw(kw::TO)?;
        if self.accept_kw(kw::DEFAULT) {
            return Ok(SqlStatement::Connection(ConnectionStatement::ConnectDefault));
        }
        let server = self.simple_value_specification()?;
        let name = if self.accept_kw(kw::AS) { Some(self.simple_value_specification()?) } else { None };
        let user = if self.accept_kw(kw::USER) { Some(self.simple_value_specification()?) } else { None };
        Ok(SqlStatement::Connection(ConnectionStatement::Connect { server, name, user }))
    }

    /// [ISO/IEC 9075:1992 §15.2] `<set connection statement>` ::= SET
    /// CONNECTION `<connection object>` ::= DEFAULT | `<connection name>`,
    /// after SET.
    pub(super) fn set_connection(&mut self) -> Result<SqlStatement> {
        self.expect_kw(kw::CONNECTION)?;
        let name = if self.accept_kw(kw::DEFAULT) { None } else { Some(self.simple_value_specification()?) };
        Ok(SqlStatement::Connection(ConnectionStatement::Set(name)))
    }

    /// [ISO/IEC 9075:1992 §15.3] `<disconnect statement>` ::= DISCONNECT
    /// `<disconnect object>` ::= `<connection object>` | ALL | CURRENT.
    pub(super) fn disconnect_statement(&mut self) -> Result<SqlStatement> {
        self.expect_kw(kw::DISCONNECT)?;
        let object = if self.accept_kw(kw::DEFAULT) {
            Disconnected::Default
        } else if self.accept_kw(kw::ALL) {
            Disconnected::All
        } else if self.accept_kw(kw::CURRENT) {
            Disconnected::Current
        } else {
            Disconnected::Named(self.simple_value_specification()?)
        };
        Ok(SqlStatement::Connection(ConnectionStatement::Disconnect(object)))
    }

    /// [ISO/IEC 9075:1992 §16.4] `<set session authorization identifier
    /// statement>` ::= SET SESSION AUTHORIZATION `<value specification>`,
    /// after SET.
    pub(super) fn set_session_authorization(&mut self) -> Result<SqlStatement> {
        self.expect_kw(kw::SESSION)?;
        self.expect_kw(kw::AUTHORIZATION)?;
        Ok(SqlStatement::SetSessionAuthorization(self.value_specification()?))
    }
}

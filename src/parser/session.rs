//! \[EXTENSION\] [ISO/IEC 9075:1992 §14.1, §14.2, §16.2] The SET
//! statements of transactions and sessions: SET TRANSACTION, SET
//! CONSTRAINTS (`parser/constraints.rs`), SET SCHEMA, and SET of a
//! setting (RULES.md D36), whose name is an `<identifier>`, as no standard
//! SET statement's is; and SET DATABASE, of the database's value of a
//! setting, by its owner (D43). In the standard's notation:
//!
//! ```text
//! <set setting statement> ::=
//!       SET <setting name> { <equals operator> | TO } { <value specification> | DEFAULT }
//! <set database setting statement> ::=
//!       SET DATABASE <setting name> { <equals operator> | TO } { <value specification> | DEFAULT }
//! <setting name> ::= <identifier> [ { <period> <identifier> }... ]
//! ```
//!
//! `<equals operator>` is [§5.1]'s, `<value specification>` [§6.2]'s.
//! DATABASE is neither a `<reserved word>` nor a `<non-reserved word>`
//! [§5.2], so an `<identifier>` to the lexer: SET DATABASE is the second
//! form when a `<setting name>` follows it, and otherwise the first, of a
//! setting named DATABASE. Split from parser.rs (RULES.md R-SIZE-2).

use super::Parser;
use crate::ast::{AccessMode, IsolationLevel, SqlStatement, TransactionModes, ValueSpecification};
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};

/// The word after SET of a database's value of a setting.
const DATABASE: &str = "DATABASE";

impl Parser {
    /// A statement that begins with SET: SET TRANSACTION, SET CONSTRAINTS,
    /// SET SCHEMA, or SET of a setting.
    pub(super) fn set_statement(&mut self) -> Result<SqlStatement> {
        let database = *self.peek_at(1) == Tok::Identifier(DATABASE.to_owned()) && matches!(self.peek_at(2), Tok::Identifier(_));
        if database {
            self.advance();
            self.advance();
            let (name, value) = self.setting_and_value()?;
            return Ok(SqlStatement::SetDatabase { name, value });
        }
        if matches!(self.peek_at(1), Tok::Identifier(_)) {
            self.advance();
            let (name, value) = self.setting_and_value()?;
            return Ok(SqlStatement::SetSetting { name, value });
        }
        if self.next_is_kw(kw::TRANSACTION) {
            self.advance();
            self.advance();
            return Ok(SqlStatement::SetTransaction(self.transaction_modes()?));
        }
        if self.next_is_kw(kw::SCHEMA) {
            self.advance();
            self.advance();
            return Ok(SqlStatement::SetSchema(self.value_specification()?));
        }
        self.set_constraints()
    }

    /// \[EXTENSION\] `<setting name>` {`<equals operator>` | TO}
    /// {`<value specification>` | DEFAULT}, after SET or SET DATABASE: the
    /// name, its parts joined by periods, and the value, none for DEFAULT.
    fn setting_and_value(&mut self) -> Result<(String, Option<ValueSpecification>)> {
        let mut parts = vec![self.identifier()?];
        while self.accept_delim(delim::PERIOD) {
            parts.push(self.identifier()?);
        }
        if !self.accept_delim(delim::EQUALS) {
            self.expect_kw(kw::TO)?;
        }
        let value = if self.accept_kw(kw::DEFAULT) { None } else { Some(self.value_specification()?) };
        Ok((parts.join("."), value))
    }

    /// [§14.1] `<transaction mode>` \[{, `<transaction mode>`}...\], each
    /// at most once \[SR1\]; READ WRITE not with READ UNCOMMITTED \[SR3\].
    fn transaction_modes(&mut self) -> Result<TransactionModes> {
        let mut modes = TransactionModes::default();
        loop {
            let twice = |what: &str| format!("[ISO/IEC 9075:1992 §14.1 SR1] {what} is specified more than once");
            if self.accept_kw(kw::ISOLATION) {
                self.expect_kw(kw::LEVEL)?;
                let level = self.isolation_level()?;
                if modes.isolation.replace(level).is_some() {
                    return Err(self.rule(twice("an <isolation level>")));
                }
            } else if self.accept_kw(kw::DIAGNOSTICS) {
                self.expect_kw(kw::SIZE)?;
                let size = self.simple_value_specification()?;
                if modes.diagnostics.replace(size).is_some() {
                    return Err(self.rule(twice("a <diagnostics size>")));
                }
            } else {
                self.expect_kw(kw::READ)?;
                let access = if self.accept_kw(kw::ONLY) {
                    AccessMode::ReadOnly
                } else {
                    self.expect_kw(kw::WRITE)?;
                    AccessMode::ReadWrite
                };
                if modes.access.replace(access).is_some() {
                    return Err(self.rule(twice("a <transaction access mode>")));
                }
            }
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        if modes.isolation == Some(IsolationLevel::ReadUncommitted) && modes.access == Some(AccessMode::ReadWrite) {
            return Err(self.rule("[ISO/IEC 9075:1992 §14.1 SR3] READ WRITE is not with READ UNCOMMITTED"));
        }
        Ok(modes)
    }

    /// [§14.1] `<level of isolation>` ::= READ UNCOMMITTED | READ COMMITTED
    /// | REPEATABLE READ | SERIALIZABLE. COMMITTED, REPEATABLE,
    /// SERIALIZABLE and UNCOMMITTED are `<non-reserved word>`s [§5.2], so
    /// `<identifier>`s to the lexer.
    fn isolation_level(&mut self) -> Result<IsolationLevel> {
        if self.accept_kw(kw::READ) {
            if self.accept_word("UNCOMMITTED") {
                return Ok(IsolationLevel::ReadUncommitted);
            }
            if self.accept_word("COMMITTED") {
                return Ok(IsolationLevel::ReadCommitted);
            }
            return Err(self.syntax("expected UNCOMMITTED or COMMITTED [ISO/IEC 9075:1992 §14.1]"));
        }
        if self.accept_word("REPEATABLE") {
            self.expect_kw(kw::READ)?;
            return Ok(IsolationLevel::RepeatableRead);
        }
        if self.accept_word("SERIALIZABLE") {
            return Ok(IsolationLevel::Serializable);
        }
        Err(self.syntax("expected a <level of isolation> [ISO/IEC 9075:1992 §14.1]"))
    }

    /// Takes the `<non-reserved word>` `word` if it is next.
    pub(super) fn accept_word(&mut self, word: &str) -> bool {
        let next = matches!(self.peek(), Tok::Identifier(s) if s == word);
        if next {
            self.advance();
        }
        next
    }

    /// [ISO/IEC 9075:1992 §6.2] `<simple value specification>` ::=
    /// `<parameter name>` | `<literal>`.
    pub(super) fn simple_value_specification(&mut self) -> Result<ValueSpecification> {
        if self.at_parameter() {
            return Ok(ValueSpecification::Parameter(self.parameter_specification()?));
        }
        Ok(ValueSpecification::Literal(self.literal()?))
    }
}

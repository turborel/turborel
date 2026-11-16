//! [ISO/IEC 9075-2:2023 §12.4 to §12.8, §19.3] The statements of roles:
//! CREATE ROLE, GRANT and REVOKE of roles, DROP ROLE and SET ROLE; and
//! GRANTED BY, which GRANT and REVOKE of privileges take too.
//!
//! ROLE, ADMIN, GRANTED and NONE are read as words, not key words, so the
//! names that spell them before roles keep their meaning. A GRANT or
//! REVOKE is of roles if TO or FROM, not ON, ends its list, or if ADMIN
//! OPTION FOR begins it.

use super::Parser;
use super::users::ROLE;
use crate::ast::{Grantor, RoleStatement, SetRole, SqlStatement};
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};

/// The `<non-reserved word>` of WITH ADMIN and ADMIN OPTION FOR.
const ADMIN: &str = "ADMIN";

/// The `<non-reserved word>` before BY of GRANTED BY.
const GRANTED: &str = "GRANTED";

/// The `<non-reserved word>` of SET ROLE NONE.
const NONE: &str = "NONE";

impl Parser {
    /// Whether CREATE ROLE or DROP ROLE is next.
    pub(super) fn at_role_definition(&self) -> bool {
        matches!(self.peek(), Tok::KeyWord(kw::CREATE | kw::DROP)) && matches!(self.peek_at(1), Tok::Identifier(w) if w == ROLE)
    }

    /// Whether the GRANT or REVOKE next is of roles: ADMIN OPTION FOR
    /// follows REVOKE, or TO or FROM, outside parentheses, comes before
    /// any ON.
    pub(super) fn at_role_grant(&self) -> bool {
        if matches!(self.peek_at(1), Tok::Identifier(w) if w == ADMIN) {
            return true;
        }
        let mut depth = 0_usize;
        for n in 1.. {
            match self.peek_at(n) {
                Tok::Delimiter(delim::LEFT_PAREN) => depth = depth.saturating_add(1),
                Tok::Delimiter(delim::RIGHT_PAREN) => depth = depth.saturating_sub(1),
                Tok::KeyWord(kw::ON) if depth == 0 => return false,
                Tok::KeyWord(kw::TO | kw::FROM) if depth == 0 => return true,
                Tok::End => return false,
                Tok::KeyWord(_)
                | Tok::Identifier(_)
                | Tok::ExactNumeric(_)
                | Tok::ApproximateNumeric(_)
                | Tok::CharacterString(_)
                | Tok::BinaryString(_)
                | Tok::Introducer(_)
                | Tok::Delimiter(_) => {}
            }
        }
        false
    }

    /// The statement of roles next, if one is.
    pub(super) fn role_statement(&mut self) -> Result<Option<RoleStatement>> {
        if self.is_kw(kw::GRANT) && self.at_role_grant() {
            return self.grant_role().map(Some);
        }
        if self.is_kw(kw::REVOKE) && self.at_role_grant() {
            return self.revoke_role().map(Some);
        }
        if self.at_role_definition() {
            return self.role_definition().map(Some);
        }
        Ok(None)
    }

    /// [§12.4] `<role definition>`, or [§12.6] `<drop role statement>`,
    /// from CREATE or DROP.
    pub(super) fn role_definition(&mut self) -> Result<RoleStatement> {
        let create = self.accept_kw(kw::CREATE);
        if !create {
            self.expect_kw(kw::DROP)?;
        }
        self.advance();
        let at = self.here();
        let name = (self.identifier()?, at);
        if !create {
            return Ok(RoleStatement::Drop { name, behavior: self.drop_behavior()? });
        }
        let admin = if self.accept_kw(kw::WITH) {
            if !self.accept_word(ADMIN) {
                return Err(self.syntax("expected ADMIN"));
            }
            Some(self.grantor()?)
        } else {
            None
        };
        Ok(RoleStatement::Create { name, admin })
    }

    /// [§12.5] `<grant role statement>`, from GRANT.
    pub(super) fn grant_role(&mut self) -> Result<RoleStatement> {
        self.expect_kw(kw::GRANT)?;
        let roles = self.role_names()?;
        self.expect_kw(kw::TO)?;
        let grantees = self.grantees()?;
        let admin = self.accept_kw(kw::WITH);
        if admin {
            if !self.accept_word(ADMIN) {
                return Err(self.syntax("expected ADMIN OPTION"));
            }
            self.expect_kw(kw::OPTION)?;
        }
        Ok(RoleStatement::Grant { roles, grantees, admin, grantor: self.granted_by()? })
    }

    /// [§12.7] `<revoke role statement>`, from REVOKE.
    fn revoke_role(&mut self) -> Result<RoleStatement> {
        self.expect_kw(kw::REVOKE)?;
        let admin_option = self.accept_word(ADMIN);
        if admin_option {
            self.expect_kw(kw::OPTION)?;
            self.expect_kw(kw::FOR)?;
        }
        let roles = self.role_names()?;
        self.expect_kw(kw::FROM)?;
        let grantees = self.grantees()?;
        let grantor = self.granted_by()?;
        Ok(RoleStatement::Revoke { admin_option, roles, grantees, grantor, behavior: self.drop_behavior()? })
    }

    /// `<role name>` \[{, `<role name>`}...\], each with where it begins.
    fn role_names(&mut self) -> Result<Vec<(String, usize)>> {
        let mut roles = Vec::new();
        loop {
            let at = self.here();
            roles.push((self.identifier()?, at));
            if !self.accept_delim(delim::COMMA) {
                return Ok(roles);
            }
        }
    }

    /// GRANTED BY `<grantor>`, if it is next [§12.2 SR6, §12.5, §12.7].
    pub(super) fn granted_by(&mut self) -> Result<Option<Grantor>> {
        if !self.accept_word(GRANTED) {
            return Ok(None);
        }
        self.expect_kw(kw::BY)?;
        self.grantor().map(Some)
    }

    /// [§12.8] `<grantor>` ::= CURRENT_USER | CURRENT_ROLE.
    fn grantor(&mut self) -> Result<Grantor> {
        if self.accept_kw(kw::CURRENT_USER) {
            return Ok(Grantor::CurrentUser);
        }
        if self.at_current_role() {
            self.advance();
            return Ok(Grantor::CurrentRole);
        }
        Err(self.syntax("expected CURRENT_USER or CURRENT_ROLE"))
    }

    /// Whether SET ROLE is next: ROLE after SET, not a setting's name, that
    /// "=", TO or a period would follow.
    pub(super) fn at_set_role(&self) -> bool {
        matches!(self.peek_at(1), Tok::Identifier(w) if w == ROLE)
            && !matches!(self.peek_at(2), Tok::Delimiter(delim::EQUALS | delim::PERIOD) | Tok::KeyWord(kw::TO))
    }

    /// [§19.3] `<set role statement>`, from SET.
    pub(super) fn set_role(&mut self) -> Result<SqlStatement> {
        self.expect_kw(kw::SET)?;
        self.advance();
        if self.accept_word(NONE) {
            return Ok(SqlStatement::SetRole(SetRole::None));
        }
        Ok(SqlStatement::SetRole(SetRole::Role(self.value_specification()?)))
    }
}

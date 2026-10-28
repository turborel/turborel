//! Domains [ISO/IEC 9075:1992 §11.21–§11.27]: `<domain definition>`,
//! `<alter domain statement>`, and a `<data type>` or `<domain name>`
//! where either may be written. Split from parser/schema.rs (RULES.md
//! R-SIZE-2).

use super::Parser;
use crate::ast::ColumnSpecification;
use crate::ast::{
    AlterDomain, AlterDomainAction, AssertionDefinition, DomainDefinition, TableConstraint, TableConstraintKind, TypeName, ValueExpression,
};
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};

/// The column VALUE stands for in a domain constraint, as it is bound
/// [ISO/IEC 9075:1992 §6.2 SR5]: no regular identifier is VALUE, a
/// reserved word.
pub const VALUE_COLUMN: &str = "VALUE";

impl Parser {
    /// [ISO/IEC 9075:1992 §11.21] `<domain definition>` ::= CREATE DOMAIN
    /// `<domain name>` \[AS\] `<data type>` \[`<default clause>`\]
    /// \[`<domain constraint>`...\]
    pub(super) fn domain_definition(&mut self) -> Result<DomainDefinition> {
        self.expect_kw(kw::CREATE)?;
        self.expect_kw(kw::DOMAIN)?;
        let name = self.table_name()?;
        self.accept_kw(kw::AS);
        let data_type = self.data_type()?;
        let default = if self.accept_kw(kw::DEFAULT) { Some(self.default_option()?) } else { None };
        let mut constraints = Vec::new();
        while self.is_kw(kw::CONSTRAINT) || self.is_kw(kw::CHECK) {
            constraints.push(self.domain_constraint()?);
        }
        Ok(DomainDefinition { name, data_type, default, constraints })
    }

    /// [ISO/IEC 9075:1992 §11.21] `<domain constraint>` ::= \[`<constraint
    /// name definition>`\] `<check constraint definition>` \[`<constraint
    /// attributes>`\]: VALUE in it the column VALUE.
    fn domain_constraint(&mut self) -> Result<TableConstraint> {
        let outer = std::mem::replace(&mut self.in_domain, true);
        let constraint = self.table_constraint();
        self.in_domain = outer;
        let constraint = constraint?;
        if !matches!(constraint.kind, TableConstraintKind::Check(_)) {
            return Err(self.syntax("[ISO/IEC 9075:1992 §11.21] a <domain constraint> is a <check constraint definition>"));
        }
        Ok(constraint)
    }

    /// [ISO/IEC 9075:1992 §11.22] `<alter domain statement>` ::= ALTER
    /// DOMAIN `<domain name>` {SET `<default clause>` | DROP DEFAULT | ADD
    /// `<domain constraint>` | DROP CONSTRAINT `<constraint name>`}
    pub(super) fn alter_domain(&mut self) -> Result<AlterDomain> {
        self.expect_kw(kw::ALTER)?;
        self.expect_kw(kw::DOMAIN)?;
        let name = self.table_name()?;
        let action = if self.accept_kw(kw::SET) {
            self.expect_kw(kw::DEFAULT)?;
            AlterDomainAction::SetDefault(self.default_option()?)
        } else if self.accept_kw(kw::ADD) {
            AlterDomainAction::AddConstraint(self.domain_constraint()?)
        } else {
            self.expect_kw(kw::DROP)?;
            if self.accept_kw(kw::DEFAULT) {
                AlterDomainAction::DropDefault
            } else {
                self.expect_kw(kw::CONSTRAINT)?;
                AlterDomainAction::DropConstraint(self.table_name()?)
            }
        };
        Ok(AlterDomain { name, action })
    }

    /// [ISO/IEC 9075:1992 §11.34] `<assertion definition>` ::= CREATE
    /// ASSERTION `<constraint name>` CHECK ( `<search condition>` )
    /// \[`<constraint attributes>`\]
    pub(super) fn assertion_definition(&mut self) -> Result<AssertionDefinition> {
        self.expect_kw(kw::CREATE)?;
        self.expect_kw(kw::ASSERTION)?;
        let name = self.table_name()?;
        let check = self.check()?;
        Ok(AssertionDefinition { name, check, attributes: self.constraint_attributes()? })
    }

    /// [ISO/IEC 9075:1992 §11.4, §6.10] A `<data type>`, or else a `<domain
    /// name>`; \[EXTENSION\] or a type beyond the standard, its name and
    /// its parameters in parentheses (RULES.md D53).
    ///
    /// ```text
    /// <extension type> ::= <identifier>
    ///     [ <left paren> <unsigned integer> [ <comma> <unsigned integer> ] <right paren> ]
    /// ```
    pub(super) fn type_name(&mut self) -> Result<TypeName> {
        if !matches!(self.peek(), Tok::Identifier(_)) {
            return Ok(TypeName::Data(self.data_type()?));
        }
        let name = self.table_name()?;
        if !self.accept_delim(delim::LEFT_PAREN) {
            return Ok(TypeName::Domain(name));
        }
        let mut params = vec![self.unsigned_integer()?];
        if self.accept_delim(delim::COMMA) {
            params.push(self.unsigned_integer()?);
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(TypeName::Extension { name, params })
    }

    /// [ISO/IEC 9075:1992 §6.2 SR5] VALUE, in a domain constraint alone:
    /// the column VALUE.
    pub(super) fn value_reference(&mut self) -> Result<ValueExpression> {
        let at = self.here();
        if !self.in_domain {
            return Err(self.rule("[ISO/IEC 9075:1992 §6.2 SR5] VALUE is of a domain constraint alone"));
        }
        self.expect_kw(kw::VALUE)?;
        Ok(ValueExpression::Column(ColumnSpecification { qualifier: None, column: VALUE_COLUMN.to_owned(), at }))
    }
}

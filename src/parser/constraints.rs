//! \[EXTENSION\] The constraints of ISO/IEC 9075:1992: `<column constraint
//! definition>` [§11.4], `<table constraint definition>` [§11.6], with their
//! `<constraint name definition>` and `<constraint attributes>` [§10.6].
//! Split from parser/schema.rs (RULES.md R-SIZE-2).

use super::Parser;
use crate::ast::{
    Check, ColumnConstraint, ColumnConstraintKind, ConstraintAttributes, MatchType, References, ReferentialAction, SqlStatement,
    TableConstraint, TableConstraintKind,
};
use crate::error::Result;
use crate::lexer::{delim, kw};

impl Parser {
    /// Whether a `<table constraint definition>` begins here.
    pub(super) fn at_table_constraint(&self) -> bool {
        [kw::CONSTRAINT, kw::UNIQUE, kw::PRIMARY, kw::FOREIGN, kw::CHECK].into_iter().any(|k| self.is_kw(k))
    }

    /// Whether a `<column constraint definition>` begins here.
    pub(super) fn at_column_constraint(&self) -> bool {
        [kw::CONSTRAINT, kw::NOT, kw::UNIQUE, kw::PRIMARY, kw::REFERENCES, kw::CHECK].into_iter().any(|k| self.is_kw(k))
    }

    /// [§11.6] `<table constraint definition>` ::= \[`<constraint name
    /// definition>`\] `<table constraint>` \[`<constraint attributes>`\]
    ///
    /// # Errors
    ///
    /// The text is not one, or uses a part of one not implemented.
    pub fn table_constraint(&mut self) -> Result<TableConstraint> {
        let at = self.here();
        let name = self.constraint_name()?;
        let kind = if self.is_kw(kw::UNIQUE) || self.is_kw(kw::PRIMARY) {
            let primary = self.unique_specification()?;
            TableConstraintKind::Unique { primary, columns: self.name_list()? }
        } else if self.accept_kw(kw::FOREIGN) {
            // [§11.8] FOREIGN KEY (<referencing columns>) <references specification>
            self.expect_kw(kw::KEY)?;
            let columns = self.name_list()?;
            TableConstraintKind::ForeignKey { columns, references: self.references()? }
        } else {
            TableConstraintKind::Check(self.check()?)
        };
        Ok(TableConstraint { name, at, kind, attributes: self.constraint_attributes()? })
    }

    /// [§11.4] `<column constraint definition>` ::= \[`<constraint name
    /// definition>`\] `<column constraint>` \[`<constraint attributes>`\]
    pub(super) fn column_constraint(&mut self) -> Result<ColumnConstraint> {
        let at = self.here();
        let name = self.constraint_name()?;
        let kind = if self.accept_kw(kw::NOT) {
            self.expect_kw(kw::NULL)?;
            ColumnConstraintKind::NotNull
        } else if self.is_kw(kw::UNIQUE) || self.is_kw(kw::PRIMARY) {
            ColumnConstraintKind::Unique { primary: self.unique_specification()? }
        } else if self.is_kw(kw::REFERENCES) {
            ColumnConstraintKind::References(self.references()?)
        } else {
            ColumnConstraintKind::Check(self.check()?)
        };
        Ok(ColumnConstraint { name, at, kind, attributes: self.constraint_attributes()? })
    }

    /// [§10.6] `<constraint name definition>` ::= CONSTRAINT `<constraint
    /// name>`, if written.
    fn constraint_name(&mut self) -> Result<Option<crate::ast::TableName>> {
        if self.accept_kw(kw::CONSTRAINT) { self.table_name().map(Some) } else { Ok(None) }
    }

    /// [§11.7] `<unique specification>` ::= UNIQUE | PRIMARY KEY: whether
    /// it is PRIMARY KEY.
    fn unique_specification(&mut self) -> Result<bool> {
        if self.accept_kw(kw::UNIQUE) {
            return Ok(false);
        }
        self.expect_kw(kw::PRIMARY)?;
        self.expect_kw(kw::KEY)?;
        Ok(true)
    }

    /// (`<column name>` \[{, `<column name>`}...\])
    pub(super) fn name_list(&mut self) -> Result<Vec<String>> {
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut names = vec![self.identifier()?];
        while self.accept_delim(delim::COMMA) {
            names.push(self.identifier()?);
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(names)
    }

    /// [ISO/IEC 9075:1992 §8.10, §11.8] `<match type>` ::= FULL | PARTIAL,
    /// after MATCH.
    pub(super) fn match_type(&mut self) -> Result<MatchType> {
        if self.accept_kw(kw::PARTIAL) {
            return Ok(MatchType::Partial);
        }
        self.expect_kw(kw::FULL)?;
        Ok(MatchType::Full)
    }

    /// [§11.8] `<references specification>` ::= REFERENCES `<referenced
    /// table and columns>` \[MATCH `<match type>`\] \[`<referential
    /// triggered action>`\]
    fn references(&mut self) -> Result<References> {
        self.expect_kw(kw::REFERENCES)?;
        let table = self.table_name()?;
        let columns = if self.is_delim(delim::LEFT_PAREN) { Some(self.name_list()?) } else { None };
        let match_type = if self.accept_kw(kw::MATCH) { self.match_type()? } else { MatchType::Simple };
        // <referential triggered action> ::= <update rule> [<delete rule>] | <delete rule> [<update rule>]
        let (mut on_update, mut on_delete) = (None, None);
        while self.accept_kw(kw::ON) {
            let rule = if self.accept_kw(kw::UPDATE) { &mut on_update } else { self.expect_kw(kw::DELETE).map(|()| &mut on_delete)? };
            if rule.is_some() {
                return Err(self.syntax("[ISO/IEC 9075:1992 §11.8] a rule is written once"));
            }
            *rule = Some(self.referential_action()?);
        }
        Ok(References {
            table,
            columns,
            match_type,
            on_update: on_update.unwrap_or(ReferentialAction::NoAction),
            on_delete: on_delete.unwrap_or(ReferentialAction::NoAction),
        })
    }

    /// [§11.8] `<referential action>` ::= CASCADE | SET NULL | SET DEFAULT
    /// | NO ACTION
    fn referential_action(&mut self) -> Result<ReferentialAction> {
        if self.accept_kw(kw::CASCADE) {
            return Ok(ReferentialAction::Cascade);
        }
        if self.accept_kw(kw::SET) {
            if self.accept_kw(kw::NULL) {
                return Ok(ReferentialAction::SetNull);
            }
            self.expect_kw(kw::DEFAULT)?;
            return Ok(ReferentialAction::SetDefault);
        }
        self.expect_kw(kw::NO)?;
        self.expect_kw(kw::ACTION)?;
        Ok(ReferentialAction::NoAction)
    }

    /// [§11.9] The `<search condition>` of a CHECK, alone, as the catalog
    /// keeps it.
    ///
    /// # Errors
    ///
    /// The text is not a `<search condition>`.
    pub fn check_condition(&mut self) -> Result<crate::ast::SearchCondition> {
        let condition = self.search_condition()?;
        self.end()?;
        Ok(condition)
    }

    /// [§11.9] `<check constraint definition>` ::= CHECK (`<search
    /// condition>`)
    pub(super) fn check(&mut self) -> Result<Check> {
        self.expect_kw(kw::CHECK)?;
        self.expect_delim(delim::LEFT_PAREN)?;
        let start = self.here();
        let condition = self.search_condition()?;
        let text = self.text_from(start);
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(Check { condition, text })
    }

    /// [§10.6] `<constraint attributes>`, if written: what they say, or
    /// imply [SR3, SR4].
    pub(super) fn constraint_attributes(&mut self) -> Result<ConstraintAttributes> {
        // <constraint attributes> ::= <constraint check time> [ [ NOT ] DEFERRABLE ]
        //                           | [ NOT ] DEFERRABLE [ <constraint check time> ]
        let (mut deferrable, mut deferred) = (None, None);
        for _ in 0_u8..2 {
            if deferred.is_none() && self.accept_kw(kw::INITIALLY) {
                // <constraint check time> ::= INITIALLY DEFERRED | INITIALLY IMMEDIATE
                deferred = Some(self.accept_kw(kw::DEFERRED));
                if deferred == Some(false) {
                    self.expect_kw(kw::IMMEDIATE)?;
                }
            } else if deferrable.is_none() && (self.is_kw(kw::DEFERRABLE) || (self.is_kw(kw::NOT) && self.next_is_kw(kw::DEFERRABLE))) {
                deferrable = Some(!self.accept_kw(kw::NOT));
                self.expect_kw(kw::DEFERRABLE)?;
            }
        }
        // [SR3] INITIALLY IMMEDIATE if no check time is written.
        let initially_deferred = deferred.unwrap_or(false);
        // [SR4a i] INITIALLY DEFERRED, not NOT DEFERRABLE.
        if initially_deferred && deferrable == Some(false) {
            return Err(self.rule("[ISO/IEC 9075:1992 §10.6 SR4a i] an INITIALLY DEFERRED constraint shall not be NOT DEFERRABLE"));
        }
        // [SR4a ii, SR4b] DEFERRABLE if INITIALLY DEFERRED, else NOT DEFERRABLE, if neither is written.
        Ok(ConstraintAttributes { deferrable: deferrable.unwrap_or(initially_deferred), initially_deferred })
    }

    /// [§14.2] `<set constraints mode statement>` ::= SET CONSTRAINTS
    /// `<constraint name list>` {DEFERRED | IMMEDIATE}, where `<constraint
    /// name list>` ::= ALL | `<constraint name>` \[{, `<constraint
    /// name>`}...\]
    pub(super) fn set_constraints(&mut self) -> Result<SqlStatement> {
        self.expect_kw(kw::SET)?;
        self.expect_kw(kw::CONSTRAINTS)?;
        let names = if self.accept_kw(kw::ALL) {
            None
        } else {
            let mut names = vec![self.table_name()?];
            while self.accept_delim(delim::COMMA) {
                names.push(self.table_name()?);
            }
            Some(names)
        };
        let deferred = self.accept_kw(kw::DEFERRED);
        if !deferred {
            self.expect_kw(kw::IMMEDIATE)?;
        }
        Ok(SqlStatement::SetConstraints { names, deferred })
    }
}

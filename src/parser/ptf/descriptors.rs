//! [ISO/IEC 9075-2:2023 §5.4, §20.6, §20.28] The names of a PTF data area
//! (Feature B209): a `<PTF descriptor name>` or `<PTF cursor name>` ::= PTF
//! `<simple value specification>`; and the statements a polymorphic table
//! function's component procedures write them in that no other has, the
//! `<copy descriptor statement>` and the `<pipe row statement>`.
//!
//! PTF, COPY and PIPE are no key words of SQL-92, so each stays an
//! identifier, read as a word here: PTF only before what begins a
//! `<simple value specification>`, so a descriptor or cursor named PTF is
//! read as it was.

use crate::ast::{CopiedItem, CopyOptions, DynamicName, DynamicStatement, ExtendedName, Scope, SqlStatement, UsingClause};
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Tok, delim, kw};
use crate::parser::Parser;
use crate::parser::user_types::TYPE;

/// Of a `<PTF descriptor name>` or `<PTF cursor name>`.
const PTF: &str = "PTF";
/// Of a `<copy descriptor statement>`.
const COPY: &str = "COPY";
/// Of a `<pipe row statement>`.
const PIPE: &str = "PIPE";
/// Of a `<pipe row statement>`.
const ROW: &str = "ROW";
/// Of `<copy descriptor options>`.
const NAME: &str = "NAME";
/// Of `<copy descriptor options>`.
const DATA: &str = "DATA";
/// The most tokens a `<fetch orientation>` and FROM take before the
/// cursor's name: ABSOLUTE, a value and FROM.
const ORIENTATION_TOKENS: usize = 3;

/// Whether `name` is a `<PTF descriptor name>` or `<PTF cursor name>`.
fn of_ptf(name: &ExtendedName) -> bool {
    name.scope == Scope::Ptf
}

/// [ISO/IEC 9075-2:2023 §11.60 SR23f] (IA078) Whether the SQL dynamic
/// statement `d` is one an SQL routine body may hold: one of a PTF data
/// area, as only a PTF component procedure's runs.
fn of_ptf_data_area(d: &DynamicStatement) -> bool {
    match d {
        DynamicStatement::GetDescriptor { name, .. } | DynamicStatement::SetDescriptor { name, .. } => of_ptf(name),
        DynamicStatement::CopyDescriptor { .. } | DynamicStatement::PipeRow(_) => true,
        DynamicStatement::Fetch { cursor: DynamicName::Extended(cursor), using, .. } => {
            of_ptf(cursor)
                && match using {
                    UsingClause::Descriptor(into) => of_ptf(into),
                    UsingClause::Arguments(_) => true,
                }
        }
        DynamicStatement::Fetch { cursor: DynamicName::Named(..), .. }
        | DynamicStatement::AllocateDescriptor { .. }
        | DynamicStatement::DeallocateDescriptor(_)
        | DynamicStatement::Prepare { .. }
        | DynamicStatement::DeallocatePrepare(_)
        | DynamicStatement::Describe { .. }
        | DynamicStatement::AllocateCursor { .. }
        | DynamicStatement::Open { .. }
        | DynamicStatement::Close(_)
        | DynamicStatement::DeletePositioned { .. }
        | DynamicStatement::UpdatePositioned { .. } => false,
    }
}

impl Parser {
    /// Whether a `<PTF descriptor name>` or `<PTF cursor name>` is next:
    /// PTF, then what begins a `<simple value specification>`.
    pub(in crate::parser) fn at_ptf_name(&self) -> bool {
        self.is_word(PTF)
            && matches!(self.peek_at(1), Tok::Identifier(_) | Tok::CharacterString(_) | Tok::Introducer(_) | Tok::Delimiter(delim::COLON))
    }

    /// [§5.4] The `<PTF descriptor name>` or `<PTF cursor name>` next.
    fn ptf_name(&mut self) -> Result<ExtendedName> {
        let at = self.here();
        self.advance();
        Ok(ExtendedName { scope: Scope::Ptf, value: self.simple_value()?, at })
    }

    /// [§5.4] `<descriptor name>` ::= `<conventional descriptor name>` |
    /// `<PTF descriptor name>`.
    ///
    /// # Errors
    ///
    /// The text is not one.
    pub(in crate::parser) fn descriptor_name(&mut self) -> Result<ExtendedName> {
        if self.at_ptf_name() { self.ptf_name() } else { self.extended_name() }
    }

    /// [ISO/IEC 9075-2:2023 §11.60 SR23f] That `statement`, at `at`, of
    /// an SQL routine body, is no SQL dynamic statement but one of a PTF
    /// data area (IA078).
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it is another.
    pub(in crate::parser) fn dynamic_allowed(statement: &SqlStatement, at: usize) -> Result<()> {
        let refused =
            matches!(statement, SqlStatement::Execute(_)) || matches!(statement, SqlStatement::Dynamic(d) if !of_ptf_data_area(d));
        if refused {
            let msg = "[ISO/IEC 9075-2:2023 §11.60 SR23f] an SQL routine body holds no <SQL dynamic statement> but one of a PTF data \
                       area: GET or SET DESCRIPTOR of a <PTF descriptor name>, COPY DESCRIPTOR, PIPE ROW, or FETCH from a <PTF cursor \
                       name> (IA078)";
            return Err(SqlError::at(Code::SyntaxRule, at, msg));
        }
        Ok(())
    }

    /// Whether a `<dynamic fetch statement>` of a `<PTF cursor name>` is
    /// next: FETCH, then within a `<fetch orientation>` and FROM, PTF and
    /// what begins its name.
    pub(in crate::parser) fn at_ptf_fetch(&self) -> bool {
        if !self.is_kw(kw::FETCH) {
            return false;
        }
        (1..=ORIENTATION_TOKENS.saturating_add(1)).any(|n| {
            matches!(self.peek_at(n), Tok::Identifier(w) if w == PTF)
                && matches!(self.peek_at(n.saturating_add(1)), Tok::Identifier(_) | Tok::CharacterString(_) | Tok::Delimiter(delim::COLON))
        })
    }

    /// Whether a `<copy descriptor statement>` begins here.
    pub(in crate::parser) fn at_copy_descriptor(&self) -> bool {
        self.is_word(COPY) && !matches!(self.peek_at(1), Tok::Delimiter(_) | Tok::End)
    }

    /// Whether a `<pipe row statement>` begins here.
    pub(in crate::parser) fn at_pipe_row(&self) -> bool {
        self.is_word(PIPE) && matches!(self.peek_at(1), Tok::Identifier(w) if w == ROW)
    }

    /// [§20.6] `<copy descriptor statement>` ::= COPY `<source descriptor
    /// name>` TO `<target descriptor name>` | COPY `<source descriptor
    /// name>` VALUE `<item number 1>` ( `<copy descriptor options>` ) TO
    /// `<target descriptor name>` VALUE `<item number 2>`, the target a
    /// `<PTF descriptor name>`.
    ///
    /// # Errors
    ///
    /// The text is not one.
    pub(in crate::parser) fn copy_descriptor(&mut self) -> Result<SqlStatement> {
        self.expect_word(COPY)?;
        let source = self.descriptor_name()?;
        let item = if self.accept_kw(kw::VALUE) {
            let from = self.simple_value()?;
            self.expect_delim(delim::LEFT_PAREN)?;
            let options = self.copy_options()?;
            self.expect_delim(delim::RIGHT_PAREN)?;
            Some(from).zip(Some(options))
        } else {
            None
        };
        self.expect_kw(kw::TO)?;
        if !self.at_ptf_name() {
            return Err(self.syntax("expected a <PTF descriptor name>"));
        }
        let target = self.ptf_name()?;
        let item = match item {
            Some((from, options)) => {
                self.expect_kw(kw::VALUE)?;
                Some(Box::new(CopiedItem { from, to: self.simple_value()?, options }))
            }
            None => None,
        };
        Ok(SqlStatement::Dynamic(Box::new(DynamicStatement::CopyDescriptor { source, target, item })))
    }

    /// `<copy descriptor options>` ::= NAME | TYPE | NAME , TYPE | DATA.
    fn copy_options(&mut self) -> Result<CopyOptions> {
        if self.accept_word(DATA) {
            return Ok(CopyOptions::Data);
        }
        if self.accept_word(TYPE) {
            return Ok(CopyOptions::Type);
        }
        self.expect_word(NAME)?;
        if !self.accept_delim(delim::COMMA) {
            return Ok(CopyOptions::Name);
        }
        self.expect_word(TYPE)?;
        Ok(CopyOptions::NameType)
    }

    /// [§20.28] `<pipe row statement>` ::= PIPE ROW `<PTF descriptor
    /// name>`.
    ///
    /// # Errors
    ///
    /// The text is not one.
    pub(in crate::parser) fn pipe_row(&mut self) -> Result<SqlStatement> {
        self.expect_word(PIPE)?;
        self.expect_word(ROW)?;
        if !self.at_ptf_name() {
            return Err(self.syntax("expected a <PTF descriptor name>"));
        }
        Ok(SqlStatement::Dynamic(Box::new(DynamicStatement::PipeRow(self.ptf_name()?))))
    }
}

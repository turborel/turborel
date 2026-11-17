//! [ISO/IEC 9075-2:2023 §11.51] `<user-defined type definition>`, read
//! whole: its options in any order [Feature S028], those a distinct type
//! refuses read so a Syntax Rule refuses them as it is processed. Its
//! `<member list>` and `<method specification list>`, of a structured
//! type, are read in `structured.rs`.

use super::Parser;
use crate::ast::{RefRepresentation, TypeDefinition, TypeName, TypeOption};
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};

/// [ISO/IEC 9075-2:2023 §11.51] The words of a `<user-defined type
/// definition>`, no key words of SQL-92.
pub(super) const TYPE: &str = "TYPE";
/// UNDER, of a `<subtype clause>`, a `<subtable clause>` and a `<subview clause>`.
pub(super) const UNDER: &str = "UNDER";
const INSTANTIABLE: &str = "INSTANTIABLE";
const FINAL: &str = "FINAL";
/// REF, of a `<reference type specification>` and a `<reference type>`.
pub(super) const REF: &str = "REF";
const SOURCE: &str = "SOURCE";
/// GENERATED, of SYSTEM GENERATED and USER GENERATED.
pub(super) const GENERATED: &str = "GENERATED";

impl Parser {
    /// Whether CREATE or DROP TYPE is next.
    pub(super) fn next_is_type(&self) -> bool {
        matches!(self.peek_at(1), Tok::Identifier(w) if w == TYPE)
    }

    /// [ISO/IEC 9075-2:2023 §11.51] `<user-defined type definition>` ::=
    /// CREATE TYPE `<user-defined type body>`.
    pub(super) fn type_definition(&mut self) -> Result<TypeDefinition> {
        self.expect_kw(kw::CREATE)?;
        if !self.accept_word(TYPE) {
            return Err(self.syntax("expected TYPE"));
        }
        let name = self.table_name()?;
        let under = if self.accept_word(UNDER) { Some(self.table_name()?) } else { None };
        // [§11.51 SR5] a <member list> defines a structured type, any
        // other <representation> a distinct type.
        let (representation, members) = match (self.accept_kw(kw::AS), self.is_delim(delim::LEFT_PAREN)) {
            (true, true) => (None, Some(self.member_list()?)),
            (true, false) => (Some(self.type_name()?), None),
            (false, _) => (None, None),
        };
        let mut options = Vec::new();
        while let Some(option) = self.type_option()? {
            options.push(option);
        }
        let methods = if self.at_method_specification() { self.method_specification_list(&name)? } else { Vec::new() };
        Ok(TypeDefinition { name, under, representation, members, methods, options })
    }

    /// [ISO/IEC 9075-2:2023 §11.51] A `<user-defined type option>`, if one
    /// is next, and where it begins.
    fn type_option(&mut self) -> Result<Option<(TypeOption, usize)>> {
        let at = self.here();
        let option = if self.accept_word(INSTANTIABLE) {
            TypeOption::Instantiable(true)
        } else if self.accept_word(FINAL) {
            TypeOption::Final(true)
        } else if self.is_kw(kw::NOT) {
            self.advance();
            if self.accept_word(INSTANTIABLE) {
                TypeOption::Instantiable(false)
            } else if self.accept_word(FINAL) {
                TypeOption::Final(false)
            } else {
                return Err(self.syntax("expected INSTANTIABLE or FINAL"));
            }
        } else if self.accept_word(REF) {
            TypeOption::Reference(self.reference_type_specification()?)
        } else if self.is_kw(kw::CAST) {
            self.advance();
            self.cast_option()?
        } else {
            return Ok(None);
        };
        Ok(Some((option, at)))
    }

    /// [ISO/IEC 9075-2:2023 §11.51] The rest of a `<reference type
    /// specification>`, after REF: USING `<predefined type>`, FROM `<list of
    /// attributes>`, or IS SYSTEM GENERATED.
    fn reference_type_specification(&mut self) -> Result<RefRepresentation> {
        if self.accept_kw(kw::USING) {
            return Ok(RefRepresentation::User(TypeName::Data(self.predefined_type()?)));
        }
        if self.accept_kw(kw::FROM) {
            return Ok(RefRepresentation::Derived(self.column_list()?));
        }
        self.expect_kw(kw::IS)?;
        if !self.accept_word("SYSTEM") || !self.accept_word(GENERATED) {
            return Err(self.syntax("expected SYSTEM GENERATED"));
        }
        Ok(RefRepresentation::System)
    }

    /// [ISO/IEC 9075-2:2023 §11.51] The rest of a `<cast to ref>`, `<cast to
    /// type>`, `<cast to distinct>` or `<cast to source>`, after CAST: (
    /// SOURCE AS REF ), ( REF AS SOURCE ), ( SOURCE AS DISTINCT ) or (
    /// DISTINCT AS SOURCE ), then WITH `<identifier>`.
    fn cast_option(&mut self) -> Result<TypeOption> {
        self.expect_delim(delim::LEFT_PAREN)?;
        let from_source = self.accept_word(SOURCE);
        let from_ref = !from_source && self.accept_word(REF);
        if !from_source && !from_ref {
            self.expect_kw(kw::DISTINCT)?;
        }
        self.expect_kw(kw::AS)?;
        let option = if from_source && self.accept_word(REF) {
            TypeOption::CastToRef(String::new())
        } else if from_source {
            self.expect_kw(kw::DISTINCT)?;
            TypeOption::CastToDistinct(String::new())
        } else if self.accept_word(SOURCE) {
            if from_ref { TypeOption::CastToType(String::new()) } else { TypeOption::CastToSource(String::new()) }
        } else {
            return Err(self.syntax("expected SOURCE"));
        };
        self.expect_delim(delim::RIGHT_PAREN)?;
        self.expect_kw(kw::WITH)?;
        let identifier = self.identifier()?;
        Ok(match option {
            TypeOption::CastToDistinct(_) => TypeOption::CastToDistinct(identifier),
            TypeOption::CastToSource(_) => TypeOption::CastToSource(identifier),
            TypeOption::CastToRef(_) => TypeOption::CastToRef(identifier),
            TypeOption::CastToType(_) => TypeOption::CastToType(identifier),
            TypeOption::Instantiable(_) | TypeOption::Final(_) | TypeOption::Reference(_) => option,
        })
    }
}

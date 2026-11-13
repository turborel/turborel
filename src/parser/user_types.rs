//! [ISO/IEC 9075-2:2023 §11.51] `<user-defined type definition>`, read
//! whole: its options in any order [Feature S028], those a distinct type
//! refuses read so a Syntax Rule refuses them as it is processed. A
//! `<member list>` and a `<method specification list>` are of structured
//! types and methods, which this implementation does not take.

use super::Parser;
use crate::ast::{TypeDefinition, TypeOption};
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Tok, delim, kw};

/// [ISO/IEC 9075-2:2023 §11.51] The words of a `<user-defined type
/// definition>`, no key words of SQL-92.
pub(super) const TYPE: &str = "TYPE";
const UNDER: &str = "UNDER";
const INSTANTIABLE: &str = "INSTANTIABLE";
const FINAL: &str = "FINAL";
const REF: &str = "REF";
const SOURCE: &str = "SOURCE";
const GENERATED: &str = "GENERATED";
/// The words that begin a `<method specification>`.
const METHOD_WORDS: [&str; 5] = ["METHOD", "INSTANCE", "STATIC", "CONSTRUCTOR", "OVERRIDING"];

impl Parser {
    /// The refusal of `what`, valid SQL this implementation does not take,
    /// at the token at hand.
    fn not_taken(&self, what: &str) -> SqlError {
        SqlError::at(Code::Unsupported, self.here(), format!("[DESIGN] {what} is valid SQL this implementation does not take"))
    }

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
        let under = if self.is_word(UNDER) {
            let at = self.here();
            self.advance();
            self.table_name()?;
            Some(at)
        } else {
            None
        };
        let representation = if self.accept_kw(kw::AS) {
            if self.is_delim(delim::LEFT_PAREN) {
                return Err(self.not_taken("a <member list>, which defines a structured type [ISO/IEC 9075-2:2023 §11.51 SR5b],"));
            }
            Some(self.type_name()?)
        } else {
            None
        };
        let mut options = Vec::new();
        while let Some(option) = self.type_option()? {
            options.push(option);
        }
        if METHOD_WORDS.iter().any(|w| self.is_word(w)) {
            return Err(self.not_taken("a <method specification list> [ISO/IEC 9075-2:2023 §11.51]"));
        }
        Ok(TypeDefinition { name, under, representation, options })
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
            self.reference_type_specification()?;
            TypeOption::Reference
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
    fn reference_type_specification(&mut self) -> Result<()> {
        if self.accept_kw(kw::USING) {
            self.predefined_type()?;
        } else if self.accept_kw(kw::FROM) {
            self.column_list()?;
        } else {
            self.expect_kw(kw::IS)?;
            if !self.accept_word("SYSTEM") || !self.accept_word(GENERATED) {
                return Err(self.syntax("expected SYSTEM GENERATED"));
            }
        }
        Ok(())
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
            TypeOption::CastToRef
        } else if from_source {
            self.expect_kw(kw::DISTINCT)?;
            TypeOption::CastToDistinct(String::new())
        } else if self.accept_word(SOURCE) {
            if from_ref { TypeOption::CastToType } else { TypeOption::CastToSource(String::new()) }
        } else {
            return Err(self.syntax("expected SOURCE"));
        };
        self.expect_delim(delim::RIGHT_PAREN)?;
        self.expect_kw(kw::WITH)?;
        let identifier = self.identifier()?;
        Ok(match option {
            TypeOption::CastToDistinct(_) => TypeOption::CastToDistinct(identifier),
            TypeOption::CastToSource(_) => TypeOption::CastToSource(identifier),
            TypeOption::Instantiable(_) | TypeOption::Final(_) | TypeOption::Reference | TypeOption::CastToRef | TypeOption::CastToType => {
                option
            }
        })
    }
}

//! The grammar of SQL/XML [ISO/IEC 9075-14:2006 draft §6]: the XML type
//! [§6.1], XMLCAST [§6.6], XMLDOCUMENT [§6.13], XMLPARSE [§6.16],
//! XMLSERIALIZE [§6.8], CAST's `<XML passing mechanism>` [§6.5], the
//! publishing operators (`publish`), XMLVALIDATE [§6.20] and the `<XML
//! valid according to clause>` [§11.6] (`valid`); and \[EXTENSION\] the
//! statements of registered XML Schemas (`registered`) and the mappings
//! of names (`names`).
//!
//! [§5.1] XML and the names of the operators are reserved words of
//! SQL/XML, and its other words non-reserved; as SQL-92's are the key
//! words this lexer reads (CONFORMANCE.md), each is read as an identifier
//! where the grammar takes it, as BOOLEAN and CLOB are.

mod map;
mod names;
pub use map::{MappedSource, mapped_sources};
mod publish;
mod query;
mod registered;
mod scoped;
mod valid;

use query::{XMLEXISTS, XMLQUERY};
pub(super) use valid::VALID;
use valid::{ELEMENT, XMLVALIDATE};

use super::Parser;
use crate::ast::{Passing, Returning, TypeName, ValueExpression, XmlOperator, XmlValidAccording};
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Tok, delim, kw};
use crate::value::DataType;
use crate::value::types::{REF_NAME, XML_NAME};
use crate::value::xml::{IMPLICIT, IMPLICIT_TYPING, Typing, XmlType};
use crate::xml::{Syntax, Whitespace};

// [§5.1] The words of SQL/XML this grammar reads.
/// `<XML cast specification>`'s.
pub const XMLCAST: &str = "XMLCAST";
/// `<XML document>`'s.
pub const XMLDOCUMENT: &str = "XMLDOCUMENT";
/// `<XML parse>`'s.
pub const XMLPARSE: &str = "XMLPARSE";
/// `<XML character string serialization>`'s.
pub const XMLSERIALIZE: &str = "XMLSERIALIZE";
/// DOCUMENT.
pub(super) const DOCUMENT: &str = "DOCUMENT";
/// CONTENT.
pub(super) const CONTENT: &str = "CONTENT";
/// SEQUENCE.
const SEQUENCE: &str = "SEQUENCE";
/// UNTYPED.
const UNTYPED: &str = "UNTYPED";
/// XMLSCHEMA.
const XMLSCHEMA: &str = "XMLSCHEMA";
/// STRIP, of STRIP WHITESPACE.
const STRIP: &str = "STRIP";
/// WHITESPACE.
const WHITESPACE: &str = "WHITESPACE";
/// ENCODING.
const ENCODING: &str = "ENCODING";
/// VERSION.
const VERSION: &str = "VERSION";
/// INCLUDING, of INCLUDING XMLDECLARATION.
const INCLUDING: &str = "INCLUDING";
/// EXCLUDING.
const EXCLUDING: &str = "EXCLUDING";
/// XMLDECLARATION.
const XMLDECLARATION: &str = "XMLDECLARATION";
/// [§11.4] RETURNING, of an `<XML returning clause>`.
const RETURNING: &str = "RETURNING";

/// [§6.1] A `<secondary XML type modifier>`, as read.
enum Secondary {
    /// ANY or UNTYPED.
    Typing(Typing),
    /// XMLSCHEMA and what indicates its registered XML Schema.
    Schema(Box<XmlValidAccording>),
}

impl Parser {
    /// Whether the `<XML type>` is next [§6.1].
    pub(super) fn at_xml_type(&self) -> bool {
        self.is_word(XML_NAME)
    }

    /// [§6.1] `<XML type>` ::= XML \[( `<XML type modifier>` )\]: XML alone
    /// \[SR2\], and DOCUMENT or CONTENT alone \[SR3b\], the implicit types
    /// (`value::xml::IMPLICIT`); of XMLSCHEMA, the registered XML Schema
    /// it indicates named, to be resolved as the statement is processed
    /// \[SR4\].
    pub(super) fn xml_type_name(&mut self) -> Result<TypeName> {
        self.expect_word(XML_NAME)?;
        if !self.accept_delim(delim::LEFT_PAREN) {
            return Ok(TypeName::Data(DataType::Xml(IMPLICIT)));
        }
        let primary = if self.accept_word(SEQUENCE) {
            None
        } else if self.accept_word(DOCUMENT) {
            Some(Syntax::Document)
        } else if self.accept_word(CONTENT) {
            Some(Syntax::Content)
        } else {
            return Err(self.syntax("expected DOCUMENT, CONTENT or SEQUENCE [ISO/IEC 9075-14:2006 draft §6.1]"));
        };
        let secondary = primary.map(|_| self.secondary_modifier()).transpose()?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(match (primary, secondary) {
            (Some(syntax), Some(Secondary::Schema(according))) => TypeName::XmlSchema(syntax, according),
            (Some(Syntax::Document), Some(Secondary::Typing(typing))) => TypeName::Data(DataType::Xml(XmlType::Document(typing))),
            (Some(Syntax::Content), Some(Secondary::Typing(typing))) => TypeName::Data(DataType::Xml(XmlType::Content(typing))),
            (None, _) | (Some(_), None) => TypeName::Data(DataType::Xml(XmlType::Sequence)),
        })
    }

    /// [§6.1] An `<XML type>` where a `<data type>` naming no type is
    /// read: not of XMLSCHEMA, which names a registered XML Schema.
    pub(super) fn xml_type(&mut self) -> Result<DataType> {
        let at = self.here();
        match self.xml_type_name()? {
            TypeName::Data(t) => Ok(t),
            TypeName::XmlSchema(..)
            | TypeName::Domain(_)
            | TypeName::Reference(..)
            | TypeName::Collection(..)
            | TypeName::Row(_)
            | TypeName::Extension { .. } => Err(SqlError::new(
                Code::SyntaxRule,
                "[ISO/IEC 9075-14:2006 draft §6.1 SR4] an XML type of XMLSCHEMA names a registered XML Schema, which is not taken here"
                    .to_owned(),
            )
            .or_at(at)),
        }
    }

    /// [§6.1] \[( `<secondary XML type modifier>` )\], after DOCUMENT or
    /// CONTENT: ANY, UNTYPED, or XMLSCHEMA `<XML valid according to what>`
    /// \[`<XML valid element clause>`\].
    fn secondary_modifier(&mut self) -> Result<Secondary> {
        if !self.accept_delim(delim::LEFT_PAREN) {
            return Ok(Secondary::Typing(IMPLICIT_TYPING));
        }
        let secondary = if self.accept_kw(kw::ANY) {
            Secondary::Typing(Typing::Any)
        } else if self.accept_word(UNTYPED) {
            Secondary::Typing(Typing::Untyped)
        } else if self.accept_word(XMLSCHEMA) {
            Secondary::Schema(Box::new(self.xml_schema_reference()?))
        } else {
            return Err(self.syntax("expected ANY, UNTYPED or XMLSCHEMA [ISO/IEC 9075-14:2006 draft §6.1]"));
        };
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(secondary)
    }

    /// Whether the SQL/XML operator `name` is next: its word, then "(".
    pub(super) fn at_xml(&self, name: &str) -> bool {
        self.is_word(name) && matches!(self.peek_at(1), Tok::Delimiter(delim::LEFT_PAREN))
    }

    /// [§6.10] An `<XML value function>`, or [§6.8] XMLSERIALIZE, if one
    /// is next: its word, no key word of SQL-92, then "(".
    pub(super) fn xml_primary(&mut self) -> Result<Option<ValueExpression>> {
        if self.at_xml(XMLCAST) {
            return self.xml_cast().map(Some);
        }
        if self.at_xml(XMLDOCUMENT) {
            return self.xml_document().map(Some);
        }
        if self.at_xml(XMLPARSE) {
            return self.xml_parse().map(Some);
        }
        if self.at_xml(XMLSERIALIZE) {
            return self.xml_serialize().map(Some);
        }
        if self.at_xml(XMLQUERY) {
            return self.xml_query().map(Some);
        }
        if self.at_xml(XMLEXISTS) {
            return self.xml_exists().map(Some);
        }
        if self.at_xml(map::XMLMAP) {
            return self.xml_map().map(Some);
        }
        if self.at_xml(XMLVALIDATE) {
            return self.xml_validate().map(Some);
        }
        if let Some(mapping) = self.xml_name_mapping()? {
            return Ok(Some(mapping));
        }
        self.xml_publishing()
    }

    /// [§6.6] `<XML cast specification>` ::= XMLCAST ( `<XML cast operand>`
    /// AS `<XML cast target>` \[`<XML passing mechanism>`\] ), the operand
    /// a `<value expression>` or NULL, the target a `<data type>` or a
    /// `<domain name>`.
    fn xml_cast(&mut self) -> Result<ValueExpression> {
        let at = self.here();
        self.expect_word(XMLCAST)?;
        self.expect_delim(delim::LEFT_PAREN)?;
        let operand = if self.accept_kw(kw::NULL) { None } else { Some(self.value_expression()?) };
        self.expect_kw(kw::AS)?;
        let target = self.type_name()?;
        let passing = self.passing_mechanism()?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(ValueExpression::Xml(Box::new(XmlOperator::Cast { operand, target, passing }), at))
    }

    /// [§6.13] `<XML document>` ::= XMLDOCUMENT ( `<XML value
    /// expression>` \[`<XML returning clause>`\] ).
    fn xml_document(&mut self) -> Result<ValueExpression> {
        let at = self.here();
        self.expect_word(XMLDOCUMENT)?;
        self.expect_delim(delim::LEFT_PAREN)?;
        let operand = self.value_expression()?;
        let returning = self.returning_clause()?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(ValueExpression::Xml(Box::new(XmlOperator::Document { operand, returning }), at))
    }

    /// [§11.4] `<XML returning clause>` ::= RETURNING { CONTENT | SEQUENCE
    /// }, if next.
    fn returning_clause(&mut self) -> Result<Option<Returning>> {
        if !self.accept_word(RETURNING) {
            return Ok(None);
        }
        if self.accept_word(CONTENT) {
            return Ok(Some(Returning::Content));
        }
        if self.accept_word(SEQUENCE) {
            return Ok(Some(Returning::Sequence));
        }
        Err(self.syntax("expected CONTENT or SEQUENCE [ISO/IEC 9075-14:2006 draft §11.4]"))
    }

    /// [§6.16] `<XML parse>` ::= XMLPARSE ( `<document or content>`
    /// `<string value expression>` `<XML whitespace option>` ), and
    /// `<XML whitespace option>` ::= { PRESERVE | STRIP } WHITESPACE.
    pub(super) fn xml_parse(&mut self) -> Result<ValueExpression> {
        let at = self.here();
        self.expect_word(XMLPARSE)?;
        self.expect_delim(delim::LEFT_PAREN)?;
        let syntax =
            self.document_or_content().ok_or_else(|| self.syntax("expected DOCUMENT or CONTENT [ISO/IEC 9075-14:2006 draft §6.16]"))?;
        let operand = self.value_expression()?;
        let whitespace = if self.accept_kw(kw::PRESERVE) {
            Whitespace::Preserve
        } else if self.accept_word(STRIP) {
            Whitespace::Strip
        } else {
            return Err(self.syntax("expected PRESERVE or STRIP [ISO/IEC 9075-14:2006 draft §6.16]"));
        };
        self.expect_word(WHITESPACE)?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(ValueExpression::Xml(Box::new(XmlOperator::Parse { syntax, operand, whitespace }), at))
    }

    /// [§6.8] `<document or content>`, if next. DOCUMENT and CONTENT are
    /// no reserved words, so one is a column of that name if what follows
    /// it ends a value expression: AS, ",", ")" or ".".
    fn document_or_content(&mut self) -> Option<Syntax> {
        let syntax = if self.is_word(DOCUMENT) {
            Syntax::Document
        } else if self.is_word(CONTENT) {
            Syntax::Content
        } else {
            return None;
        };
        let ends = matches!(self.peek_at(1), Tok::KeyWord(kw::AS) | Tok::Delimiter(delim::COMMA | delim::RIGHT_PAREN | delim::PERIOD));
        if ends {
            return None;
        }
        self.advance();
        Some(syntax)
    }

    /// [§6.8] `<XML character string serialization>` and `<XML binary
    /// string serialization>` ::= XMLSERIALIZE ( \[`<document or
    /// content>`\] `<XML value expression>` AS `<data type>` \[ENCODING
    /// `<XML encoding name>`\] \[VERSION `<character string literal>`\]
    /// \[{INCLUDING | EXCLUDING} XMLDECLARATION\] ): CONTENT if neither is
    /// written \[SR2a, SR4a\].
    pub(super) fn xml_serialize(&mut self) -> Result<ValueExpression> {
        let at = self.here();
        self.expect_word(XMLSERIALIZE)?;
        self.expect_delim(delim::LEFT_PAREN)?;
        let syntax = self.document_or_content().unwrap_or(Syntax::Content);
        let operand = self.value_expression()?;
        self.expect_kw(kw::AS)?;
        let target = self.data_type()?;
        let encoding = if self.accept_word(ENCODING) { Some(self.encoding_name()?) } else { None };
        let version = if self.accept_word(VERSION) { Some(self.character_string_literal("<XML serialize version>")?) } else { None };
        let declaration = if self.accept_word(INCLUDING) {
            self.expect_word(XMLDECLARATION)?;
            Some(true)
        } else if self.accept_word(EXCLUDING) {
            self.expect_word(XMLDECLARATION)?;
            Some(false)
        } else {
            None
        };
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(ValueExpression::Xml(Box::new(XmlOperator::Serialize { syntax, operand, target, encoding, version, declaration }), at))
    }

    /// [§6.5, §6.18] `<XML passing mechanism>` ::= BY REF | BY VALUE, if
    /// next.
    pub(super) fn passing_mechanism(&mut self) -> Result<Option<Passing>> {
        if !self.accept_kw(kw::BY) {
            return Ok(None);
        }
        if self.accept_word(REF_NAME) {
            return Ok(Some(Passing::ByRef));
        }
        if self.accept_kw(kw::VALUE) {
            return Ok(Some(Passing::ByValue));
        }
        Err(self.syntax("expected REF or VALUE [ISO/IEC 9075-14:2006 draft §6.5]"))
    }

    /// [§6.8] `<XML encoding name>` ::= `<SQL language identifier>`.
    fn encoding_name(&mut self) -> Result<String> {
        if let Tok::Identifier(name) = self.peek() {
            let name = name.clone();
            self.advance();
            return Ok(name);
        }
        Err(SqlError::at(Code::Syntax, self.here(), "expected an <XML encoding name> [ISO/IEC 9075-14:2006 draft §6.8]"))
    }
}

//! The grammar of what validates an XML value against a registered XML
//! Schema [ISO/IEC 9075-14:2006 draft §6.20, §8.5, §11.6]: XMLVALIDATE,
//! the second part of IS VALID, and the `<XML valid according to clause>`
//! each takes, whose `<XML valid according to what>` an XML type of
//! XMLSCHEMA writes too [§6.1].

use super::super::Parser;
use super::{CONTENT, DOCUMENT, SEQUENCE, XMLSCHEMA};
use crate::ast::{Dcs, ValueExpression, XmlOperator, XmlValidAccording, XmlValidWhat};
use crate::error::Result;
use crate::lexer::{delim, kw};

/// [§6.20] XMLVALIDATE.
pub const XMLVALIDATE: &str = "XMLVALIDATE";
/// [§8.5] VALID, of IS VALID.
pub(in crate::parser) const VALID: &str = "VALID";
/// [§11.6] ACCORDING, of an `<XML valid according to clause>`.
const ACCORDING: &str = "ACCORDING";
/// [§11.6] ID, of a registered XML Schema.
const ID: &str = "ID";
/// URI.
const URI: &str = "URI";
/// LOCATION.
pub(super) const LOCATION: &str = "LOCATION";
/// NAMESPACE.
const NAMESPACE: &str = "NAMESPACE";
/// ELEMENT.
pub(super) const ELEMENT: &str = "ELEMENT";

impl Parser {
    /// [§6.20] `<XML validate>` ::= XMLVALIDATE ( `<document or content or
    /// sequence>` `<XML value expression>` \[`<XML valid according to
    /// clause>`\] ).
    pub(super) fn xml_validate(&mut self) -> Result<ValueExpression> {
        let at = self.here();
        self.expect_word(XMLVALIDATE)?;
        self.expect_delim(delim::LEFT_PAREN)?;
        let dcs = self.document_or_content_or_sequence()?;
        let operand = self.value_expression()?;
        let according = self.xml_valid_according_clause()?.map(Box::new);
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(ValueExpression::Xml(Box::new(XmlOperator::Validate { dcs, operand, according }), at))
    }

    /// [§8.5] The rest of an `<XML valid predicate part 2>` ::= IS \[NOT\]
    /// VALID `<document or content or sequence>` \[`<XML valid according
    /// to clause>`\], after VALID.
    pub(in crate::parser) fn xml_valid_rest(&mut self) -> Result<(Dcs, Option<Box<XmlValidAccording>>)> {
        let dcs = self.document_or_content_or_sequence()?;
        Ok((dcs, self.xml_valid_according_clause()?.map(Box::new)))
    }

    /// [§6.20] `<document or content or sequence>` ::= DOCUMENT | CONTENT |
    /// SEQUENCE.
    fn document_or_content_or_sequence(&mut self) -> Result<Dcs> {
        if self.accept_word(DOCUMENT) {
            return Ok(Dcs::Document);
        }
        if self.accept_word(CONTENT) {
            return Ok(Dcs::Content);
        }
        if self.accept_word(SEQUENCE) {
            return Ok(Dcs::Sequence);
        }
        Err(self.syntax("expected DOCUMENT, CONTENT or SEQUENCE [ISO/IEC 9075-14:2006 draft §6.20]"))
    }

    /// [§11.6] `<XML valid according to clause>` ::= ACCORDING TO
    /// XMLSCHEMA `<XML valid according to what>` \[`<XML valid element
    /// clause>`\], if ACCORDING is next.
    fn xml_valid_according_clause(&mut self) -> Result<Option<XmlValidAccording>> {
        if !self.accept_word(ACCORDING) {
            return Ok(None);
        }
        self.expect_kw(kw::TO)?;
        self.expect_word(XMLSCHEMA)?;
        self.xml_schema_reference().map(Some)
    }

    /// [§11.6] `<XML valid according to what>` \[`<XML valid element
    /// clause>`\]: URI `<XML URI>` \[LOCATION `<XML URI>`\], NO NAMESPACE
    /// \[LOCATION `<XML URI>`\], or ID `<registered XML Schema name>`; then
    /// ELEMENT `<identifier>`, or NO NAMESPACE or NAMESPACE `<XML URI>`,
    /// each \[ELEMENT `<identifier>`\]. NO NAMESPACE is the zero-length
    /// URI [SR2a i, SR4a i 1].
    pub(super) fn xml_schema_reference(&mut self) -> Result<XmlValidAccording> {
        let at = self.here();
        let what = if self.accept_word(ID) {
            XmlValidWhat::Id(self.table_name()?)
        } else {
            let namespace = if self.accept_word(URI) {
                self.character_string_literal("<XML valid target namespace URI>")?
            } else {
                self.expect_kw(kw::NO)?;
                self.expect_word(NAMESPACE)?;
                String::new()
            };
            let location =
                if self.accept_word(LOCATION) { Some(self.character_string_literal("<XML valid schema location URI>")?) } else { None };
            XmlValidWhat::Uri { namespace, location }
        };
        let namespace = if self.accept_kw(kw::NO) {
            self.expect_word(NAMESPACE)?;
            Some(String::new())
        } else if self.accept_word(NAMESPACE) {
            Some(self.character_string_literal("<XML valid element namespace URI>")?)
        } else {
            None
        };
        let element = if self.accept_word(ELEMENT) { Some(self.identifier()?) } else { None };
        Ok(XmlValidAccording { what, namespace, element, at })
    }
}

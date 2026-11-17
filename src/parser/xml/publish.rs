//! The grammar of SQL/XML's publishing operators [ISO/IEC 9075-14:2006
//! draft §6]: XMLCOMMENT [§6.11], XMLCONCAT [§6.12], XMLELEMENT [§6.14],
//! XMLFOREST [§6.15], XMLPI [§6.17] and XMLTEXT [§6.19], XMLAGG [§11.2],
//! and the `<XML namespace declaration>` [§11.3].

use super::super::Parser;
use super::CONTENT;
use crate::ast::{
    ContentOption, ElementConstructor, ForestConstructor, Named, NamespaceItem, SetArg, SetFunctionSpecification, ValueExpression,
    XmlOperator,
};
use crate::error::Result;
use crate::functions::XMLAGG;
use crate::lexer::{delim, kw};

/// `<XML comment>`'s word.
const XMLCOMMENT: &str = "XMLCOMMENT";
/// `<XML concatenation>`'s.
const XMLCONCAT: &str = "XMLCONCAT";
/// `<XML PI>`'s.
const XMLPI: &str = "XMLPI";
/// `<XML text>`'s.
const XMLTEXT: &str = "XMLTEXT";
/// `<XML element>`'s.
const XMLELEMENT: &str = "XMLELEMENT";
/// `<XML forest>`'s.
const XMLFOREST: &str = "XMLFOREST";
/// `<XML attributes>`'.
const XMLATTRIBUTES: &str = "XMLATTRIBUTES";
/// [§11.3] `<XML namespace declaration>`'s.
pub(in crate::parser) const XMLNAMESPACES: &str = "XMLNAMESPACES";
/// EMPTY, of EMPTY ON NULL.
const EMPTY: &str = "EMPTY";
/// ABSENT, of ABSENT ON NULL.
const ABSENT: &str = "ABSENT";
/// NIL, of NIL ON NULL and NIL ON NO CONTENT.
const NIL: &str = "NIL";
/// NAME, before a name the operator makes.
const NAME: &str = "NAME";

impl Parser {
    /// A publishing operator, if one is next.
    pub(super) fn xml_publishing(&mut self) -> Result<Option<ValueExpression>> {
        let at = self.here();
        let operator = if self.at_xml(XMLCOMMENT) {
            self.xml_comment()?
        } else if self.at_xml(XMLCONCAT) {
            self.xml_concatenation()?
        } else if self.at_xml(XMLPI) {
            self.xml_pi()?
        } else if self.at_xml(XMLTEXT) {
            self.xml_text()?
        } else if self.at_xml(XMLELEMENT) {
            self.xml_element()?
        } else if self.at_xml(XMLFOREST) {
            self.xml_forest()?
        } else if self.at_xml(XMLAGG) {
            return self.xml_aggregate().map(|sf| Some(ValueExpression::SetFunction(sf)));
        } else {
            return Ok(None);
        };
        Ok(Some(ValueExpression::Xml(Box::new(operator), at)))
    }

    /// [§6.11] `<XML comment>` ::= XMLCOMMENT ( `<character value
    /// expression>` \[`<XML returning clause>`\] ).
    fn xml_comment(&mut self) -> Result<XmlOperator> {
        self.expect_word(XMLCOMMENT)?;
        self.expect_delim(delim::LEFT_PAREN)?;
        let operand = self.value_expression()?;
        let returning = self.returning_clause()?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(XmlOperator::Comment { operand, returning })
    }

    /// [§6.12] `<XML concatenation>` ::= XMLCONCAT ( `<XML value
    /// expression>` { , `<XML value expression>` }... \[`<XML returning
    /// clause>`\] ): two values or more.
    fn xml_concatenation(&mut self) -> Result<XmlOperator> {
        self.expect_word(XMLCONCAT)?;
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut operands = vec![self.value_expression()?];
        self.expect_delim(delim::COMMA)?;
        operands.push(self.value_expression()?);
        while self.accept_delim(delim::COMMA) {
            operands.push(self.value_expression()?);
        }
        let returning = self.returning_clause()?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(XmlOperator::Concat { operands, returning })
    }

    /// [§6.17] `<XML PI>` ::= XMLPI ( NAME `<XML PI target>` \[ ,
    /// `<character value expression>` \] \[`<XML returning clause>`\] ),
    /// and `<XML PI target>` ::= `<identifier>`.
    fn xml_pi(&mut self) -> Result<XmlOperator> {
        self.expect_word(XMLPI)?;
        self.expect_delim(delim::LEFT_PAREN)?;
        self.expect_word(NAME)?;
        let target = self.identifier()?;
        let operand = if self.accept_delim(delim::COMMA) { Some(self.value_expression()?) } else { None };
        let returning = self.returning_clause()?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(XmlOperator::Pi { target, operand, returning })
    }

    /// [§6.19] `<XML text>` ::= XMLTEXT ( `<character value expression>`
    /// \[`<XML returning clause>`\] ).
    fn xml_text(&mut self) -> Result<XmlOperator> {
        self.expect_word(XMLTEXT)?;
        self.expect_delim(delim::LEFT_PAREN)?;
        let operand = self.value_expression()?;
        let returning = self.returning_clause()?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(XmlOperator::Text { operand, returning })
    }

    /// [§6.14] `<XML element>` ::= XMLELEMENT ( NAME `<XML element name>`
    /// \[ , `<XML namespace declaration>` \] \[ , `<XML attributes>` \] \[ {
    /// , `<XML element content>` }... \[ OPTION `<XML content option>` \]
    /// \] \[`<XML returning clause>`\] ).
    fn xml_element(&mut self) -> Result<XmlOperator> {
        self.expect_word(XMLELEMENT)?;
        self.expect_delim(delim::LEFT_PAREN)?;
        self.expect_word(NAME)?;
        let name = self.identifier()?;
        let mut namespaces = None;
        let mut attributes = Vec::new();
        let mut content = Vec::new();
        while self.accept_delim(delim::COMMA) {
            if content.is_empty() && attributes.is_empty() && namespaces.is_none() && self.at_xml(XMLNAMESPACES) {
                namespaces = Some(self.xml_namespace_declaration()?);
            } else if content.is_empty() && attributes.is_empty() && self.at_xml(XMLATTRIBUTES) {
                attributes = self.xml_attributes()?;
            } else {
                content.push(self.value_expression()?);
            }
        }
        let option = if content.is_empty() { None } else { self.content_option()? };
        let returning = self.returning_clause()?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(XmlOperator::Element(ElementConstructor { name, namespaces, attributes, content, option, returning }))
    }

    /// [§6.14] `<XML attributes>` ::= XMLATTRIBUTES ( `<XML attribute>` \[
    /// { , `<XML attribute>` }... \] ).
    fn xml_attributes(&mut self) -> Result<Vec<Named>> {
        self.expect_word(XMLATTRIBUTES)?;
        self.expect_delim(delim::LEFT_PAREN)?;
        let attributes = self.named_list()?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(attributes)
    }

    /// [§6.14] `<XML attribute>` { , `<XML attribute>` }..., or [§6.15]
    /// `<forest element list>`: each a `<value expression>` \[ AS
    /// `<identifier>` \].
    fn named_list(&mut self) -> Result<Vec<Named>> {
        let mut list = Vec::new();
        loop {
            let value = self.value_expression()?;
            let name = if self.accept_kw(kw::AS) { Some(self.identifier()?) } else { None };
            list.push(Named { value, name });
            if !self.accept_delim(delim::COMMA) {
                return Ok(list);
            }
        }
    }

    /// [§6.15] `<XML forest>` ::= XMLFOREST ( \[ `<XML namespace
    /// declaration>` , \] `<forest element list>` \[ OPTION `<XML content
    /// option>` \] \[`<XML returning clause>`\] ).
    fn xml_forest(&mut self) -> Result<XmlOperator> {
        self.expect_word(XMLFOREST)?;
        self.expect_delim(delim::LEFT_PAREN)?;
        let namespaces = if self.at_xml(XMLNAMESPACES) {
            let items = self.xml_namespace_declaration()?;
            self.expect_delim(delim::COMMA)?;
            Some(items)
        } else {
            None
        };
        let elements = self.named_list()?;
        let option = self.content_option()?;
        let returning = self.returning_clause()?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(XmlOperator::Forest(ForestConstructor { namespaces, elements, option, returning }))
    }

    /// [§6.14] OPTION `<XML content option>`, if next: NULL ON NULL, EMPTY
    /// ON NULL, ABSENT ON NULL, NIL ON NULL or NIL ON NO CONTENT.
    fn content_option(&mut self) -> Result<Option<ContentOption>> {
        if !self.accept_kw(kw::OPTION) {
            return Ok(None);
        }
        let option = if self.accept_kw(kw::NULL) {
            ContentOption::NullOnNull
        } else if self.accept_word(EMPTY) {
            ContentOption::EmptyOnNull
        } else if self.accept_word(ABSENT) {
            ContentOption::AbsentOnNull
        } else if self.accept_word(NIL) {
            self.expect_kw(kw::ON)?;
            if self.accept_kw(kw::NO) {
                self.expect_word(CONTENT)?;
                return Ok(Some(ContentOption::NilOnNoContent));
            }
            self.expect_kw(kw::NULL)?;
            return Ok(Some(ContentOption::NilOnNull));
        } else {
            return Err(self.syntax("expected NULL, EMPTY, ABSENT or NIL [ISO/IEC 9075-14:2006 draft §6.14]"));
        };
        self.expect_kw(kw::ON)?;
        self.expect_kw(kw::NULL)?;
        Ok(Some(option))
    }

    /// [§11.3] `<XML namespace declaration>` ::= XMLNAMESPACES ( `<XML
    /// namespace declaration item>` \[ { , `<XML namespace declaration
    /// item>` }... \] ), each `<XML namespace URI>` AS `<XML namespace
    /// prefix>`, DEFAULT `<XML namespace URI>` or NO DEFAULT.
    pub(in crate::parser) fn xml_namespace_declaration(&mut self) -> Result<Vec<NamespaceItem>> {
        self.expect_word(XMLNAMESPACES)?;
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut items = Vec::new();
        loop {
            let item = if self.accept_kw(kw::DEFAULT) {
                NamespaceItem::Default(self.character_string_literal("<XML namespace URI>")?)
            } else if self.accept_kw(kw::NO) {
                self.expect_kw(kw::DEFAULT)?;
                NamespaceItem::NoDefault
            } else {
                let uri = self.character_string_literal("<XML namespace URI>")?;
                self.expect_kw(kw::AS)?;
                NamespaceItem::Regular { uri, prefix: self.identifier()? }
            };
            items.push(item);
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(items)
    }

    /// [§11.2] `<XML aggregate>` ::= XMLAGG ( `<XML value expression>` \[
    /// ORDER BY `<sort specification list>` \] \[`<XML returning clause>`\]
    /// ), an `<aggregate function>`.
    fn xml_aggregate(&mut self) -> Result<SetFunctionSpecification> {
        let at = self.here();
        self.expect_word(XMLAGG)?;
        self.expect_delim(delim::LEFT_PAREN)?;
        let value = self.nested(Parser::value_expression)?;
        let mut order_by = Vec::new();
        if self.accept_kw(kw::ORDER) {
            self.expect_kw(kw::BY)?;
            order_by = self.sort_specification_list()?;
        }
        let returning = self.returning_clause()?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        let arg = SetArg::All(Box::new(value));
        Ok(SetFunctionSpecification { name: XMLAGG, arg, order_by, listagg: None, filter: None, semantics: None, returning, at })
    }
}

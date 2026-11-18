//! Constructors compiled [XQuery §3.7]: a direct element's namespace
//! declaration attributes changing the static context within it [XQuery
//! §3.7.1.2], its names expanded and its attributes' names distinct
//! \[err:XQST0040\], its literal characters normalized [XQuery §3.7.1.1]
//! and its boundary whitespace removed by the boundary-space policy
//! [XQuery §3.7.1.4]; a computed one's name expanded, or the namespaces a
//! name expression's string is resolved by kept [XQuery §3.7.3.1].

use std::sync::Arc;

use super::super::ast::{AttributeValuePart, ComputedName, Constructor, DirAttribute, DirContent, DirElement, PiName, boundary_whitespace};
use super::super::error::{ErrorCode, error};
use super::super::plan::{ComputedElement, Content, DirectElement, NameOp, Op, Part, PiOp, StaticNames};
use super::Analysis;
use super::scope::{Unprefixed, display};
use crate::error::Result;
use crate::xml::{Binding, QName, is_space};

/// The name of a namespace declaration attribute, and the prefix of the
/// others [XQuery §3.7.1.2].
const XMLNS: &str = "xmlns";
/// [XML §3.3.3] What attribute value normalization makes of each white
/// space character written.
const SPACE: char = ' ';

impl Analysis<'_> {
    /// A constructor compiled [XQuery §3.7].
    pub(super) fn constructor_expr(&mut self, c: &Constructor) -> Result<Op> {
        Ok(match c {
            Constructor::Element(e) => Op::Element(Box::new(self.direct_element(e)?)),
            Constructor::Comment(text) => Op::DirectComment(text.as_str().into()),
            Constructor::Pi(pi) => {
                Op::Pi(Box::new(PiOp { target: Ok(pi.target.as_str().into()), content: Ok(pi.contents.as_str().into()) }))
            }
            Constructor::Document(content) => Op::Document(Box::new(self.expr(content)?)),
            Constructor::ComputedElement(name, content) => {
                let name = self.computed_name(name, Unprefixed::Element)?;
                let content = content.as_ref().map(|e| self.expr(e)).transpose()?;
                Op::ComputedElement(Box::new(ComputedElement { name, content, inherited: self.scope.names.constructed.clone() }))
            }
            Constructor::ComputedAttribute(name, content) => {
                let name = self.computed_name(name, Unprefixed::None)?;
                Op::Attribute(Box::new(name), content.as_ref().map(|e| self.expr(e).map(Box::new)).transpose()?)
            }
            Constructor::Text(content) => Op::Text(Box::new(self.expr(content)?)),
            Constructor::ComputedComment(content) => Op::Comment(Box::new(self.expr(content)?)),
            Constructor::ComputedPi(target, content) => {
                let target = match target {
                    PiName::Name(name) => Ok(name.as_str().into()),
                    PiName::Expr(e) => Err(self.expr(e)?),
                };
                let content = Err(content.as_ref().map(|e| self.expr(e)).transpose()?);
                Op::Pi(Box::new(PiOp { target, content }))
            }
        })
    }

    /// The name of a computed element or attribute [XQuery §3.7.3.1,
    /// §3.7.3.2]: a QName expanded \[err:XPST0081\], or a name expression
    /// and the namespaces its string is resolved by.
    fn computed_name(&mut self, name: &ComputedName, unprefixed: Unprefixed) -> Result<NameOp> {
        Ok(match name {
            ComputedName::Name(n) => NameOp::Fixed(self.scope.name(n, unprefixed)?),
            ComputedName::Expr(e) => {
                let default_element = if unprefixed == Unprefixed::Element { self.scope.names.default_element.clone() } else { None };
                let names = StaticNames { prefixes: self.scope.names.known(), default_element };
                NameOp::Computed(self.expr(e)?, Arc::new(names))
            }
        })
    }

    /// A direct element constructor compiled [XQuery §3.7.1], its
    /// namespace declaration attributes in the static context within it
    /// alone.
    pub(super) fn direct_element(&mut self, e: &DirElement) -> Result<DirectElement> {
        let saved = (self.scope.names.prefixes.len(), self.scope.names.default_element.clone(), self.scope.names.constructed.len());
        let inherited = self.scope.names.constructed.clone();
        let compiled = self.direct_element_within(e, inherited);
        self.scope.names.prefixes.truncate(saved.0);
        self.scope.names.default_element = saved.1;
        self.scope.names.constructed.truncate(saved.2);
        compiled
    }

    /// As `direct_element`, the static context to be restored after.
    fn direct_element_within(&mut self, e: &DirElement, inherited: Vec<Binding>) -> Result<DirectElement> {
        let mut declared = Vec::new();
        for attribute in &e.attributes {
            if let Some(binding) = namespace_declaration(attribute) {
                self.declare(&binding);
                declared.push(binding);
            }
        }
        let name = self.scope.expand(&e.name, Unprefixed::Element, ErrorCode::XPST0008)?;
        let mut attributes: Vec<(QName, Vec<Part>)> = Vec::new();
        for attribute in e.attributes.iter().filter(|a| namespace_declaration(a).is_none()) {
            let attribute_name = self.scope.expand(&attribute.name, Unprefixed::None, ErrorCode::XPST0008)?;
            if attributes.iter().any(|(other, _)| other.same(&attribute_name)) {
                let why = format!("a direct element constructor has two attributes named {} [XQuery §3.7.1.1]", display(&attribute_name));
                return Err(error(ErrorCode::XQST0040, &why));
            }
            let value = self.attribute_value(&attribute.value)?;
            attributes.push((attribute_name, value));
        }
        let content = self.content(&e.content)?;
        Ok(DirectElement { name, declared, inherited, attributes, content })
    }

    /// `binding`, of a namespace declaration attribute, in the static
    /// context within its element [XQuery §3.7.1.2]: a prefix bound or
    /// undeclared, or the default element/type namespace set or "none".
    fn declare(&mut self, binding: &Binding) {
        match &binding.prefix {
            Some(prefix) => self.scope.names.prefixes.push((prefix.clone(), binding.uri.clone())),
            None => self.scope.names.default_element = Some(binding.uri.clone()).filter(|u| !u.is_empty()),
        }
        self.scope.names.constructed.push(binding.clone());
    }

    /// The parts of a direct attribute's value [XQuery §3.7.1.1]: each run
    /// of characters written normalized as a CDATA attribute is [XML
    /// §3.3.3], each white space character a space, a reference's
    /// character kept; each enclosed expression compiled.
    fn attribute_value(&mut self, value: &[AttributeValuePart]) -> Result<Vec<Part>> {
        let mut parts = Vec::new();
        let mut text = String::new();
        for part in value {
            match part {
                AttributeValuePart::Chars(chars) => text.extend(chars.chars().map(|c| if is_space(c) { SPACE } else { c })),
                AttributeValuePart::Reference(c) => text.push(*c),
                AttributeValuePart::Enclosed(e) => {
                    if !text.is_empty() {
                        parts.push(Part::Text(std::mem::take(&mut text).into()));
                    }
                    parts.push(Part::Enclosed(self.expr(e)?));
                }
            }
        }
        if !text.is_empty() {
            parts.push(Part::Text(text.into()));
        }
        Ok(parts)
    }

    /// A direct element's content [XQuery §3.7.1.3]: its boundary
    /// whitespace removed unless the boundary-space policy preserves it
    /// [XQuery §3.7.1.4], references and CDATA sections their characters,
    /// adjacent characters one text.
    fn content(&mut self, content: &[DirContent]) -> Result<Vec<Content>> {
        let mut out = Vec::new();
        let mut text = String::new();
        for (at, part) in content.iter().enumerate() {
            if !self.boundary_preserve && boundary_whitespace(content, at) {
                continue;
            }
            let compiled = match part {
                DirContent::Chars(chars) | DirContent::CData(chars) => {
                    text.push_str(chars);
                    continue;
                }
                DirContent::Reference(c) => {
                    text.push(*c);
                    continue;
                }
                DirContent::Element(e) => Content::Element(self.direct_element(e)?),
                DirContent::Comment(c) => Content::Comment(c.as_str().into()),
                DirContent::Pi(pi) => Content::Pi(pi.target.as_str().into(), pi.contents.as_str().into()),
                DirContent::Enclosed(e) => Content::Enclosed(self.expr(e)?),
            };
            if !text.is_empty() {
                out.push(Content::Text(std::mem::take(&mut text).into()));
            }
            out.push(compiled);
        }
        if !text.is_empty() {
            out.push(Content::Text(text.into()));
        }
        Ok(out)
    }
}

/// The binding `attribute` makes, if it is a namespace declaration
/// attribute [XQuery §3.7.1.2], whose value the parser has checked is a
/// URI literal.
fn namespace_declaration(attribute: &DirAttribute) -> Option<Binding> {
    let prefix = match &attribute.name.prefix {
        None => {
            if attribute.name.local != XMLNS {
                return None;
            }
            None
        }
        Some(p) => {
            if p != XMLNS {
                return None;
            }
            Some(attribute.name.local.as_str().into())
        }
    };
    let mut uri = String::new();
    for part in &attribute.value {
        match part {
            AttributeValuePart::Chars(chars) => uri.push_str(chars),
            AttributeValuePart::Reference(c) => uri.push(*c),
            AttributeValuePart::Enclosed(_) => {}
        }
    }
    Some(Binding { prefix, uri: uri.into() })
}

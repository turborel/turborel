//! Binding XMLELEMENT [ISO/IEC 9075-14:2006 draft §6.14] and XMLFOREST
//! [§6.15], each a call of its function (`functions/xml/element.rs`), and
//! RETURNING CONTENT a call of XMLDOCUMENT's of that.

use std::sync::Arc;

use super::super::{Binder, Scope, rule};
use super::publish::returned;
use crate::ast::{ContentOption, ElementConstructor, ForestConstructor, Named, ValueExpression};
use crate::error::{Code, Result, SqlError};
use crate::functions::{IMPLICIT_BINARY_ENCODING, ScalarRef, XmlElement, XmlForest, xml_schema_type};
use crate::logical::Expr;
use crate::value::DataType;
use crate::xml::{Escape, QName, Version, XSI_NAMESPACE, name_of, qname_parts};

/// [§6.14 SR8a i] The `<XML content option>` of an XMLELEMENT of content
/// that writes none.
pub const ELEMENT_OPTION: ContentOption = ContentOption::EmptyOnNull;

/// [§6.15 SR3] That of an XMLFOREST that writes none.
pub const FOREST_OPTION: ContentOption = ContentOption::NullOnNull;

/// [§6.14 SR3f] The name no attribute has, nor any's prefix.
const XMLNS: &str = "xmlns";

/// [§6.14 SR9] The local name of `xsi:nil`.
const NIL: &str = "nil";

/// The name `named` gives its value, written: AS's `<identifier>`, else,
/// of a column reference, its column's name mapped to an XML Name fully
/// escaped [§9.1]; `clause` the rule that asks for one.
fn given_name(named: &Named, clause: &str) -> Result<String> {
    if let Some(name) = &named.name {
        return Ok(name.clone());
    }
    if let ValueExpression::Column(c) = &named.value {
        return Ok(name_of(&c.column, Escape::Full));
    }
    Err(rule(format!("[ISO/IEC 9075-14:2006 draft {clause}] a value that is no column reference is named by AS")))
}

impl Binder<'_> {
    /// The XML QName `text`, the `what`, names: an XML 1.1 QName, else the
    /// error citing `clause`, its namespace URI by §10.12.
    fn xml_qname(&self, text: &str, what: &str, clause: &str) -> Result<QName> {
        let Some((prefix, local)) = qname_parts(Version::V11, text) else {
            return Err(rule(format!("[ISO/IEC 9075-14:2006 draft {clause}] the {what} {text} is no XML 1.1 QName")));
        };
        let uri = self.namespace_uri(prefix, &format!("the {what} {text}"))?;
        Ok(QName { prefix: prefix.map(Into::into), uri, local: local.into() })
    }

    /// `operand`, bound as a value §9.8 maps [§6.14 SR3c, SR7, §6.15
    /// SR2b]: an XML value if `xml`; else of a type §9.5 maps, a distinct
    /// type's as its source's, a datetime as the SQL-session shows it
    /// [§9.8 GR7]. A collection is valid SQL not yet taken, SQLCODE -900.
    fn xml_mapped(&self, operand: &ValueExpression, scope: Scope<'_>, xml: bool, clause: &str) -> Result<Expr> {
        let (e, t) = self.value_expression(operand, scope)?;
        let represented = t.represented().clone();
        if matches!(represented, DataType::Xml(_)) && xml {
            return Ok(e);
        }
        if matches!(represented, DataType::Array(_) | DataType::Multiset(_)) && xml {
            return Err(SqlError::new(
                Code::Unsupported,
                format!("[ISO/IEC 9075-14:2006 draft §9.8 GR9j] {t} mapped to XML is not yet supported"),
            ));
        }
        if matches!(represented, DataType::Xml(_)) || xml_schema_type(&t).is_none() {
            return Err(rule(format!("[ISO/IEC 9075-14:2006 draft {clause}] a value of {t} is not mapped to XML here")));
        }
        Ok(super::super::datetime::shown(e, represented, self.now))
    }

    /// [§6.14 SR3] The name of attribute `a`: a QName, not `xmlns` nor of
    /// its prefix \[SR3d to SR3f\], in the namespace §10.12 gives \[SR3g\].
    fn attribute_name(&self, a: &Named) -> Result<QName> {
        let text = given_name(a, "§6.14 SR3d ii")?;
        if text == XMLNS || text.split_once(':').is_some_and(|(p, _)| p == XMLNS) {
            return Err(rule(format!("[ISO/IEC 9075-14:2006 draft §6.14 SR3f] no attribute is named {text}")));
        }
        self.xml_qname(&text, "attribute name", "§6.14 SR3e")
    }

    /// [§6.14] XMLELEMENT `x`, its declaration in scope within it \[SR1\].
    pub(super) fn xml_element(&self, x: &ElementConstructor, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        self.in_namespaces(x.namespaces.as_deref(), || {
            let name = self.xml_qname(&x.name, "element name", "§6.14 SR5")?;
            let mut names: Vec<QName> = Vec::with_capacity(x.attributes.len());
            let mut args = Vec::with_capacity(x.attributes.len().saturating_add(x.content.len()));
            for a in &x.attributes {
                let n = self.attribute_name(a)?;
                if names.iter().any(|m| m.same(&n)) {
                    return Err(rule(format!("[ISO/IEC 9075-14:2006 draft §6.14 SR4] two attributes are named {}", n.local)));
                }
                if !x.content.is_empty() && n.uri.as_deref() == Some(XSI_NAMESPACE) && &*n.local == NIL {
                    return Err(rule("[ISO/IEC 9075-14:2006 draft §6.14 SR9] an element of content has no attribute xsi:nil"));
                }
                args.push(self.xml_mapped(&a.value, scope, false, "§6.14 SR3c")?);
                names.push(n);
            }
            for c in &x.content {
                args.push(self.xml_mapped(c, scope, true, "§6.14 SR7")?);
            }
            // [SR8] The content option, of content alone.
            let option = (!x.content.is_empty()).then(|| x.option.unwrap_or(ELEMENT_OPTION));
            let element = XmlElement { name, attributes: names, option, encoding: IMPLICIT_BINARY_ENCODING };
            Ok(returned(Expr::Call(ScalarRef(Arc::new(element)), args), x.returning))
        })
    }

    /// [§6.15] XMLFOREST `x`, its declaration in scope within it \[SR1\].
    pub(super) fn xml_forest(&self, x: &ForestConstructor, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        self.in_namespaces(x.namespaces.as_deref(), || {
            let mut names = Vec::with_capacity(x.elements.len());
            let mut args = Vec::with_capacity(x.elements.len());
            for e in &x.elements {
                let text = given_name(e, "§6.15 SR2c ii")?;
                names.push(self.xml_qname(&text, "forest element name", "§6.15 SR2d")?);
                args.push(self.xml_mapped(&e.value, scope, true, "§6.15 SR2b")?);
            }
            let forest = XmlForest { names, option: x.option.unwrap_or(FOREST_OPTION), encoding: IMPLICIT_BINARY_ENCODING };
            Ok(returned(Expr::Call(ScalarRef(Arc::new(forest)), args), x.returning))
        })
    }
}

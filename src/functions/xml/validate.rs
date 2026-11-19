//! XMLVALIDATE [ISO/IEC 9075-14:2006 draft §6.20] and IS VALID [§8.5]:
//! each element of an XML value validated against a registered XML
//! Schema, the one an `<XML valid according to clause>` indicates or, of
//! none, the one its namespace chooses, by §10.22, which is XQuery's
//! `validate strict { $SEQ }` (`xml::schema::validate`).
//!
//! As §6.20 GR6 and §8.5 GR7 say: of DOCUMENT, the document node is what
//! is validated, so its identity constraints and IDs are checked
//! [XQuery §3.13]; of CONTENT and SEQUENCE, each element child of a
//! document node is validated alone, as an element node, so they are not.

use std::sync::Arc;

use super::super::ScalarFunction;
use crate::ast::Dcs;
use crate::error::{Code, Result, SqlError, SqlState};
use crate::value::xml::XmlValue;
use crate::value::{DataType, Value};
use crate::xml::schema::{Failure, Mode, Schema, validate};
use crate::xml::{Builder, Item, Kind, Node, Sequence};

/// [§11.6] What an `<XML valid according to clause>` indicates: the
/// registered XML Schema RXS, read, and ENSURI and EN, if any.
#[derive(Debug)]
pub struct According {
    /// RXS's `<registered XML Schema name>`, for messages.
    pub name: String,
    /// RXS's schema.
    pub schema: Arc<Schema>,
    /// The indicated XML namespace ENSURI, of an element clause.
    pub namespace: Option<String>,
    /// The NCName EN of the indicated global element declaration.
    pub element: Option<String>,
}

/// [§6.20 GR6a iii 2 A II 2] The registered XML Schema the elements of a
/// namespace are validated by, of a validation of no `<XML valid
/// according to clause>`, chosen as the value is bound.
#[derive(Debug)]
pub struct Candidate {
    /// Its target namespace URI, the zero-length string of none.
    pub namespace: String,
    /// Its `<registered XML Schema name>`, for messages.
    pub name: String,
    /// Its schema; none of a permanently registered one, whose components
    /// turborel does not hold.
    pub schema: Option<Arc<Schema>>,
}

/// [§6.20 GR6, §8.5 GR7] How each element validated finds its registered
/// XML Schema.
#[derive(Debug)]
pub enum ValidBy {
    /// The one an `<XML valid according to clause>` indicates.
    According(According),
    /// Data-driven: of each namespace, the one chosen, if any.
    Namespaces(Vec<Candidate>),
}

/// [§6.20 GR6a iii 2 B, GR6b ii] An element not of the name or namespace
/// an element clause indicates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mismatch {
    /// Not of the QName ENSURI and EN.
    QName,
    /// Not of the namespace ENSURI.
    Namespace,
}

/// What validating one element came to.
#[derive(Debug)]
enum Checked {
    /// [§10.22] SUCCESS, and the node validated.
    Valid(Node),
    /// [§10.22] FAILURE.
    Invalid(Failure),
    /// No registered XML Schema of the element's namespace.
    NoSchema(String),
    /// Not of the element clause's name or namespace.
    Other(Mismatch),
}

/// The namespace URI of element `e`, the zero-length string of none.
fn namespace_of(e: &Node) -> &str {
    e.tree().name(e.id()).and_then(|n| n.uri.as_deref()).unwrap_or_default()
}

impl ValidBy {
    /// [§6.20 GR6a iii 2 A, B; GR6b i, ii] [§8.5 GR7] Element `element`,
    /// validated by §10.22 as `item`: itself, or of DOCUMENT the document
    /// node it is the child of.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 if the registered XML Schema chosen is permanently
    /// registered; the errors of validating.
    fn check(&self, element: &Node, item: &Node) -> Result<Checked> {
        let schema = match self {
            ValidBy::According(a) => {
                if let Some(m) = a.mismatch(element) {
                    return Ok(Checked::Other(m));
                }
                &a.schema
            }
            ValidBy::Namespaces(candidates) => {
                let ns = namespace_of(element);
                let Some(found) = candidates.iter().find(|c| c.namespace == ns) else { return Ok(Checked::NoSchema(ns.to_owned())) };
                found.schema.as_ref().ok_or_else(|| {
                    let msg = format!("[ISO/IEC 9075-14:2006 draft §4.2.6] {} is permanently registered, and turborel holds no components of it to validate by", found.name);
                    SqlError::new(Code::Unsupported, msg)
                })?
            }
        };
        Ok(match validate(schema, item, Mode::Strict)? {
            Ok(node) => Checked::Valid(node),
            Err(failure) => Checked::Invalid(failure),
        })
    }
}

impl According {
    /// [§6.20 GR6a iii 2 B, GR6b ii] [§8.5 GR7a iii 3 B II 1, GR7b ii 2 A]
    /// Whether `element`'s name ENAME is not the QName ENSURI and EN, of
    /// an element name specification, or not of the namespace ENSURI, of
    /// a namespace specification alone.
    fn mismatch(&self, element: &Node) -> Option<Mismatch> {
        let ns = namespace_of(element);
        if let Some(en) = &self.element {
            let local = element.tree().name(element.id()).map(|n| &*n.local);
            let named = local == Some(en.as_str()) && self.namespace.as_deref() == Some(ns);
            return (!named).then_some(Mismatch::QName);
        }
        let other = self.namespace.as_deref().is_some_and(|ensuri| ensuri != ns);
        other.then_some(Mismatch::Namespace)
    }
}

/// [§6.20] XMLVALIDATE: the value, each element of it validated, of the
/// declared type SR3 gives.
#[derive(Debug)]
pub struct XmlValidate {
    /// DOCUMENT, CONTENT or SEQUENCE, DCS.
    pub dcs: Dcs,
    /// How each element finds its registered XML Schema.
    pub by: ValidBy,
    /// The declared type \[SR3\].
    pub declared: DataType,
}

impl ScalarFunction for XmlValidate {
    fn name(&self) -> &'static str {
        "XMLVALIDATE"
    }

    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(self.declared.clone())
    }

    /// [§6.20 GR2] The null value of the null value; else, of the shape
    /// DCS asks \[GR3\], each item validated or copied \[GR4 to GR8\].
    fn call(&self, args: &[Value]) -> Result<Value> {
        let Some(Value::Xml(x)) = args.first() else { return Ok(Value::Null) };
        let v = x.sequence()?;
        shaped(self.dcs, &v)?;
        let items = v.items().iter().map(|item| self.validated(item)).collect::<Result<Vec<_>>>()?;
        Ok(Value::Xml(XmlValue::of(Sequence::new(items))))
    }
}

impl XmlValidate {
    /// [§6.20 GR6] Item `item` validated: Rj.
    fn validated(&self, item: &Item) -> Result<Item> {
        let Item::Node(node) = item else { return Err(sequence_not_validated()) };
        match node.kind() {
            Some(Kind::Document) => self.document(node).map(Item::Node),
            Some(Kind::Element) => required(self.by.check(node, node)?).map(Item::Node),
            Some(Kind::Comment | Kind::ProcessingInstruction) => standing_alone(node).map(Item::Node),
            Some(Kind::Attribute | Kind::Text) | None => Err(sequence_not_validated()),
        }
    }

    /// [§6.20 GR6a] Document node `d` validated: of DOCUMENT, `d` itself by
    /// its element \[GR6a iii 2 C I, E I\]; else a new document node of each
    /// element child validated alone and of copies of its comments and
    /// processing instructions \[GR6a iii 2 C II, E II, 3, iv\].
    fn document(&self, d: &Node) -> Result<Node> {
        let tree = d.tree();
        if self.dcs == Dcs::Document {
            let element = tree.children(d.id()).find(|&c| tree.kind(c) == Some(Kind::Element)).ok_or_else(document_not_validated)?;
            return required(self.by.check(&d.at(element), d)?);
        }
        let mut builder = Builder::document();
        for child in tree.children(d.id()) {
            let c = d.at(child);
            match c.kind() {
                Some(Kind::Element) => {
                    let valid = required(self.by.check(&c, &c)?)?;
                    builder.copy(valid.tree(), valid.id())?;
                }
                Some(Kind::Comment | Kind::ProcessingInstruction) => builder.copy(tree, child)?,
                Some(Kind::Document | Kind::Attribute | Kind::Text) | None => return Err(document_not_validated()),
            }
        }
        Ok(Node::root(builder.finish()?))
    }
}

/// [§6.20 GR3] That `v` is of the shape `dcs` asks: of DOCUMENT, a
/// document \[GR3a\]; of CONTENT, one document node \[GR3b\]; of SEQUENCE,
/// no atomic value, attribute or text node \[GR3c\].
fn shaped(dcs: Dcs, v: &Sequence) -> Result<()> {
    match dcs {
        Dcs::Document => {
            if v.is_document() {
                return Ok(());
            }
            let msg =
                "[ISO/IEC 9075-14:2006 draft §6.20 GR3a] XMLVALIDATE DOCUMENT of a value that is no XML document: invalid XML document";
            Err(SqlError::new(Code::Data, msg.to_owned()).with_state(SqlState::INVALID_XML_DOCUMENT))
        }
        Dcs::Content => {
            if v.is_content() {
                return Ok(());
            }
            let msg =
                "[ISO/IEC 9075-14:2006 draft §6.20 GR3b] XMLVALIDATE CONTENT of a value that is not one document node: invalid XML content";
            Err(SqlError::new(Code::Data, msg.to_owned()).with_state(SqlState::INVALID_XML_CONTENT))
        }
        Dcs::Sequence => {
            if v.items().iter().all(validatable) {
                Ok(())
            } else {
                Err(sequence_not_validated())
            }
        }
    }
}

/// [§6.20 GR3c, §8.5 GR5b] Whether `item` is a document, element, comment
/// or processing instruction node: no atomic value, attribute or text
/// node.
fn validatable(item: &Item) -> bool {
    let Item::Node(node) = item else { return false };
    matches!(node.kind(), Some(Kind::Document | Kind::Element | Kind::Comment | Kind::ProcessingInstruction))
}

/// [§6.20 GR3c] The exception of a sequence holding an item that is not
/// validated.
fn sequence_not_validated() -> SqlError {
    SqlError::new(
        Code::Data,
        "[ISO/IEC 9075-14:2006 draft §6.20 GR3c] XMLVALIDATE of an atomic value, attribute or text node: XQuery sequence cannot be validated"
            .to_owned(),
    )
    .with_state(SqlState::XQUERY_SEQUENCE_CANNOT_BE_VALIDATED)
}

/// [§6.20 GR6a iii 1] The exception of a document node with a child that
/// is no element, comment or processing instruction.
fn document_not_validated() -> SqlError {
    SqlError::new(
        Code::Data,
        "[ISO/IEC 9075-14:2006 draft §6.20 GR6a iii 1] a document node validated has a child that is no element, comment or processing instruction: XQuery document node cannot be validated".to_owned(),
    )
    .with_state(SqlState::XQUERY_DOCUMENT_NODE_CANNOT_BE_VALIDATED)
}

/// [§6.20 GR6a iii 2, GR6b] The node an element's validation made, or
/// the exception it raises: no XML schema found \[A II 2, i 2\], no XML
/// element with the specified QName or namespace \[B, ii\], validation
/// failure \[D, iv\].
fn required(checked: Checked) -> Result<Node> {
    match checked {
        Checked::Valid(node) => Ok(node),
        Checked::Invalid(failure) => Err(failure.validation_failure()),
        Checked::NoSchema(ns) => Err(SqlError::new(
            Code::Data,
            format!("[ISO/IEC 9075-14:2006 draft §6.20 GR6b i 2] no registered XML Schema of namespace '{ns}' may be used: no XML schema found"),
        )
        .with_state(SqlState::NO_XML_SCHEMA_FOUND)),
        Checked::Other(Mismatch::QName) => Err(SqlError::new(
            Code::Data,
            "[ISO/IEC 9075-14:2006 draft §6.20 GR6b ii 1] the element validated is not of the name the element clause gives: no XML element with the specified QName".to_owned(),
        )
        .with_state(SqlState::NO_XML_ELEMENT_WITH_THE_SPECIFIED_QNAME)),
        Checked::Other(Mismatch::Namespace) => Err(SqlError::new(
            Code::Data,
            "[ISO/IEC 9075-14:2006 draft §6.20 GR6b ii 2] the element validated is not of the namespace the element clause gives: no XML element with the specified namespace".to_owned(),
        )
        .with_state(SqlState::NO_XML_ELEMENT_WITH_THE_SPECIFIED_NAMESPACE)),
    }
}

/// [§10.18] A copy of comment or processing instruction `node`, standing
/// alone [§6.20 GR6c].
fn standing_alone(node: &Node) -> Result<Node> {
    let mut builder = Builder::parentless();
    builder.copy(node.tree(), node.id())?;
    Ok(Node::root(builder.finish()?))
}

/// [§8.5] IS VALID: whether an XML value is valid according to a
/// registered XML Schema, Unknown if an element's namespace has none.
#[derive(Debug)]
pub struct XmlValid {
    /// DOCUMENT, CONTENT or SEQUENCE, DCS.
    pub dcs: Dcs,
    /// How each element finds its registered XML Schema.
    pub by: ValidBy,
}

impl ScalarFunction for XmlValid {
    fn name(&self) -> &'static str {
        "IS VALID"
    }

    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(DataType::Boolean)
    }

    /// [§8.5 GR2] Unknown of the null value; False of a value not of the
    /// shape DCS asks \[GR3 to GR5\]; else True AND the truth of each item
    /// \[GR6 to GR8\]: False as soon as one is.
    fn call(&self, args: &[Value]) -> Result<Value> {
        let Some(Value::Xml(x)) = args.first() else { return Ok(Value::Null) };
        let v = x.sequence()?;
        let fits = match self.dcs {
            Dcs::Document => v.is_document(),
            Dcs::Content => v.is_content(),
            Dcs::Sequence => v.items().iter().all(validatable),
        };
        if !fits {
            return Ok(Value::Bool(false));
        }
        let mut unknown = false;
        for item in v.items() {
            match self.truth(item)? {
                Some(false) => return Ok(Value::Bool(false)),
                Some(true) => {}
                None => unknown = true,
            }
        }
        Ok(if unknown { Value::Null } else { Value::Bool(true) })
    }
}

impl XmlValid {
    /// [§8.5 GR7] The truth of item `item`: of a document node, of each
    /// element child, validated as a whole of DOCUMENT, alone otherwise
    /// \[GR7a\]; of an element, its own \[GR7b\]; of a comment or processing
    /// instruction, True \[GR7c\]. None is Unknown.
    fn truth(&self, item: &Item) -> Result<Option<bool>> {
        let Item::Node(node) = item else { return Ok(Some(false)) };
        match node.kind() {
            Some(Kind::Element) => self.of_element(node, node),
            Some(Kind::Document) => self.of_document(node),
            Some(Kind::Comment | Kind::ProcessingInstruction) => Ok(Some(true)),
            Some(Kind::Attribute | Kind::Text) | None => Ok(Some(false)),
        }
    }

    /// [§8.5 GR7a] The truth of document node `d`, ITVM: False of a child
    /// no element, comment or processing instruction \[iii 1\]; else True
    /// AND that of each element child \[iii 3\].
    fn of_document(&self, d: &Node) -> Result<Option<bool>> {
        let tree = d.tree();
        let mut unknown = false;
        for child in tree.children(d.id()) {
            let truth = match tree.kind(child) {
                Some(Kind::Element) => {
                    let item = if self.dcs == Dcs::Document { d.clone() } else { d.at(child) };
                    self.of_element(&d.at(child), &item)?
                }
                Some(Kind::Comment | Kind::ProcessingInstruction) => Some(true),
                Some(Kind::Document | Kind::Attribute | Kind::Text) | None => Some(false),
            };
            match truth {
                Some(false) => return Ok(Some(false)),
                Some(true) => {}
                None => unknown = true,
            }
        }
        Ok(if unknown { None } else { Some(true) })
    }

    /// [§8.5 GR7a iii 3, GR7b] The truth of element `element`, validated as
    /// `item`: Unknown if no registered XML Schema is found \[B I, ii 1\];
    /// False if it is not of the element clause's name or namespace, or
    /// is not valid; else True.
    fn of_element(&self, element: &Node, item: &Node) -> Result<Option<bool>> {
        Ok(match self.by.check(element, item)? {
            Checked::Valid(_) => Some(true),
            Checked::Invalid(_) | Checked::Other(_) => Some(false),
            Checked::NoSchema(_) => None,
        })
    }
}

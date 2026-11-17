//! The functions of XMLELEMENT [ISO/IEC 9075-14:2006 draft §6.14] and
//! XMLFOREST [§6.15], as the binder calls them (`binder/xml/element.rs`),
//! each RETURNING SEQUENCE: an element constructed as §10.13 says.

use std::sync::Arc;

use super::super::{BinaryEncoding, ScalarFunction, xml_lexical};
use crate::ast::ContentOption;
use crate::error::Result;
use crate::value::xml::{XmlType, XmlValue};
use crate::value::{DataType, Value};
use crate::xml::{Item, Kind, QName, Sequence, Specified, Syntax, Whitespace, XSI_NAMESPACE, XSI_PREFIX, concatenate, element, parse};

/// [§10.13 GR4c iii 1 B] The local name and value of `xsi:nil`.
const NIL: (&str, &str) = ("nil", "true");

/// [§10.13 GR4c ii 2] Whether `contents` hold an element or a text node,
/// or an atomic value, which is made one [XQuery §3.7.1.3]; a document
/// node's children counting as its own.
fn has_content(contents: &[Arc<Sequence>]) -> bool {
    let is_content = |k: Option<Kind>| matches!(k, Some(Kind::Element | Kind::Text));
    contents.iter().flat_map(|s| s.items()).any(|item| match item {
        Item::Atomic(_) => true,
        Item::Node(node) => {
            let tree = node.tree();
            is_content(node.kind()) || (node.kind() == Some(Kind::Document) && tree.children(node.id()).any(|c| is_content(tree.kind(c))))
        }
    })
}

/// [§10.13 GR4c i] Value `v`, not null, as the element's content: an XML
/// value as it is \[1 A\]; another mapped to XML [§9.8], `&`, `<`, `>`
/// and a carriage return as references, and parsed as CONTENT,
/// whitespace preserved \[1 B\].
fn content_of(v: &Value, encoding: BinaryEncoding) -> Result<Arc<Sequence>> {
    if let Value::Xml(x) = v {
        return x.sequence();
    }
    let text = xml_lexical(v, encoding, true)?;
    Ok(Arc::new(Sequence::document(parse(&text, Syntax::Content, Whitespace::Preserve)?)))
}

/// [§10.13] The element of `name` and `attributes` and the content
/// `values`, by `option`, binary strings written by `encoding`: none, the
/// null value, if every value is null and the option NULL ON NULL; the
/// empty sequence of ABSENT ON NULL \[GR4a, GR4b\]; else the element, its
/// content the values not null, `xsi:nil="true"` after its attributes if
/// every value is null and the option NIL ON NULL, or none is an element
/// or text and the option NIL ON NO CONTENT \[GR4c\].
fn construct(
    name: &QName,
    mut attributes: Vec<Specified>,
    values: &[&Value],
    option: Option<ContentOption>,
    encoding: BinaryEncoding,
) -> Result<Option<Sequence>> {
    let all_null = !values.is_empty() && values.iter().all(|v| v.is_null());
    match (all_null, option) {
        (true, Some(ContentOption::NullOnNull)) => return Ok(None),
        (true, Some(ContentOption::AbsentOnNull)) => return Ok(Some(Sequence::default())),
        (true | false, _) => {}
    }
    let contents = values.iter().filter(|v| !v.is_null()).map(|v| content_of(v, encoding)).collect::<Result<Vec<_>>>()?;
    let nilled = match option {
        Some(ContentOption::NilOnNull) => all_null,
        Some(ContentOption::NilOnNoContent) => !has_content(&contents),
        Some(ContentOption::NullOnNull | ContentOption::EmptyOnNull | ContentOption::AbsentOnNull) | None => false,
    };
    if nilled {
        let (local, value) = NIL;
        let nil = QName { prefix: Some(XSI_PREFIX.into()), uri: Some(XSI_NAMESPACE.into()), local: local.into() };
        attributes.push(Specified { name: nil, value: value.into(), is_id: false, is_idrefs: false });
    }
    let refs: Vec<&Sequence> = contents.iter().map(|s| &**s).collect();
    element(name.clone(), attributes, &refs).map(Some)
}

/// [§6.14] XMLELEMENT, RETURNING SEQUENCE: of its attributes' values,
/// then its content's.
#[derive(Debug)]
pub struct XmlElement {
    /// The element's name, its namespace URI found [SR5, SR6].
    pub name: QName,
    /// The attributes' names, in order \[SR3\].
    pub attributes: Vec<QName>,
    /// The `<XML content option>`, of content alone \[SR8\].
    pub option: Option<ContentOption>,
    /// How binary strings are written [GR1, GR2].
    pub encoding: BinaryEncoding,
}

impl ScalarFunction for XmlElement {
    fn name(&self) -> &'static str {
        "XMLELEMENT"
    }

    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(DataType::Xml(XmlType::Sequence))
    }

    /// [§6.14 GR3 to GR6] Each attribute whose value is not null, that
    /// value mapped to XML [§9.8], characters as they are; the element
    /// §10.13 makes of them and the content.
    fn call(&self, args: &[Value]) -> Result<Value> {
        let (given, content) = args.split_at(self.attributes.len().min(args.len()));
        let mut attributes = Vec::with_capacity(given.len());
        for (name, v) in self.attributes.iter().zip(given) {
            if !v.is_null() {
                let value = xml_lexical(v, self.encoding, false)?;
                attributes.push(Specified { name: name.clone(), value, is_id: false, is_idrefs: false });
            }
        }
        let values: Vec<&Value> = content.iter().collect();
        Ok(construct(&self.name, attributes, &values, self.option, self.encoding)?.map_or(Value::Null, |s| Value::Xml(XmlValue::of(s))))
    }
}

/// [§6.15] XMLFOREST, RETURNING SEQUENCE: of its elements' values.
#[derive(Debug)]
pub struct XmlForest {
    /// The elements' names, their namespace URIs found \[SR2\].
    pub names: Vec<QName>,
    /// The `<XML content option>` \[SR3\].
    pub option: ContentOption,
    /// How binary strings are written [GR1, GR2].
    pub encoding: BinaryEncoding,
}

impl ScalarFunction for XmlForest {
    fn name(&self) -> &'static str {
        "XMLFOREST"
    }

    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(DataType::Xml(XmlType::Sequence))
    }

    /// [§6.15 GR3 to GR7] An element of each value, §10.13's of no
    /// attribute, each concatenated with the last [§10.14].
    fn call(&self, args: &[Value]) -> Result<Value> {
        let mut made = Vec::with_capacity(args.len());
        for (name, v) in self.names.iter().zip(args) {
            made.push(construct(name, Vec::new(), &[v], Some(self.option), self.encoding)?);
        }
        Ok(concatenate(made.iter().map(Option::as_ref)).map_or(Value::Null, |s| Value::Xml(XmlValue::of(s))))
    }
}

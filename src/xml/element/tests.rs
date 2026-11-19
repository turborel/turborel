//! Tests of the computed element constructor.

use super::*;
use crate::error::SqlState;
use crate::xml::sequence::{Atomic, AtomicType};
use crate::xml::{Parameters, Syntax, Version, Whitespace, parse, serialize, text};

/// The serialization of `value`, in XML 1.1.
fn written(value: &Sequence) -> String {
    serialize(value, &Parameters { version: Version::V11, declaration: false, encoding: "UTF-8", undeclare: true }).unwrap()
}

fn content(text: &str) -> Sequence {
    Sequence::document(parse(text, Syntax::Content, Whitespace::Preserve).unwrap())
}

fn atomic(lexical: &str) -> Item {
    Item::Atomic(Atomic::new(AtomicType::String, lexical))
}

fn name(prefix: Option<&str>, uri: Option<&str>, local: &str) -> QName {
    QName { prefix: prefix.map(Into::into), uri: uri.map(Into::into), local: local.into() }
}

fn attribute(name: QName, value: &str) -> Specified {
    Specified { name, value: value.into(), is_id: false, is_idrefs: false }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.13 GR4c iv to ix] [XQuery
/// §3.7.3.1, §3.7.1.3] an element of the name, standing alone: its
/// attributes those given; its children copies of the content's nodes, a
/// document node's children in its place; adjacent atomic values one
/// text node a space apart, even of zero-length strings; adjacent text
/// one; no text node empty.
#[test]
fn an_element_of_its_content() {
    let a = content("<b x='1'>t</b><!--c--><?p d?>");
    let atoms = Sequence::new(vec![atomic("1"), atomic(""), atomic("2")]);
    let e = element(QName::local("a"), vec![attribute(QName::local("y"), "v")], &[&a, &atoms, &text("u").unwrap(), &text("").unwrap()])
        .unwrap();
    assert_eq!(written(&e), "<a y=\"v\"><b x=\"1\">t</b><!--c--><?p d?>1  2u</a>");
    let [Item::Node(node)] = e.items() else { panic!("one node") };
    assert!(node.tree().parent(node.id()).is_none(), "standing alone");
    assert_eq!(written(&element(QName::local("e"), Vec::new(), &[]).unwrap()), "<e/>");
    let lone = Sequence::new(vec![atomic("")]);
    assert_eq!(written(&element(QName::local("e"), Vec::new(), &[&lone]).unwrap()), "<e/>");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.13 GR4c viii] [XQuery
/// §3.7.1.3] an attribute node of the content is the element's if no
/// other node is before it, else err:XQTY0024; two attributes of one
/// expanded name err:XQDY0025, each SQLSTATE 10000.
#[test]
fn attributes_of_the_content() {
    let mut b = Builder::parentless();
    b.attribute(&attribute(QName::local("z"), "w")).unwrap();
    let lone = Sequence::new(vec![Item::Node(Node::root(b.finish().unwrap()))]);
    let e = element(QName::local("a"), vec![attribute(QName::local("y"), "v")], &[&lone, &text("t").unwrap()]).unwrap();
    assert_eq!(written(&e), "<a y=\"v\" z=\"w\">t</a>");
    let after = element(QName::local("a"), Vec::new(), &[&text("t").unwrap(), &lone]).unwrap_err();
    assert_eq!(after.sqlstate(), SqlState::XQUERY_ERROR);
    assert!(after.to_string().contains("XQTY0024"), "{after}");
    let empty = Sequence::new(vec![atomic("")]);
    assert!(element(QName::local("a"), Vec::new(), &[&empty, &lone]).is_ok(), "an empty text node is deleted first");
    let twice = element(QName::local("a"), vec![attribute(QName::local("z"), "v")], &[&lone]).unwrap_err();
    assert!(twice.to_string().contains("XQDY0025"), "{twice}");
    let other = vec![attribute(name(Some("p"), Some("u"), "z"), "1"), attribute(QName::local("z"), "2")];
    assert!(element(QName::local("a"), other, &[]).is_ok(), "names alike in other namespaces");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.13 GR4c viii] [XQuery §3.7.4]
/// an element's bindings are those its names
/// need: its own prefix, or the default namespace; each attribute's
/// prefix, one made for an attribute in a namespace with none; a prefix
/// of a binding to another URI changed to one made; none for `xml` or a
/// name in no namespace.
#[test]
fn bindings_its_names_need() {
    let e = element(name(Some("p"), Some("u"), "a"), vec![attribute(name(Some("p"), Some("w"), "x"), "1")], &[]).unwrap();
    assert_eq!(written(&e), "<p:a xmlns:p=\"u\" xmlns:p1=\"w\" p1:x=\"1\"/>");
    let d = element(name(None, Some("u"), "a"), vec![attribute(name(None, Some("u"), "x"), "1")], &[]).unwrap();
    assert_eq!(written(&d), "<a xmlns=\"u\" xmlns:ns1=\"u\" ns1:x=\"1\"/>");
    let x =
        element(QName::local("a"), vec![attribute(name(Some("xml"), Some(crate::xml::tree::XML_NAMESPACE), "lang"), "en")], &[]).unwrap();
    assert_eq!(written(&x), "<a xml:lang=\"en\"/>");
    let none = element(name(Some("q"), None, "a"), Vec::new(), &[]).unwrap();
    assert_eq!(written(&none), "<a/>", "a name in no namespace has no prefix");
    let shared = element(name(Some("p"), Some("u"), "a"), vec![attribute(name(Some("p"), Some("u"), "x"), "1")], &[]).unwrap();
    assert_eq!(written(&shared), "<p:a xmlns:p=\"u\" p:x=\"1\"/>");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.13 GR4c viii] [XQuery
/// §3.7.1.3] [XQueryDM §6.2.1] an element copied into
/// another keeps its names' namespaces, its declarations written where
/// its parent's do not bind them; one in no namespace under a default
/// namespace undeclares it.
#[test]
fn copies_keep_their_namespaces() {
    let inner = element(name(Some("p"), Some("u"), "b"), Vec::new(), &[]).unwrap();
    let outer = element(name(Some("p"), Some("u"), "a"), Vec::new(), &[&inner]).unwrap();
    assert_eq!(written(&outer), "<p:a xmlns:p=\"u\"><p:b/></p:a>");
    let plain = element(QName::local("c"), Vec::new(), &[]).unwrap();
    let defaulted = element(name(None, Some("u"), "a"), Vec::new(), &[&plain]).unwrap();
    assert_eq!(written(&defaulted), "<a xmlns=\"u\"><c xmlns=\"\"/></a>");
}

//! Tests of trees and their building.

use super::*;
use crate::xml::{MAX_NESTING, MAX_TEXT};

fn element(name: &str, attributes: &[(&str, &str)]) -> ElementStart {
    ElementStart {
        name: QName::local(name),
        bindings: Vec::new(),
        attributes: attributes
            .iter()
            .map(|(n, v)| Specified { name: QName::local(n), value: (*v).to_owned(), is_id: false, is_idrefs: false })
            .collect(),
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.16 GR9] [XQueryDM §2.4,
/// §6.7.3] nodes are in document order, an element's attributes after it
/// and before its children; adjacent text is one text node and empty
/// text none.
#[test]
fn a_tree_is_in_document_order() {
    let mut b = Builder::document();
    b.start_element(element("a", &[("x", "1"), ("y", "2")])).unwrap();
    b.text("t").unwrap();
    b.text("").unwrap();
    b.text("u").unwrap();
    b.comment("c").unwrap();
    b.text("v").unwrap();
    b.end_element().unwrap();
    b.processing_instruction("p", "d").unwrap();
    let tree = b.finish().unwrap();
    let a = tree.children(NodeId::ROOT).next().unwrap();
    let attributes: Vec<NodeId> = tree.attributes(a).collect();
    let children: Vec<NodeId> = tree.children(a).collect();
    assert_eq!(attributes.len(), 2);
    assert!(attributes.iter().all(|&x| x > a && x < children[0]));
    assert_eq!(tree.content(attributes[1]), "2");
    assert_eq!(children.iter().map(|&c| tree.kind(c).unwrap()).collect::<Vec<_>>(), [Kind::Text, Kind::Comment, Kind::Text]);
    assert_eq!(tree.content(children[0]), "tu");
    assert_eq!(tree.string_value(a), "tuv");
    assert_eq!(tree.descendants(a).count(), 3);
    assert_eq!(tree.parent(children[1]), Some(a));
    assert_eq!(tree.parent(attributes[0]), Some(a));
    assert_eq!(tree.parent(NodeId::ROOT), None);
    assert_eq!(tree.name(tree.children(NodeId::ROOT).nth(1).unwrap()).map(|n| &*n.local), Some("p"));
    assert_eq!(tree.len(), 8);
    assert!(!tree.is_empty());
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.16 GR9] [XQueryDM §5.7,
/// §6.4.1] the bindings in scope of an element are `xml`'s, then its
/// ancestors' and its own, the nearer overriding, an undeclared one gone.
#[test]
fn bindings_in_scope() {
    let mut b = Builder::document();
    let mut outer = element("a", &[]);
    outer.bindings = vec![Binding { prefix: Some("p".into()), uri: "u1".into() }, Binding { prefix: None, uri: "d".into() }];
    b.start_element(outer).unwrap();
    let mut inner = element("b", &[]);
    inner.bindings = vec![Binding { prefix: Some("p".into()), uri: "u2".into() }, Binding { prefix: None, uri: "".into() }];
    b.start_element(inner).unwrap();
    b.end_element().unwrap();
    b.end_element().unwrap();
    let tree = b.finish().unwrap();
    let a = tree.children(NodeId::ROOT).next().unwrap();
    let inner_id = tree.children(a).next().unwrap();
    assert_eq!(tree.in_scope(a), [(Some("xml"), XML_NAMESPACE), (Some("p"), "u1"), (None, "d")]);
    assert_eq!(tree.in_scope(inner_id), [(Some("xml"), XML_NAMESPACE), (Some("p"), "u2")]);
    assert_eq!(tree.declared(inner_id).len(), 2);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.16 GR9] [XQueryDM §3.3.3]
/// two names are the same if their namespace URIs and local names are,
/// whatever their prefixes; names are kept once in a tree.
#[test]
fn names_compare_by_uri_and_local_name() {
    let a = QName { prefix: Some("p".into()), uri: Some("u".into()), local: "x".into() };
    let b = QName { prefix: Some("q".into()), uri: Some("u".into()), local: "x".into() };
    assert!(a.same(&b));
    assert!(!a.same(&QName::local("x")));
    let mut builder = Builder::document();
    builder.start_element(element("x", &[("x", "1")])).unwrap();
    builder.end_element().unwrap();
    assert_eq!(builder.finish().unwrap().names.len(), 1);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.16 GR9] [DESIGN] RULES.md
/// D106: a builder refuses to nest past `MAX_NESTING`, to hold more than
/// `MAX_TEXT` octets, to close what it did not open, or to finish with an
/// element open.
#[test]
fn a_builder_keeps_its_bounds() {
    let mut b = Builder::document();
    for _ in 0..MAX_NESTING {
        b.start_element(element("a", &[])).unwrap();
    }
    assert_eq!(b.depth(), MAX_NESTING);
    assert_eq!(b.start_element(element("a", &[])).unwrap_err().sqlcode(), -900_i32);
    assert_eq!(b.finish().unwrap_err().sqlcode(), -900_i32);
    let mut text = Builder::document();
    text.text(&"x".repeat(MAX_TEXT)).unwrap();
    assert_eq!(text.text("y").unwrap_err().sqlcode(), -900_i32);
    assert_eq!(Builder::document().end_element().unwrap_err().sqlcode(), -900_i32);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.16 GR9] [XQueryDM §5.5,
/// §5.6] an attribute keeps whether it is an ID or an IDREF; a node a
/// tree does not have has no kind, name, content or flags.
#[test]
fn attribute_flags_and_absent_nodes() {
    let mut b = Builder::document();
    let mut start = element("a", &[("i", "v")]);
    start.attributes[0].is_id = true;
    start.attributes.push(Specified { name: QName::local("r"), value: "v w".into(), is_id: false, is_idrefs: true });
    b.start_element(start).unwrap();
    b.end_element().unwrap();
    let tree = b.finish().unwrap();
    let a = tree.children(NodeId::ROOT).next().unwrap();
    let flags: Vec<(bool, bool)> = tree.attributes(a).map(|x| (tree.is_id(x), tree.is_idrefs(x))).collect();
    assert_eq!(flags, [(true, false), (false, true)]);
    let absent = NodeId(99);
    assert_eq!((tree.kind(absent), tree.name(absent), tree.content(absent)), (None, None, ""));
    assert!(!tree.is_id(absent) && !tree.is_idrefs(absent));
    assert_eq!(tree.string_value(absent), "");
    assert_eq!(tree.string_value(tree.attributes(a).next().unwrap()), "v");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.18] [XQueryDM §6.2.1,
/// §6.3.1] [DESIGN] RULES.md D106: a tree's root may be a node with no
/// parent, an element or an attribute alone among others; it has one
/// root, an attribute alone is its only node, and a tree has a node.
#[test]
fn trees_rooted_in_parentless_nodes() {
    let mut element = Builder::parentless();
    element.start_element(element_start("a")).unwrap();
    element.text("t").unwrap();
    element.end_element().unwrap();
    assert_eq!(element.text("u").unwrap_err().sqlcode(), -900_i32);
    let tree = element.finish().unwrap();
    assert_eq!(
        (tree.kind(NodeId::ROOT), tree.parent(NodeId::ROOT), tree.string_value(NodeId::ROOT)),
        (Some(Kind::Element), None, "t".to_owned())
    );
    let mut attribute = Builder::parentless();
    let alone = Specified { name: QName::local("x"), value: "v".into(), is_id: true, is_idrefs: false };
    attribute.attribute(&alone).unwrap();
    assert_eq!(attribute.attribute(&alone).unwrap_err().sqlcode(), -900_i32);
    let lone = attribute.finish().unwrap();
    assert!(lone.is_id(NodeId::ROOT) && lone.kind(NodeId::ROOT) == Some(Kind::Attribute));
    assert_eq!(Builder::document().attribute(&alone).unwrap_err().sqlcode(), -900_i32);
    assert_eq!(Builder::parentless().finish().unwrap_err().sqlcode(), -900_i32);
    assert_eq!(Builder::parentless().end_element().unwrap_err().sqlcode(), -900_i32);
}

fn element_start(name: &str) -> ElementStart {
    element(name, &[])
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §4.2.3] [XQueryDM §3.3.1.2,
/// §5.15, §6.2.4] the typed value of an annotated element: of a list type
/// an atomic value of its item type for each item; of empty content, and
/// of a nilled element, none; of mixed content its string value, untyped.
#[test]
fn typed_values_by_annotation() {
    let mut plain = Builder::parentless();
    plain.start_element(element_start("e")).unwrap();
    plain.text(" 1 20 ").unwrap();
    plain.end_element().unwrap();
    let annotated = |annotation: &Annotation, nilled: bool| {
        let mut b = Builder::parentless();
        b.start_element(element_start("e")).unwrap();
        b.text(" 1 20 ").unwrap();
        b.annotate(NodeId::ROOT, annotation, (nilled, false, false)).unwrap();
        b.end_element().unwrap();
        b.finish().unwrap().typed_value(NodeId::ROOT).unwrap()
    };
    let named = |local: &str, typing| Annotation { name: QName::local(local), typing };
    let list = annotated(&named("sizes", Typing::List(crate::xml::AtomicType::Integer.into())), false);
    assert_eq!(
        list.iter().map(|a| (a.kind, &*a.lexical)).collect::<Vec<_>>(),
        [(crate::xml::AtomicType::Integer, "1"), (crate::xml::AtomicType::Integer, "20")]
    );
    assert_eq!(annotated(&named("mark", Typing::Empty), false), []);
    assert_eq!(annotated(&named("sizes", Typing::List(crate::xml::AtomicType::Integer.into())), true), []);
    let mixed = annotated(&Annotation::any_type(), false);
    assert_eq!(mixed.iter().map(|a| (a.kind, &*a.lexical)).collect::<Vec<_>>(), [(crate::xml::AtomicType::UntypedAtomic, " 1 20 ")]);
    assert!(plain.finish().unwrap().annotation(NodeId::ROOT).is_none());
}

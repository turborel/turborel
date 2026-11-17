//! Tests of the computed document constructor.

use std::sync::Arc;

use super::*;
use crate::xml::sequence::{Atomic, AtomicType, Node};
use crate::xml::tests::shape;
use crate::xml::tree::NodeId;
use crate::xml::{Parameters, Syntax, Version, Whitespace, parse, serialize};

fn tree(text: &str) -> Arc<crate::xml::Tree> {
    Arc::new(parse(text, Syntax::Content, Whitespace::Preserve).unwrap())
}

fn atomic(lexical: &str) -> Item {
    Item::Atomic(Atomic { kind: AtomicType::String, lexical: lexical.into() })
}

/// The one tree of `value`, a document node.
fn made(value: &Sequence) -> String {
    let [Item::Node(node)] = value.items() else { panic!("one node") };
    assert_eq!(node.kind(), Some(Kind::Document));
    shape(node.tree())
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.13 GR8] [XQuery §3.7.3.3,
/// §3.7.1.3] `document { $EXPR }`: a new document node whose children
/// are copies of the nodes of $EXPR, a document node's children in its
/// place; adjacent atomic values one text node, a space apart, merged
/// with text next to them; no text node empty.
#[test]
fn a_document_node_of_the_content() {
    let doc = tree("<!--c--><a x='1'>t<b/></a>u");
    let a = doc.children(NodeId::ROOT).nth(1).unwrap();
    let b = doc.children(a).nth(1).unwrap();
    let items = vec![
        atomic("1"),
        atomic(""),
        atomic("2"),
        Item::Node(Node::new(Arc::clone(&doc), NodeId::ROOT)),
        Item::Node(Node::new(Arc::clone(&doc), b)),
        atomic("3"),
    ];
    let value = document(&Sequence::new(items)).unwrap();
    assert_eq!(made(&value), "1  2<!--c--><a x=\"1\">t<b></></>u<b></>3");
    let [Item::Node(node)] = value.items() else { panic!("one node") };
    assert!(!node.is(&Node::new(Arc::clone(&doc), NodeId::ROOT)), "a new node");
    assert_eq!(made(&document(&Sequence::default()).unwrap()), "", "of the empty sequence");
    assert_eq!(made(&document(&Sequence::new(vec![atomic("")])).unwrap()), "");
    let text = Sequence::new(vec![Item::Node(Node::new(Arc::clone(&doc), doc.children(NodeId::ROOT).nth(2).unwrap())), atomic("v")]);
    let merged = document(&text).unwrap();
    let [Item::Node(only)] = merged.items() else { panic!("one node") };
    assert_eq!(only.tree().children(NodeId::ROOT).count(), 1, "adjacent text is one text node");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.13 GR5, GR8] [XQuery §3.7.1.3]
/// [XQueryDM §6.2.1] an element copied from below another declares every
/// binding in its scope, and keeps its attributes' ID and IDREFS
/// properties, construction mode being preserve.
#[test]
fn copies_keep_namespaces_and_ids() {
    let doc = tree("<!DOCTYPE r [<!ATTLIST e i ID #IMPLIED>]><r xmlns='d' xmlns:p='u' xmlns:q='w'><e i='a' p:x='1'/></r>");
    let r = doc.children(NodeId::ROOT).find(|&c| doc.kind(c) == Some(Kind::Element)).unwrap();
    let e = doc.children(r).next().unwrap();
    let value = document(&Sequence::new(vec![Item::Node(Node::new(Arc::clone(&doc), e))])).unwrap();
    let parameters = Parameters { version: Version::V10, declaration: false, encoding: "UTF-8", undeclare: false };
    assert_eq!(serialize(&value, &parameters).unwrap(), "<e xmlns=\"d\" xmlns:p=\"u\" xmlns:q=\"w\" i=\"a\" p:x=\"1\"/>");
    let [Item::Node(node)] = value.items() else { panic!("one node") };
    let copied = node.tree().children(NodeId::ROOT).next().unwrap();
    let i = node.tree().attributes(copied).next().unwrap();
    assert!(node.tree().is_id(i));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.13 GR8] [XQuery §3.7.3.3] an
/// attribute in the content is a type error, err:XPTY0004: SQLSTATE
/// 10000, XQuery error.
#[test]
fn an_attribute_is_refused() {
    let doc = tree("<a x='1'/>");
    let a = doc.children(NodeId::ROOT).next().unwrap();
    let x = doc.attributes(a).next().unwrap();
    let error = document(&Sequence::new(vec![Item::Node(Node::new(Arc::clone(&doc), x))])).unwrap_err();
    assert_eq!(error.sqlstate(), SqlState::XQUERY_ERROR, "{error}");
    assert!(error.to_string().contains("XPTY0004"), "{error}");
}

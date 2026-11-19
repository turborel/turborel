//! Tests of the accessors.

use super::super::testing::{atom, call, call_in, code, document, elements, empty, focus, int, one, seq, text, texts, typed};
use super::*;
use crate::xml::Item;

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §2.1]
/// [XQueryDM §5.11] the name of an element or attribute, its namespace URI
/// and prefix kept; a processing instruction's target in no namespace; the
/// empty sequence of a document, text or comment node, or of no node.
#[test]
fn node_names() {
    let doc = document("<p:a xmlns:p='urn:p' b='1'>t<!--c--><?go x?></p:a>");
    let a = elements(&doc, "a").remove(0);
    let name = call("node-name", &[one(&a)]).unwrap();
    let [Item::Atomic(q)] = name.items() else { panic!("one QName") };
    assert_eq!((q.kind, &*q.lexical, q.namespace.as_deref()), (AtomicType::QName, "p:a", Some("urn:p")));
    let tree = a.tree();
    let b = a.at(tree.attributes(a.id()).next().unwrap());
    assert_eq!(text(call("node-name", &[one(&b)])), "b");
    let children: Vec<_> = tree.children(a.id()).map(|id| a.at(id)).collect();
    assert_eq!(text(call("node-name", &[one(&children[2])])), "go");
    for node in [&doc, &children[0], &children[1]] {
        assert!(call("node-name", &[one(node)]).unwrap().items().is_empty());
    }
    assert!(call("node-name", &[empty()]).unwrap().items().is_empty());
    assert_eq!(code(call("node-name", &[int(1)])), "XPTY0004");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §2.2]
/// [XQueryDM §6.2.3] no element is nilled, as none is validated; another
/// node, or none, has no nilled property.
#[test]
fn nilled_elements() {
    let doc = document("<a>t</a>");
    let a = elements(&doc, "a").remove(0);
    assert_eq!(typed(call("nilled", &[one(&a)])), (AtomicType::Boolean, "false".to_owned()));
    assert!(call("nilled", &[one(&doc)]).unwrap().items().is_empty());
    assert!(call("nilled", &[empty()]).unwrap().items().is_empty());
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §2.3] the
/// string of a node is its string value, of an atomic value its cast to a
/// string; of the empty sequence the zero-length string; with no argument
/// the context item's, err:FONC0001 if there is none; more than one item
/// err:XPTY0004.
#[test]
fn strings_of_items() {
    let doc = document("<a>x<b>y</b></a>");
    assert_eq!(text(call("string", &[one(&doc)])), "xy");
    assert_eq!(text(call("string", &[atom(AtomicType::Double, "1.0E7")])), "1.0E7");
    assert_eq!(text(call("string", &[atom(AtomicType::Integer, "5")])), "5");
    assert_eq!(text(call("string", &[empty()])), "");
    let at = focus().at(Item::Atomic(Atomic::new(AtomicType::Boolean, "true")), 1, 1);
    assert_eq!(text(call_in(&at, "string", &[])), "true");
    assert_eq!(code(call("string", &[])), "FONC0001");
    assert_eq!(code(call("string", &[seq(&[int(1), int(2)])])), "XPTY0004");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §2.4]
/// [XQueryDM §5.15] `fn:data`: each atomic value itself, an element's or
/// attribute's typed value untyped, a comment's a string.
#[test]
fn data_of_items() {
    let doc = document("<a b='2'>1<!--c--></a>");
    let a = elements(&doc, "a").remove(0);
    let b = a.at(a.tree().attributes(a.id()).next().unwrap());
    let comment = a.at(a.tree().children(a.id()).last().unwrap());
    let value = call("data", &[seq(&[one(&a), one(&b), one(&comment), int(3)])]).unwrap();
    assert_eq!(texts(&value), vec!["1", "2", "c", "3"]);
    let kinds: Vec<_> = value.items().iter().map(|i| if let Item::Atomic(x) = i { x.kind } else { panic!("atomic") }).collect();
    assert_eq!(kinds, [AtomicType::UntypedAtomic, AtomicType::UntypedAtomic, AtomicType::String, AtomicType::Integer]);
    assert!(call("data", &[empty()]).unwrap().items().is_empty());
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §2.5]
/// [XQueryDM §5.2] a node's base URI: what `xml:base` attributes make,
/// each resolved against the one above it; a text or attribute node its
/// parent's; none if none declares one; the context item's with no
/// argument, err:FOTY0011 if it is no node.
#[test]
fn base_uris() {
    let xml = "<a xml:base='http://e.org/x/'><b xml:base='y/z'>t</b><c xml:base='rel/'><d/></c></a>";
    let doc = document(xml);
    let b = elements(&doc, "b").remove(0);
    assert_eq!(typed(call("base-uri", &[one(&b)])), (AtomicType::AnyUri, "http://e.org/x/y/z".to_owned()));
    let t = b.at(b.tree().first_child(b.id()).unwrap());
    assert_eq!(text(call("base-uri", &[one(&t)])), "http://e.org/x/y/z");
    let d = elements(&doc, "d").remove(0);
    assert_eq!(text(call("base-uri", &[one(&d)])), "http://e.org/x/rel/");
    assert!(call("base-uri", &[one(&doc)]).unwrap().items().is_empty());
    let relative = document("<r xml:base='a/'><s xml:base='b'/></r>");
    assert_eq!(text(call("base-uri", &[one(&elements(&relative, "s").remove(0))])), "b");
    assert!(call("base-uri", &[empty()]).unwrap().items().is_empty());
    let at = focus().at(Item::Node(b), 1, 1);
    assert_eq!(text(call_in(&at, "base-uri", &[])), "http://e.org/x/y/z");
    let atomic = focus().at(Item::Atomic(Atomic::new(AtomicType::String, "x")), 1, 1);
    assert_eq!(code(call_in(&atomic, "base-uri", &[])), "FOTY0011");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §2.6] no
/// document node here has a document URI; an atomic value is
/// err:XPTY0004.
#[test]
fn document_uris() {
    assert!(call("document-uri", &[one(&document("<a/>"))]).unwrap().items().is_empty());
    assert!(call("document-uri", &[empty()]).unwrap().items().is_empty());
    assert_eq!(code(call("document-uri", &[int(1)])), "XPTY0004");
}

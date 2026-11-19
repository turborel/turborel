//! Tests of the functions and operators on nodes.

use super::super::testing::{atom, below, call, call_in, code, document, elements, empty, focus, int, one, s, seq, text, typed};
use super::*;

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §14.1 to
/// §14.3] a node's name, local name and namespace URI: an element's and
/// attribute's of their prefixes and namespaces; zero-length of a node of
/// no name and of the empty sequence; of the context node if none is
/// given, err:FONC0001 if there is none and err:FOTY0011 if it is no node.
#[test]
fn names_of_nodes() {
    let doc = document("<p:a xmlns:p='urn:p' b='1'>t<?go x?></p:a>");
    let a = elements(&doc, "a").remove(0);
    assert_eq!(text(call("name", &[one(&a)])), "p:a");
    assert_eq!(text(call("local-name", &[one(&a)])), "a");
    assert_eq!(typed(call("namespace-uri", &[one(&a)])), (AtomicType::AnyUri, "urn:p".to_owned()));
    let b = below(&doc, |n| n.kind() == Some(Kind::Attribute)).remove(0);
    assert_eq!((text(call("name", &[one(&b)])), text(call("namespace-uri", &[one(&b)]))), ("b".to_owned(), String::new()));
    let pi = below(&doc, |n| n.kind() == Some(Kind::ProcessingInstruction)).remove(0);
    assert_eq!((text(call("name", &[one(&pi)])), text(call("namespace-uri", &[one(&pi)]))), ("go".to_owned(), String::new()));
    assert_eq!(text(call("name", &[one(&doc)])), "");
    assert_eq!(text(call("local-name", &[empty()])), "");
    assert_eq!(text(call("namespace-uri", &[empty()])), "");
    let at = focus().at(Item::Node(a), 1, 1);
    assert_eq!(text(call_in(&at, "local-name", &[])), "a");
    assert_eq!(code(call("name", &[])), "FONC0001");
    let atomic = focus().at(Item::Atomic(Atomic::new(AtomicType::String, "x")), 1, 1);
    assert_eq!(code(call_in(&atomic, "namespace-uri", &[])), "FOTY0011");
    assert_eq!(code(call("name", &[int(1)])), "XPTY0004");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §14.4.1]
/// the examples of `fn:number`: a node's typed value as a double, NaN of
/// one that is no number and of the empty sequence; the context item
/// "15" 1.5E1.
#[test]
fn numbers_of_items() {
    let doc = document("<q>5.0</q><d>x</d>");
    assert_eq!(typed(call("number", &[one(&elements(&doc, "q").remove(0))])), (AtomicType::Double, "5".to_owned()));
    assert_eq!(text(call("number", &[one(&elements(&doc, "d").remove(0))])), "NaN");
    assert_eq!(text(call("number", &[empty()])), "NaN");
    assert_eq!(text(call("number", &[atom(AtomicType::Boolean, "true")])), "1");
    assert_eq!(text(call("number", &[atom(AtomicType::Date, "2000-01-01")])), "NaN");
    let at = focus().at(Item::Atomic(Atomic::new(AtomicType::String, "15")), 1, 1);
    assert_eq!(text(call_in(&at, "number", &[])), "15");
    assert_eq!(code(call("number", &[])), "FONC0001");
    assert_eq!(code(call("number", &[seq(&[int(1), int(2)])])), "XPTY0004");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §14.5.1]
/// the examples of `fn:lang`: the language of a node or its nearest
/// ancestor's `xml:lang`, caseless, a sublanguage matching; false of none.
#[test]
fn languages() {
    let doc = document("<r><para xml:lang='en'/><div xml:lang='en'><para/></div><para xml:lang='EN'/><para xml:lang='en-us'/><para/></r>");
    let paras = elements(&doc, "para");
    for para in paras.iter().take(4) {
        assert_eq!(text(call("lang", &[s("en"), one(para)])), "true");
    }
    assert_eq!(text(call("lang", &[s("fr"), one(&paras[2])])), "false");
    assert_eq!(text(call("lang", &[s("en-US"), one(&paras[3])])), "true");
    assert_eq!(text(call("lang", &[s("e"), one(&paras[3])])), "false");
    assert_eq!(text(call("lang", &[s("en"), one(&paras[4])])), "false");
    assert_eq!(text(call("lang", &[empty(), one(&paras[0])])), "false");
    let at = focus().at(Item::Node(paras[1].clone()), 1, 1);
    assert_eq!(text(call_in(&at, "lang", &[s("EN")])), "true");
    assert_eq!(code(call("lang", &[s("en")])), "FONC0001");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §14.9.1]
/// `fn:root`: the root of a node's tree, a document node of its own; the
/// empty sequence of the empty sequence.
#[test]
fn roots() {
    let doc = document("<order><tool>wrench</tool><quantity>5</quantity></order>");
    let quantity = elements(&doc, "quantity").remove(0);
    let found = call("root", &[one(&quantity)]).unwrap();
    let [Item::Node(r)] = found.items() else { panic!("one node") };
    assert!(r.is(&doc));
    assert!(call("root", &[empty()]).unwrap().items().is_empty());
    let at = focus().at(Item::Node(quantity), 1, 1);
    assert_eq!(call_in(&at, "root", &[]).unwrap().items().len(), 1);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §14.6.1,
/// §14.7.1, §14.8.1] the examples of the node operators: identity, and
/// order before and after.
#[test]
fn node_operators() {
    let doc = document("<po><item/><item/></po>");
    let items = elements(&doc, "item");
    let (item1, item2) = (&items[0], &items[1]);
    assert!(is_same_node(item1, item1) && !is_same_node(item1, item2));
    assert!(node_before(item1, item2) && !node_before(item1, item1));
    assert!(!node_after(item1, item2) && !node_after(item1, item1) && node_after(item2, item1));
}

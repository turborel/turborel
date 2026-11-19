//! Tests of the functions that generate sequences.

use super::super::testing::{call, call_in, code, document, empty, focus, int, one, s, seq, text, texts};
use crate::xml::{Item, Kind};

/// A document whose DTD makes `id` attributes IDs and `ref` and `refs`
/// IDREF and IDREFS.
const DTD: &str = "<!DOCTYPE r [<!ATTLIST e id ID #IMPLIED ref IDREF #IMPLIED refs IDREFS #IMPLIED>]>\
    <r><e id='a'>A</e><e id='b' ref='a'>B</e><e id='c' refs='a  b'>C</e><e id='a2'/></r>";

/// The string values of the nodes of `value`.
fn values(value: &crate::xml::Sequence) -> Vec<String> {
    texts(value)
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §15.5.2]
/// `fn:id`: the elements of the document with each IDREF token as their
/// ID, in document order, once each; tokens no NCName ignored; a node not
/// in a document err:FODC0001; no context item err:FONC0001.
#[test]
fn ids() {
    let doc = document(DTD);
    assert_eq!(values(&call("id", &[s(" b  a "), one(&doc)]).unwrap()), ["A", "B"]);
    assert_eq!(values(&call("id", &[seq(&[s("c"), s("1x a")]), one(&doc)]).unwrap()), ["A", "C"]);
    assert!(call("id", &[s("z"), one(&doc)]).unwrap().items().is_empty());
    assert!(call("id", &[empty(), one(&doc)]).unwrap().items().is_empty());
    let at = focus().at(Item::Node(doc.clone()), 1, 1);
    assert_eq!(values(&call_in(&at, "id", &[s("c")]).unwrap()), ["C"]);
    assert_eq!(code(call("id", &[s("a")])), "FONC0001");
    let alone = crate::xml::text("x").unwrap();
    assert_eq!(code(call("id", &[s("a"), alone])), "FODC0001");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §15.5.3]
/// `fn:idref`: the attributes that are IDREFs with each ID among their
/// tokens, in document order, once each.
#[test]
fn idrefs() {
    let doc = document(DTD);
    let found = call("idref", &[s("a"), one(&doc)]).unwrap();
    let kinds: Vec<_> = found.items().iter().map(|i| if let Item::Node(n) = i { n.kind() } else { None }).collect();
    assert_eq!(kinds, [Some(Kind::Attribute), Some(Kind::Attribute)]);
    assert_eq!(values(&found), ["a", "a b"]);
    assert_eq!(values(&call("idref", &[seq(&[s("a"), s("b")]), one(&doc)]).unwrap()), ["a", "a b"]);
    assert!(call("idref", &[s("a b"), one(&doc)]).unwrap().items().is_empty());
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §15.5.4 to
/// §15.5.6] no document or collection is available: `fn:doc` of a URI
/// err:FODC0005, of the empty sequence the empty sequence;
/// `fn:doc-available` false; `fn:collection` err:FODC0002 of none,
/// err:FODC0004 of a URI.
#[test]
fn documents_and_collections() {
    assert_eq!(code(call("doc", &[s("foo.xml")])), "FODC0005");
    assert!(call("doc", &[empty()]).unwrap().items().is_empty());
    assert_eq!(text(call("doc-available", &[s("foo.xml")])), "false");
    assert_eq!(text(call("doc-available", &[empty()])), "false");
    assert_eq!(code(call("collection", &[])), "FODC0002");
    assert_eq!(code(call("collection", &[empty()])), "FODC0002");
    assert_eq!(code(call("collection", &[s("c")])), "FODC0004");
    assert_eq!(code(call("doc", &[int(1)])), "XPTY0004");
}

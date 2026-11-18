//! Tests of the functions related to QNames.

use super::super::testing::{atom, call, code, document, elements, empty, int, one, s, text, texts, typed};
use super::*;

/// The one QName of `result`.
fn qname_of(result: Result<Sequence>) -> Atomic {
    match result.unwrap().items() {
        [Item::Atomic(a)] => a.clone(),
        other => panic!("one QName: {other:?}"),
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §11.1.1.2]
/// the examples of `fn:resolve-QName`: a name of no prefix in no
/// namespace where none is default, one of a prefix in its namespace, the
/// prefix kept; the default namespace for a name of none; the empty
/// sequence of the empty sequence; no QName err:FOCA0002; an unbound
/// prefix err:FONS0004.
#[test]
fn resolve_qname_examples() {
    let doc = document("<eg:e xmlns:eg='urn:eg'><d xmlns='urn:d'/></eg:e>");
    let e = one(&elements(&doc, "e").remove(0));
    let hello = qname_of(call("resolve-QName", &[s("hello"), e.clone()]));
    assert_eq!((&*hello.lexical, hello.namespace.as_deref()), ("hello", None));
    let func = qname_of(call("resolve-QName", &[s(" eg:myFunc "), e.clone()]));
    assert_eq!((&*func.lexical, func.namespace.as_deref()), ("eg:myFunc", Some("urn:eg")));
    let d = one(&elements(&doc, "d").remove(0));
    assert_eq!(qname_of(call("resolve-QName", &[s("x"), d])).namespace.as_deref(), Some("urn:d"));
    assert!(call("resolve-QName", &[empty(), e.clone()]).unwrap().items().is_empty());
    assert_eq!(code(call("resolve-QName", &[s("1x"), e.clone()])), "FOCA0002");
    assert_eq!(code(call("resolve-QName", &[s("no:x"), e.clone()])), "FONS0004");
    assert_eq!(code(call("resolve-QName", &[s("x"), one(&doc)])), "XPTY0004");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §11.1.2.1,
/// §11.2.2 to §11.2.4.1] the examples of `fn:QName` and of the parts of a
/// QName; a prefix of no namespace, or no QName, err:FOCA0002.
#[test]
fn qname_and_its_parts() {
    let person = call("QName", &[s("http://www.example.com/example"), s("person")]).unwrap();
    assert_eq!(text(call("local-name-from-QName", std::slice::from_ref(&person))), "person");
    assert!(call("prefix-from-QName", std::slice::from_ref(&person)).unwrap().items().is_empty());
    assert_eq!(typed(call("namespace-uri-from-QName", &[person])), (AtomicType::AnyUri, "http://www.example.com/example".to_owned()));
    let ht = call("QName", &[s("http://www.example.com/example"), s("ht:person")]).unwrap();
    assert_eq!(typed(call("prefix-from-QName", std::slice::from_ref(&ht))), (AtomicType::NcName, "ht".to_owned()));
    assert_eq!(typed(call("local-name-from-QName", &[ht])), (AtomicType::NcName, "person".to_owned()));
    let local = call("QName", &[empty(), s("x")]).unwrap();
    assert_eq!(text(call("namespace-uri-from-QName", &[local])), "");
    assert_eq!(code(call("QName", &[s(""), s("p:x")])), "FOCA0002");
    assert_eq!(code(call("QName", &[s("urn:u"), s("p:")])), "FOCA0002");
    assert!(call("local-name-from-QName", &[empty()]).unwrap().items().is_empty());
    assert_eq!(code(call("local-name-from-QName", &[s("x")])), "XPTY0004");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §11.2.1]
/// `op:QName-equal`: namespace URIs and local names alike, prefixes
/// ignored, local names by the collation given; an operand of another
/// type err:XPTY0004.
#[test]
fn qname_equality() {
    let codepoint = Collator::Codepoint;
    let a = Atomic::qname(Some("urn:u"), "a:x");
    assert!(qname_equal(&a, &Atomic::qname(Some("urn:u"), "b:x"), &codepoint).unwrap());
    assert!(!qname_equal(&a, &Atomic::qname(Some("urn:v"), "a:x"), &codepoint).unwrap());
    assert!(!qname_equal(&a, &Atomic::qname(Some("urn:u"), "a:X"), &codepoint).unwrap());
    let insensitive = Collator::named("CASE_INSENSITIVE").unwrap();
    assert!(qname_equal(&a, &Atomic::qname(Some("urn:u"), "a:X"), &insensitive).unwrap());
    assert!(qname_equal(&a, &Atomic::new(AtomicType::String, "a:x"), &codepoint).is_err());
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §11.2.5,
/// §11.2.6] [XQueryDM §5.7] the namespaces in scope of an element: `xml`,
/// those of its ancestors, its own default; a prefix's URI, the default's
/// of the zero-length string or none, the empty sequence of a prefix not
/// bound.
#[test]
fn namespaces_in_scope() {
    let doc = document("<a xmlns:p='urn:p'><b xmlns='urn:d'/></a>");
    let b = one(&elements(&doc, "b").remove(0));
    let mut prefixes = texts(&call("in-scope-prefixes", std::slice::from_ref(&b)).unwrap());
    prefixes.sort();
    assert_eq!(prefixes, ["", "p", "xml"]);
    assert_eq!(text(call("namespace-uri-for-prefix", &[s("p"), b.clone()])), "urn:p");
    assert_eq!(text(call("namespace-uri-for-prefix", &[s(""), b.clone()])), "urn:d");
    assert_eq!(text(call("namespace-uri-for-prefix", &[empty(), b.clone()])), "urn:d");
    assert_eq!(text(call("namespace-uri-for-prefix", &[s("xml"), b.clone()])), "http://www.w3.org/XML/1998/namespace");
    assert!(call("namespace-uri-for-prefix", &[s("q"), b]).unwrap().items().is_empty());
    assert_eq!(code(call("in-scope-prefixes", &[int(1)])), "XPTY0004");
    assert_eq!(code(call("in-scope-prefixes", &[atom(AtomicType::String, "a")])), "XPTY0004");
}

//! Tests of the static analysis of queries [XQuery §2.2.3.1]: the static
//! context and the static errors it raises.

use crate::xquery::testing::{code_of, collations, evaluate, run, shown};
use crate::xquery::{Environment, Library, Query};

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.20 SR2] [XQuery §2, Appendix
/// C.1] the predeclared prefixes, `sqlxml` among them, and the default
/// function namespace `fn`; a prefix of no statically known namespace
/// err:XPST0081, of a name test err:XPST0008.
#[test]
fn predeclared_namespaces() {
    let q = "declare function local:f() { 1 }; fn:true(), xs:integer('2'), xdt:untypedAtomic('3'), local:f(), true()";
    assert_eq!(run(q), "true 2 3 1 true");
    assert_eq!(run("<sqlxml:a/>"), "<sqlxml:a xmlns:sqlxml=\"http://standards.iso.org/iso/9075/2003/sqlxml\"/>");
    assert_eq!(code_of("<a/>/q:b"), "err:XPST0008");
    assert_eq!(code_of("q:b()"), "err:XPST0081");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.20 SR2d] the namespaces the
/// statement declares in scope are statically known, overriding the
/// predeclared ones, and its default the default element namespace; the
/// prolog's override them, its default element namespace of none an
/// element's [XQuery §4.13, §3.7.4].
#[test]
fn statement_namespaces() {
    let (library, collations) = (Library::builtin(), collations());
    let env = Environment {
        namespaces: vec![("p".to_owned(), "urn:p".to_owned()), ("xs".to_owned(), "urn:not-xs".to_owned())],
        default_element: Some("urn:d".to_owned()),
        variables: Vec::new(),
        library: &library,
        collations: &collations,
        schemas: Vec::new(),
        importable: Vec::new(),
    };
    assert!(Query::compile("<p:a/>, <b/>", &env).is_ok());
    assert!(Query::compile("xs:integer('1')", &env).is_err(), "xs is bound to another namespace");
    assert!(Query::compile("declare namespace xs = 'http://www.w3.org/2001/XMLSchema'; xs:integer('1')", &env).is_ok());
    let undeclared = Query::compile("declare default element namespace ''; <f/>", &env).unwrap();
    assert_eq!(shown(&evaluate(&undeclared, None, Vec::new()).unwrap()), "<f/>", "in no namespace");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQuery §2.2.3.1,
/// Appendix F] the static errors the analysis raises, each SQLSTATE 10000:
/// an unbound prefix err:XPST0081, an undefined variable err:XPST0008, an
/// unknown function err:XPST0017, a pragma unrecognized of no expression
/// err:XQST0079, a schema element test err:XPST0008, a schema import of
/// no registered XML Schema err:XQST0059, a module import err:XQST0016.
#[test]
fn static_errors() {
    assert_eq!(code_of("q:f()"), "err:XPST0081");
    assert_eq!(code_of("element q:a {}"), "err:XPST0081");
    assert_eq!(code_of("$x"), "err:XPST0008");
    assert_eq!(code_of("fn:nosuch()"), "err:XPST0017");
    assert_eq!(code_of("(# q:p #) {1}"), "err:XPST0081");
    assert_eq!(code_of("(# xs:p #) {}"), "err:XQST0079");
    assert_eq!(run("(# xs:p x #) {1}"), "1");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQuery §2.5.4.3,
/// §2.5.4.4, §4.16, §5.2, §3.12.3] the static errors of types,
/// imports, modules and options.
#[test]
fn static_errors_of_types_and_prologs() {
    assert_eq!(code_of("1 instance of schema-element(a)"), "err:XPST0008");
    assert_eq!(code_of("1 instance of element(a, xs:nosuch)"), "err:XPST0008");
    assert_eq!(code_of("import schema 'u'; 1"), "err:XQST0059");
    assert_eq!(code_of("import module 'u'; 1"), "err:XQST0016");
    assert_eq!(code_of("module namespace m = 'u';"), "err:XQST0016");
    assert_eq!(code_of("declare option q:o 'v'; 1"), "err:XPST0081");
    assert_eq!(run("declare option xs:o 'v'; 1"), "1");
    assert_eq!(code_of("1 cast as xs:NOTATION"), "err:XPST0080");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2, GR3] [XQuery §2.5.4.1,
/// §2.5.4.2, §2.5.4.5, §2.5.4.6] the kind tests of attributes and
/// documents: attribute() of any name, of a name, of a type its
/// annotation derives from; document-node(element()) of its element;
/// empty-sequence() of no item; a schema attribute test, as no attribute
/// is declared in scope, err:XPST0008.
#[test]
fn kind_tests_of_attributes_and_documents() {
    let a = "<e b='1'/>/@b";
    let tests = format!(
        "{a} instance of attribute(), {a} instance of attribute(b), {a} instance of attribute(c), \
         {a} instance of attribute(*, xdt:untypedAtomic), {a} instance of attribute(b, xs:integer)"
    );
    assert_eq!(run(&tests), "true true false true false");
    let documents = "document { <a/> } instance of document-node(element(a)), document { <a/> } instance of document-node(), \
                     () instance of empty-sequence(), 1 instance of empty-sequence()";
    assert_eq!(run(documents), "true true true false");
    assert_eq!(code_of("1 instance of schema-attribute(a)"), "err:XPST0008");
}

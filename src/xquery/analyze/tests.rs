//! Tests of the static analysis of queries [XQuery §2.2.3.1]: the static
//! context and the static errors it raises.

use crate::error::Code;
use crate::xquery::testing::{collations, error_of, fails_with, run};
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
    assert!(fails_with("<a/>/q:b", "err:XPST0008"));
    assert!(fails_with("q:b()", "err:XPST0081"));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.20 SR2d] the namespaces the
/// statement declares in scope are statically known, overriding the
/// predeclared ones, and its default the default element namespace; the
/// prolog's override them.
#[test]
fn statement_namespaces() {
    let (library, collations) = (Library::builtin(), collations());
    let env = Environment {
        namespaces: vec![("p".to_owned(), "urn:p".to_owned()), ("xs".to_owned(), "urn:not-xs".to_owned())],
        default_element: Some("urn:d".to_owned()),
        variables: Vec::new(),
        library: &library,
        collations: &collations,
    };
    assert!(Query::compile("<p:a/>, <b/>", &env).is_ok());
    assert!(Query::compile("xs:integer('1')", &env).is_err(), "xs is bound to another namespace");
    assert!(Query::compile("declare namespace xs = 'http://www.w3.org/2001/XMLSchema'; xs:integer('1')", &env).is_ok());
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQuery §2.2.3.1,
/// Appendix F] the static errors the analysis raises, each SQLSTATE 10000:
/// an unbound prefix err:XPST0081, an undefined variable err:XPST0008, an
/// unknown function err:XPST0017, a pragma unrecognized of no expression
/// err:XQST0079, a schema element test err:XPST0008, a schema import
/// err:XQST0009, a module import err:XQST0016; and a validate of lax
/// mode, valid XQuery not processed, SQLCODE -900.
#[test]
fn static_errors() {
    assert!(fails_with("q:f()", "err:XPST0081"));
    assert!(fails_with("element q:a {}", "err:XPST0081"));
    assert!(fails_with("$x", "err:XPST0008"));
    assert!(fails_with("fn:nosuch()", "err:XPST0017"));
    assert!(fails_with("(# q:p #) {1}", "err:XPST0081"));
    assert!(fails_with("(# xs:p #) {}", "err:XQST0079"));
    assert_eq!(run("(# xs:p x #) {1}"), "1");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQuery §2.5.4.3,
/// §2.5.4.4, §4.16, §5.2, §3.12.3] the static errors of types,
/// imports, modules and options, and lax validation SQLCODE -900.
#[test]
fn static_errors_of_types_and_prologs() {
    assert!(fails_with("1 instance of schema-element(a)", "err:XPST0008"));
    assert!(fails_with("1 instance of element(a, xs:nosuch)", "err:XPST0008"));
    assert!(fails_with("import schema 'u'; 1", "err:XQST0009"));
    assert!(fails_with("import module 'u'; 1", "err:XQST0016"));
    assert!(fails_with("module namespace m = 'u';", "err:XQST0016"));
    assert_eq!(error_of("validate lax { <a/> }").code, Code::Unsupported);
    assert!(fails_with("declare option q:o 'v'; 1", "err:XPST0081"));
    assert_eq!(run("declare option xs:o 'v'; 1"), "1");
    assert!(fails_with("1 cast as xs:NOTATION", "err:XPST0080"));
}

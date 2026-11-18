//! Tests of library modules and module imports [XQuery §4.2, §4.11]: the
//! modules a module import finds, each module's own static context, what
//! is imported and what is not, and the static errors of modules.

use std::sync::Arc;

use super::{LibraryModule, library_module};
use crate::error::Result;
use crate::xml::Version;
use crate::xquery::testing::{collations, evaluate, shown};
use crate::xquery::{Environment, Library, Query, parse};

/// The registered XQuery module `A.<ident>` of `text`, at `location`,
/// importing the modules named `imports`, of no schema.
fn registered(ident: &str, location: &str, text: &str, imports: &[&str]) -> LibraryModule {
    LibraryModule {
        name: format!("A.{ident}"),
        ident: ident.to_owned(),
        location: location.to_owned(),
        module: Arc::new(parse(text, Version::V11).expect(text)),
        modules: imports.iter().map(|m| format!("A.{m}")).collect(),
        importable: Vec::new(),
        schemas: Vec::new(),
    }
}

/// `query` compiled with `modules` importable, `linked` imported by them,
/// and the statement's namespaces `namespaces`.
fn compiled(query: &str, modules: Vec<LibraryModule>, linked: Vec<LibraryModule>, namespaces: &[(&str, &str)]) -> Result<Query> {
    let (library, collations) = (Library::builtin(), collations());
    let env = Environment {
        namespaces: namespaces.iter().map(|(p, u)| ((*p).to_owned(), (*u).to_owned())).collect(),
        default_element: None,
        variables: Vec::new(),
        library: &library,
        collations: &collations,
        schemas: Vec::new(),
        importable: Vec::new(),
        modules,
        linked,
    };
    Query::compile(query, &env)
}

/// The value of `query`, serialized, with `modules` importable and
/// `linked` imported by them.
fn value(query: &str, modules: Vec<LibraryModule>, linked: Vec<LibraryModule>) -> String {
    let compiled = compiled(query, modules, linked, &[]).unwrap_or_else(|e| panic!("{query}: {e:?}"));
    shown(&evaluate(&compiled, None, Vec::new()).expect(query))
}

/// The XQuery error code `result` fails with, as its message names it.
fn code<T: std::fmt::Debug>(result: Result<T>) -> String {
    let message = result.expect_err("an error").msg;
    message.split_whitespace().find(|w| w.starts_with("err:")).map_or(message.clone(), |w| w.trim_end_matches(':').to_owned())
}

/// The module of namespace urn:m that doubles by its variable.
const DOUBLING: &str = "module namespace m = 'urn:m'; declare variable $m:v := 2; declare function m:twice($x) { $x * $m:v };";

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery §4.2, §4.11,
/// §4.14, §4.15] a module import binds its
/// prefix to the target namespace and adds the module's functions and
/// variables to the importing module's static context, its functions
/// called and its variables read as its own are; a library module's
/// function calls those of the modules it imports.
#[test]
fn imported_functions_and_variables_are_called_and_read() {
    let m = registered("M", "", DOUBLING, &[]);
    assert_eq!(value("import module namespace m = 'urn:m'; m:twice(21), $m:v", vec![m.clone()], Vec::new()), "42 2");
    let user = "module namespace u = 'urn:u'; import module namespace m = 'urn:m'; declare function u:four() { m:twice(2) };";
    let u = registered("U", "", user, &["M"]);
    assert_eq!(value("import module namespace u = 'urn:u'; u:four()", vec![u], vec![m]), "4");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery §4.11] each
/// module has its own static context: a
/// library module's function constructs by its own construction mode,
/// not the importing module's, and sees no namespace the statement
/// declares [ISO/IEC 9075-14:2006 draft §10.20 SR2d]; and module imports
/// are not transitive.
#[test]
fn each_module_has_its_own_static_context() {
    let strip = "module namespace s = 'urn:s'; declare construction strip; declare function s:e() { <a>{<b/>}</a> };";
    let s = registered("S", "", strip, &[]);
    let typed = "import module namespace s = 'urn:s'; s:e() instance of element(*, xdt:untyped), <a/> instance of element(*, xdt:untyped)";
    assert_eq!(value(typed, vec![s], Vec::new()), "true false");
    let prefixed = registered("P", "", "module namespace p = 'urn:p'; declare function p:e() { q:f() };", &[]);
    assert_eq!(code(compiled("import module 'urn:p'; 1", vec![prefixed], Vec::new(), &[("q", "urn:q")])), "err:XPST0081");
    let m = registered("M", "", DOUBLING, &[]);
    let u = registered("U", "", "module namespace u = 'urn:u'; import module 'urn:m'; declare function u:f() { 1 };", &["M"]);
    let through = "import module namespace u = 'urn:u'; declare namespace m = 'urn:m'; m:twice(1)";
    assert_eq!(code(compiled(through, vec![u], vec![m], &[])), "err:XPST0017");
}

/// Verifies: [EXTENSION] RULES.md D106 [XQuery §4.11]: of the modules of
/// a target namespace, a module import imports the one whose location or
/// name, alone or qualified, is a location hint, else the one of the
/// least name; a library module importing its own target namespace
/// imports another module of it.
#[test]
fn location_hints_choose_among_modules_of_a_namespace() {
    let one = registered("M1", "loc1", "module namespace m = 'urn:m'; declare function m:which() { 1 };", &[]);
    let two = registered("M2", "loc2", "module namespace m = 'urn:m'; declare function m:which() { 2 };", &[]);
    let both = || vec![one.clone(), two.clone()];
    assert_eq!(value("import module namespace m = 'urn:m'; m:which()", both(), Vec::new()), "1");
    assert_eq!(value("import module namespace m = 'urn:m' at 'M2'; m:which()", both(), Vec::new()), "2");
    assert_eq!(value("import module namespace m = 'urn:m' at 'x', 'A.M2'; m:which()", both(), Vec::new()), "2");
    assert_eq!(value("import module namespace m = 'urn:m' at 'loc2'; m:which()", both(), Vec::new()), "2");
    assert_eq!(value("import module namespace m = 'urn:m' at 'nowhere'; m:which()", both(), Vec::new()), "1");
    let own = "module namespace m = 'urn:m'; import module 'urn:m'; declare function m:other() { m:which() + 10 };";
    let three = registered("M3", "", own, &["M1"]);
    assert_eq!(value("import module namespace m = 'urn:m' at 'M3'; m:other()", vec![three], vec![one]), "11");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery §4.11] a module
/// imported by two others is analysed
/// once into the query, its variable evaluated once, so the query does not
/// declare its functions twice.
#[test]
fn a_module_imported_twice_is_analysed_once() {
    let c = registered("C", "", "module namespace c = 'urn:c'; declare variable $c:n := <n/>; declare function c:n() { $c:n };", &[]);
    let a =
        registered("A", "", "module namespace a = 'urn:a'; import module namespace c = 'urn:c'; declare function a:n() { c:n() };", &["C"]);
    let b =
        registered("B", "", "module namespace b = 'urn:b'; import module namespace c = 'urn:c'; declare function b:n() { c:n() };", &["C"]);
    let query = "import module namespace a = 'urn:a'; import module namespace b = 'urn:b'; a:n() is b:n()";
    assert_eq!(value(query, vec![a, b], vec![c]), "true");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQuery §4.11, §4.2,
/// §4.14, §4.15, Appendix F] the static
/// errors of module imports: a namespace imported twice err:XQST0047, one
/// no module has err:XQST0059, a function of the name and arity of one
/// imported err:XQST0034, a variable of the name of one imported
/// err:XQST0049; and of library modules: a function or variable not in
/// the target namespace err:XQST0048, a variable whose initializing
/// expression depends on itself err:XQST0054, each found as a module
/// importing it is analysed.
#[test]
fn the_static_errors_of_modules() {
    let m = || vec![registered("M", "", DOUBLING, &[])];
    assert_eq!(code(compiled("import module 'urn:m'; import module 'urn:m'; 1", m(), Vec::new(), &[])), "err:XQST0047");
    assert_eq!(code(compiled("import module 'urn:none'; 1", m(), Vec::new(), &[])), "err:XQST0059");
    let declared = "import module namespace m = 'urn:m'; declare function m:twice($y) { $y }; 1";
    assert_eq!(code(compiled(declared, m(), Vec::new(), &[])), "err:XQST0034");
    assert_eq!(code(compiled("import module namespace m = 'urn:m'; declare variable $m:v := 3; 1", m(), Vec::new(), &[])), "err:XQST0049");
    for wrong in [
        "module namespace m = 'urn:m'; declare function local:f() { 1 };",
        "module namespace m = 'urn:m'; declare variable $v := 1;",
        "module namespace m = 'urn:m'; declare namespace o = 'urn:o'; declare variable $o:v := 1;",
    ] {
        assert_eq!(code(compiled("import module 'urn:m'; 1", vec![registered("W", "", wrong, &[])], Vec::new(), &[])), "err:XQST0048");
    }
    let circular = "module namespace m = 'urn:m'; declare variable $m:v := m:f(); declare function m:f() { $m:v };";
    assert_eq!(code(compiled("import module 'urn:m'; 1", vec![registered("C", "", circular, &[])], Vec::new(), &[])), "err:XQST0054");
}

/// Verifies: [DESIGN] RULES.md R-CF-6, D106 [XQuery §4.11, Appendix F]: a
/// cycle of module imports not all of one namespace is
/// err:XQST0073; a chain of imports longer than `MAX_IMPORT_DEPTH` is
/// SQLCODE -900, and one as long is analysed.
#[test]
fn cycles_and_chains_of_imports() {
    let a = registered("A", "", "module namespace a = 'urn:a'; import module 'urn:b'; declare function a:f() { 1 };", &["B"]);
    let b = registered("B", "", "module namespace b = 'urn:b'; import module 'urn:a'; declare function b:f() { 1 };", &["A"]);
    assert_eq!(code(compiled("import module 'urn:a'; 1", vec![a], vec![b], &[])), "err:XQST0073");
    let chain = |n: usize| {
        let mut modules = vec![registered("L0", "", "module namespace l = 'urn:l0'; declare function l:f() { 0 };", &[])];
        for k in 1..n {
            let text = format!(
                "module namespace l = 'urn:l{k}'; import module namespace p = 'urn:l{}'; declare function l:f() {{ p:f() + 1 }};",
                k - 1
            );
            modules.push(registered(&format!("L{k}"), "", &text, &[&format!("L{}", k - 1)]));
        }
        let top = modules.pop().unwrap();
        compiled(&format!("import module namespace t = 'urn:l{}'; t:f()", n - 1), vec![top], modules, &[])
    };
    let deepest = chain(super::super::MAX_IMPORT_DEPTH).unwrap();
    assert_eq!(shown(&evaluate(&deepest, None, Vec::new()).unwrap()), (super::super::MAX_IMPORT_DEPTH - 1).to_string());
    assert_eq!(chain(super::super::MAX_IMPORT_DEPTH + 1).unwrap_err().sqlcode(), -900_i32);
}

/// Verifies: [EXTENSION] RULES.md D106 [XQuery §4.2, §4.11]: a library
/// module is analysed as it is registered, giving its target namespace and
/// the modules it imported; a main module is no library module and a
/// library module no query, err:XPST0003; a query names the modules it
/// imports itself, not those they import.
#[test]
fn library_modules_are_analysed_as_registered() {
    let (library, collations) = (Library::builtin(), collations());
    let env = Environment {
        namespaces: Vec::new(),
        default_element: None,
        variables: Vec::new(),
        library: &library,
        collations: &collations,
        schemas: Vec::new(),
        importable: Vec::new(),
        modules: vec![registered("M", "", DOUBLING, &[])],
        linked: Vec::new(),
    };
    assert_eq!(code(library_module("import module namespace m = 'urn:m'; m:twice(1)", ("A.U", "U"), "", &env)), "err:XPST0003");
    let fixed = "module namespace u = 'urn:u'; import module namespace m = 'urn:m'; declare function u:f() { m:twice(1) };";
    let analysed = library_module(fixed, ("A.U", "U"), "", &env).unwrap();
    assert_eq!((analysed.namespace.as_str(), analysed.modules.clone(), analysed.schemas.len()), ("urn:u", vec!["A.M".to_owned()], 0));
    assert_eq!(
        code(library_module("module namespace u = 'urn:u'; declare function u:f() { $u:x };", ("A.U", "U"), "", &env)),
        "err:XPST0008"
    );
    assert_eq!(code(Query::compile(DOUBLING, &env)), "err:XPST0003");
    let u = registered("U", "", fixed, &["M"]);
    let query = compiled("import module namespace u = 'urn:u'; u:f()", vec![u], env.modules.clone(), &[]).unwrap();
    assert_eq!(query.modules(), ["A.U".to_owned()]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery §4.11, §4.15] a
/// cycle of modules of one namespace is allowed: each is analysed once,
/// the functions of each calling the other's, recursively, and their
/// variables read across the cycle; of three modules too; a query
/// importing the cycle at either module calls both; calls across the
/// cycle nest no deeper than the evaluation's bound.
#[test]
fn modules_of_one_namespace_import_one_another() {
    let even = "module namespace m = 'urn:m'; import module 'urn:m'; declare variable $m:zero := 0; declare function m:even($n) { if ($n = $m:zero) then fn:true() else m:odd($n - 1) };";
    let odd = "module namespace m = 'urn:m'; import module 'urn:m'; declare function m:odd($n) { if ($n = $m:zero) then fn:false() else m:even($n - 1) };";
    let (e, o) = (registered("EVEN", "", even, &["ODD"]), registered("ODD", "", odd, &["EVEN"]));
    let both = || vec![e.clone(), o.clone()];
    assert_eq!(value("import module namespace m = 'urn:m' at 'EVEN'; m:even(10), m:even(7)", both(), Vec::new()), "true false");
    assert_eq!(value("import module namespace m = 'urn:m' at 'ODD'; m:odd(9)", both(), Vec::new()), "true");
    let one = registered(
        "C1",
        "",
        "module namespace c = 'urn:c'; import module 'urn:c'; declare function c:one($n) { if ($n = 0) then 'done' else c:two($n) };",
        &["C2"],
    );
    let two =
        registered("C2", "", "module namespace c = 'urn:c'; import module 'urn:c'; declare function c:two($n) { c:three($n) };", &["C3"]);
    let three = registered(
        "C3",
        "",
        "module namespace c = 'urn:c'; import module 'urn:c'; declare function c:three($n) { c:one($n - 1) };",
        &["C1"],
    );
    assert_eq!(value("import module namespace c = 'urn:c' at 'C2'; c:two(3)", vec![one, two, three], Vec::new()), "done");
    let deep = compiled("import module namespace m = 'urn:m' at 'EVEN'; m:even(100000)", both(), Vec::new(), &[]).unwrap();
    assert_eq!(evaluate(&deep, None, Vec::new()).unwrap_err().sqlcode(), -900_i32, "calls across the cycle nest to MAX_DEPTH");
}

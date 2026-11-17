//! Tests of modules and prologs: version and module declarations, the
//! setters, namespace declarations and imports, the variable, function
//! and option declarations, and the static errors a prolog shows by
//! itself.

use crate::xquery::tests::{body, code, codes, kinds, module, round_trips, syntax_error, syntax_errors, written};
use crate::xquery::{
    BoundarySpace, Construction, Declaration, DefaultNamespaceDecl, EmptyOrder, Expr, Import, InheritMode, ModuleBody, ModuleImport,
    NamespaceDecl, NamespaceKind, OrderingMode, PreserveMode, SchemaImport, SchemaPrefix, Setter, Setup, VersionDecl,
};

/// The first part of the prolog of `text`.
fn setup(text: &str) -> Vec<Setup> {
    module(text).prolog().setup.clone()
}

/// The second part of the prolog of `text`.
fn declarations(text: &str) -> Vec<Declaration> {
    module(text).prolog().declarations.clone()
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery §4.1] [XQuery
/// Appendix A.1 \[2\]] a version declaration: version 1.0 alone, else
/// err:XQST0031; an encoding that is an `EncName`, else err:XQST0087;
/// its separator.
#[test]
fn version_declarations() {
    assert_eq!(module("xquery version '1.0'; 1").version, Some(VersionDecl { version: "1.0".to_owned(), encoding: None }));
    assert_eq!(
        module("xquery version \"1.0\" encoding \"utf-8\"; 1").version,
        Some(VersionDecl { version: "1.0".to_owned(), encoding: Some("utf-8".to_owned()) })
    );
    assert_eq!(module("xquery version '1.0' encoding 'ISO-8859-1'; 1").version.unwrap().encoding.as_deref(), Some("ISO-8859-1"));
    assert_eq!(module("1").version, None);
    assert_eq!(code("xquery version '2.0'; 1"), "XQST0031");
    assert_eq!(code("xquery version '1'; 1"), "XQST0031");
    for bad in ["8bit", "", "utf 8", "\u{e9}"] {
        assert_eq!(code(&format!("xquery version '1.0' encoding '{bad}'; 1")), "XQST0087", "{bad}");
    }
    assert_eq!(code("xquery version '1.0' 1"), "XPST0003");
    assert_eq!(code("xquery version 1.0; 1"), "XPST0003");
    assert_eq!(code("1; xquery version '1.0'"), "XPST0003");
    assert!(matches!(body("xquery"), Expr::Path(_)));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery §4.2] [XQuery
/// Appendix A.1 \[1\], \[4\], \[5\]] a library module is its module
/// declaration and a prolog, with no query body; a main module has one.
#[test]
fn library_and_main_modules() {
    let library = module("xquery version '1.0'; module namespace m = 'u'; declare variable $m:a := 1;");
    let ModuleBody::Library(decl, prolog) = &library.body else { panic!() };
    assert_eq!((decl.prefix.as_str(), decl.uri.as_str(), prolog.declarations.len()), ("m", "u", 1));
    assert_eq!(code("module namespace m = 'u'; 1"), "XPST0003");
    assert_eq!(code("module namespace m = 'u'"), "XPST0003");
    assert_eq!(code("module namespace m:n = 'u';"), "XPST0003");
    assert_eq!(code("declare variable $a := 1;"), "XPST0003");
    assert!(matches!(module("module").body, ModuleBody::Main(..)));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery §4.3 to §4.9,
/// §4.12, §4.13] [XQuery Appendix A.1 \[6\], \[7\], \[10\] to \[12\],
/// \[14\] to \[20\], \[25\]] each setter and namespace declaration, as its
/// production writes it.
#[test]
fn setters_and_namespace_declarations() {
    let text = "declare boundary-space strip; declare boundary-space preserve; 1";
    assert_eq!(code(text), "XQST0068");
    assert_eq!(
        setup(
            "declare boundary-space strip; declare default collation 'c'; declare base-uri 'b'; declare construction preserve; \
             declare ordering ordered; declare default order empty greatest; declare copy-namespaces preserve, inherit; \
             declare namespace p = 'u'; declare default element namespace 'e'; declare default function namespace ''; 1"
        ),
        vec![
            Setup::Setter(Setter::BoundarySpace(BoundarySpace::Strip)),
            Setup::Setter(Setter::DefaultCollation("c".to_owned())),
            Setup::Setter(Setter::BaseUri("b".to_owned())),
            Setup::Setter(Setter::Construction(Construction::Preserve)),
            Setup::Setter(Setter::OrderingMode(OrderingMode::Ordered)),
            Setup::Setter(Setter::EmptyOrder(EmptyOrder::Greatest)),
            Setup::Setter(Setter::CopyNamespaces(PreserveMode::Preserve, InheritMode::Inherit)),
            Setup::Namespace(NamespaceDecl { prefix: "p".to_owned(), uri: "u".to_owned() }),
            Setup::DefaultNamespace(DefaultNamespaceDecl { kind: NamespaceKind::Element, uri: "e".to_owned() }),
            Setup::DefaultNamespace(DefaultNamespaceDecl { kind: NamespaceKind::Function, uri: String::new() }),
        ]
    );
    for bad in [
        "declare boundary-space keep; 1",
        "declare construction; 1",
        "declare ordering sorted; 1",
        "declare default order greatest; 1",
        "declare default order empty most; 1",
        "declare copy-namespaces preserve inherit; 1",
        "declare copy-namespaces inherit, preserve; 1",
        "declare namespace p 'u'; 1",
        "declare namespace p:q = 'u'; 1",
        "declare default element 'e'; 1",
        "declare base-uri b; 1",
        "declare boundary-space strip 1",
    ] {
        assert_eq!(code(bad), "XPST0003", "{bad}");
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery §4.3 to §4.9,
/// §4.13] [XQuery Appendix F] a prolog declares each setter and each
/// default namespace once at most, the error of each its own.
#[test]
fn setters_are_declared_once() {
    for (twice, error) in [
        ("declare boundary-space strip", "XQST0068"),
        ("declare default collation 'c'", "XQST0038"),
        ("declare base-uri 'b'", "XQST0032"),
        ("declare construction strip", "XQST0067"),
        ("declare ordering unordered", "XQST0065"),
        ("declare default order empty least", "XQST0069"),
        ("declare copy-namespaces no-preserve, no-inherit", "XQST0055"),
        ("declare default element namespace 'e'", "XQST0066"),
        ("declare default function namespace 'f'", "XQST0066"),
    ] {
        assert_eq!(code(&format!("{twice}; declare namespace p = 'u'; {twice}; 1")), error, "{twice}");
        assert_eq!(written(&format!("{twice}; 1")), format!("{}; 1", twice.replace('\'', "\"")));
    }
    assert!(module("declare default element namespace 'e'; declare default function namespace 'f'; 1").unsupported().is_none());
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery §4.12] [XQuery
/// Appendix F] a prolog declares each prefix once, by a namespace
/// declaration, its module declaration or an import \[err:XQST0033\];
/// never `xml` or `xmlns`, and no prefix bound to `xml`'s namespace
/// \[err:XQST0070\].
#[test]
fn prefixes_are_declared_once() {
    assert_eq!(code("declare namespace p = 'u'; declare namespace p = 'v'; 1"), "XQST0033");
    assert_eq!(code("module namespace p = 'u'; declare namespace p = 'v';"), "XQST0033");
    assert_eq!(code("declare namespace p = 'u'; import module namespace p = 'v'; 1"), "XQST0033");
    assert_eq!(code("import schema namespace p = 'u'; declare namespace p = 'v'; 1"), "XQST0033");
    assert_eq!(code("declare namespace xml = 'u'; 1"), "XQST0070");
    assert_eq!(code("declare namespace xmlns = 'u'; 1"), "XQST0070");
    assert_eq!(code("declare namespace p = 'http://www.w3.org/XML/1998/namespace'; 1"), "XQST0070");
    assert_eq!(code("module namespace xml = 'u';"), "XQST0070");
    assert_eq!(setup("declare namespace p = 'u'; declare namespace q = 'u'; 1").len(), 2);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery §4.10, §4.11]
/// [XQuery Appendix A.1 \[21\] to \[23\]] schema and module imports, with
/// their prefixes and location hints; a schema import binding a prefix
/// has a target namespace \[err:XQST0057\], and a module import's is of
/// nonzero length \[err:XQST0088\].
#[test]
fn imports() {
    assert_eq!(
        setup("import schema namespace s = 'u' at 'a', 'b'; import schema default element namespace 'v'; import schema ''; 1"),
        vec![
            Setup::Import(Import::Schema(SchemaImport {
                prefix: Some(SchemaPrefix::Namespace("s".to_owned())),
                uri: "u".to_owned(),
                at: vec!["a".to_owned(), "b".to_owned()],
            })),
            Setup::Import(Import::Schema(SchemaImport { prefix: Some(SchemaPrefix::DefaultElement), uri: "v".to_owned(), at: Vec::new() })),
            Setup::Import(Import::Schema(SchemaImport { prefix: None, uri: String::new(), at: Vec::new() })),
        ]
    );
    assert_eq!(
        setup("import module namespace m = 'u' at 'l'; import module 'v'; 1"),
        vec![
            Setup::Import(Import::Module(ModuleImport { prefix: Some("m".to_owned()), uri: "u".to_owned(), at: vec!["l".to_owned()] })),
            Setup::Import(Import::Module(ModuleImport { prefix: None, uri: "v".to_owned(), at: Vec::new() })),
        ]
    );
    assert_eq!(code("import schema namespace s = ''; 1"), "XQST0057");
    assert_eq!(code("import module ''; 1"), "XQST0088");
    assert_eq!(code("import module namespace m = '' at 'l'; 1"), "XQST0088");
    for bad in ["import schema namespace s 'u'; 1", "import schema 'u' at; 1", "import module 'u' at 'a',; 1", "import module m = 'u'; 1"] {
        assert_eq!(code(bad), "XPST0003", "{bad}");
    }
    assert!(matches!(body("import"), Expr::Path(_)));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery §4.14 to §4.16]
/// [XQuery Appendix A.1 \[13\], \[24\], \[26\] to \[28\]] variable
/// declarations initialized or external, function declarations with
/// typed parameters, a return type, and a body or `external`; option
/// declarations; each after the setters.
#[test]
fn variable_function_and_option_declarations() {
    let text = "declare variable $a as xs:int := 1; declare variable $b external; declare function f($x as item()*, $y) as xs:int { $x }; \
                declare function g() external; declare option p:o \"v\"; 1";
    round_trips(&[text]);
    let declared = declarations(text);
    let Declaration::Function(function) = &declared[2] else { panic!() };
    assert_eq!((declared.len(), function.params.len()), (5, 2));
    syntax_errors(&[
        "declare variable $a; 1",
        "declare variable a := 1; 1",
        "declare variable $a = 1; 1",
        "declare function f() { }; 1",
        "declare function f(a) { 1 }; 1",
        "declare function f($a,) { 1 }; 1",
        "declare function f { 1 }; 1",
        "declare option o; 1",
        "declare option o 1; 1",
        "declare variable $a := 1; declare boundary-space strip; 1",
        "declare variable $a := 1 1",
    ]);
    assert!(syntax_error("declare option o 'v'; declare namespace p = 'u'; 1").contains("come before its variable"));
    kinds(&[("declare", "Path"), ("declare - 1", "Additive")]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery §4.15] [XQuery
/// Appendix F] a function declaration names each of its parameters once
/// \[err:XQST0039\]; parameters whose names differ as written are told
/// apart only once their prefixes are resolved.
#[test]
fn function_parameters_are_named_once() {
    codes(&[("declare function f($a, $b, $a) { 1 }; 1", "XQST0039"), ("declare function f($p:a, $p:a) external; 1", "XQST0039")]);
    assert_eq!(declarations("declare function f($a, $p:a) { 1 }; 1").len(), 1);
}

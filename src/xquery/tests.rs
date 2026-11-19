//! Tests of XQuery parsing as a whole, and what the tests of its parts
//! share: a query parsed, its tree written back and parsed again, its
//! operators' structure written out, and the error codes it raises.

use super::*;
use crate::error::{Code, SqlState};

/// The module `text` is, by XML 1.1's lexical rules.
pub(super) fn module(text: &str) -> Module {
    parse(text, Version::V11).unwrap_or_else(|e| panic!("{text}: {e}"))
}

/// The query body of the main module `text`.
pub(super) fn body(text: &str) -> Expr {
    match module(text).body {
        ModuleBody::Main(_, body) => body,
        ModuleBody::Library(..) => panic!("{text} is a library module"),
    }
}

/// `text` parsed, its tree written back as text and that parsed again:
/// the text written, once both trees are checked to be the same.
pub(super) fn written(text: &str) -> String {
    let first = module(text);
    let again = first.to_string();
    let second = parse(&again, Version::V11).unwrap_or_else(|e| panic!("{text} written as {again}: {e}"));
    assert_eq!(first, second, "{text} written as {again}");
    again
}

/// The XQuery error code `text` raises by `version`'s rules, as
/// `XPST0003`, checking the condition is SQL/XML's XQuery error.
pub(super) fn code_by(text: &str, version: Version) -> String {
    let error = parse(text, version).map(|m| m.to_string()).unwrap_err();
    assert_eq!(error.sqlstate(), SqlState::XQUERY_ERROR, "{text}: {error}");
    assert_eq!(error.rule.as_deref(), Some("ISO/IEC 9075-14:2006 draft §6.18 GR3"), "{text}: {error}");
    let after = error.msg.split_once("err:").unwrap_or_else(|| panic!("{text}: {error}")).1;
    after.chars().take(8).collect()
}

/// The kind of the expression the query body `text` is: its variant's
/// name, as `Flwor`.
pub(super) fn kind(text: &str) -> String {
    let debug = format!("{:?}", body(text));
    debug.split(['(', ' ']).next().unwrap().to_owned()
}

/// Checks each query body of `cases` is of its kind.
pub(super) fn kinds(cases: &[(&str, &str)]) {
    for (text, expected) in cases {
        assert_eq!(kind(text), *expected, "{text}");
    }
}

/// Checks each query body of `cases` has its operators' shape.
pub(super) fn shapes(cases: &[(&str, &str)]) {
    for (text, expected) in cases {
        assert_eq!(shape_of(text), *expected, "{text}");
    }
}

/// Checks each query of `cases` raises its error code.
pub(super) fn codes(cases: &[(&str, &str)]) {
    for (text, expected) in cases {
        assert_eq!(code(text), *expected, "{text}");
    }
}

/// Checks each of `texts` is a syntax error, err:XPST0003.
pub(super) fn syntax_errors(texts: &[&str]) {
    for text in texts {
        assert_eq!(code(text), "XPST0003", "{text}");
    }
}

/// Checks each of `texts` is written back as it is.
pub(super) fn round_trips(texts: &[&str]) {
    for text in texts {
        assert_eq!(written(text), *text);
    }
}

/// The XQuery error code `text` raises by XML 1.1's rules.
pub(super) fn code(text: &str) -> String {
    code_by(text, Version::V11)
}

/// The message of the syntax error `text` raises.
pub(super) fn syntax_error(text: &str) -> String {
    assert_eq!(code(text), "XPST0003", "{text}");
    parse(text, Version::V11).unwrap_err().to_string()
}

/// `expr`'s operators written out, each node in parentheses with its
/// kind first, so precedence and chains show; any other node as it is
/// written back.
pub(super) fn shape(expr: &Expr) -> String {
    let list = |kind: &str, items: &[Expr]| format!("({kind} {})", items.iter().map(shape).collect::<Vec<_>>().join(" "));
    match expr {
        Expr::Sequence(items) => list(",", items),
        Expr::Or(items) => list("or", items),
        Expr::And(items) => list("and", items),
        Expr::Union(items) => list("union", items),
        Expr::Comparison(c) => format!("({:?} {} {})", c.op, shape(&c.left), shape(&c.right)),
        Expr::Range(operands) => {
            let [from, to] = &**operands;
            format!("(to {} {})", shape(from), shape(to))
        }
        Expr::Additive(first, rest) => chain(first, rest),
        Expr::Multiplicative(first, rest) => chain(first, rest),
        Expr::IntersectExcept(first, rest) => chain(first, rest),
        Expr::InstanceOf(operand, t) => format!("(instance-of {} {t})", shape(operand)),
        Expr::Treat(operand, t) => format!("(treat {} {t})", shape(operand)),
        Expr::Castable(operand, t) => format!("(castable {} {t})", shape(operand)),
        Expr::Cast(operand, t) => format!("(cast {} {t})", shape(operand)),
        Expr::Unary(signs, operand) => format!("({signs:?} {})", shape(operand)),
        Expr::Parenthesized(Some(inner)) => format!("(paren {})", shape(inner)),
        Expr::Flwor(_)
        | Expr::Quantified(_)
        | Expr::Typeswitch(_)
        | Expr::If(_)
        | Expr::Validate(..)
        | Expr::Extension(..)
        | Expr::Path(_)
        | Expr::Filter(..)
        | Expr::Literal(_)
        | Expr::Variable(_)
        | Expr::Parenthesized(None)
        | Expr::ContextItem
        | Expr::FunctionCall(..)
        | Expr::Ordered(_)
        | Expr::Unordered(_)
        | Expr::Constructor(_) => expr.to_string(),
    }
}

/// A chain's operands and operators written out.
fn chain<T: std::fmt::Debug>(first: &Expr, rest: &[(T, Expr)]) -> String {
    let rest: Vec<String> = rest.iter().map(|(op, operand)| format!("{op:?} {}", shape(operand))).collect();
    format!("({} {})", shape(first), rest.join(" "))
}

/// The shape of the query body `text`.
pub(super) fn shape_of(text: &str) -> String {
    shape(&body(text))
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery Appendix A.1] a
/// query of every production, written back, parses to the same tree, and
/// is written back the same again.
#[test]
fn every_production_round_trips() {
    for text in [
        "xquery version '1.0' encoding 'UTF-8'; declare boundary-space preserve; declare default collation 'c'; declare base-uri 'b'; declare construction strip; declare ordering unordered; declare default order empty least; declare copy-namespaces no-preserve, no-inherit; declare namespace p = 'u'; declare default element namespace 'e'; declare default function namespace 'f'; declare variable $p:v as xs:integer* := 1; declare variable $w external; declare function p:f($a as item()?, $b) as node()+ { $a }; declare function p:g() external; declare option p:o 'x'; p:f(1, 2)",
        "import schema namespace s = 'u' at 'a', 'b'; import schema default element namespace 'v'; import schema 'w'; import module namespace m = 'm' at 'l'; import module 'n'; 1",
        "module namespace m = 'u'; declare variable $m:x := 1;",
        "for $x as xs:integer at $i in (1, 2), $y in $x let $z := $x, $w as item() := 2 for $q in 3 where $x > 1 stable order by $x ascending empty greatest collation 'c', $y descending, $z empty least return ($x, $i)",
        "some $x in 1, $y as xs:integer in 2 satisfies $x = $y",
        "every $x in (1, 2) satisfies $x",
        "typeswitch ($a) case $i as xs:integer return 1 case element(a, t?)* return 2 default $d return 3",
        "typeswitch ($a) case empty-sequence() return 1 default return 2",
        "if ($a) then 1 else if ($b) then 2 else 3",
        "1 or 2 and 3 or 4 = 5 and 6 != 7 and 8 < 9 and 1 <= 2 and 3 > 4 and 5 >= 6",
        "1 eq 2 or 1 ne 2 or 1 lt 2 or 1 le 2 or 1 gt 2 or 1 ge 2 or $a is $b or $a << $b or $a >> $b",
        "1 to 2 + 3 - 4 * 5 div 6 idiv 7 mod 8",
        "$a union $b | $c intersect $d except $e",
        "$a instance of xs:integer?, $a treat as node()*, $a castable as xs:int?, $a cast as xs:int, - + -$a cast as xs:int",
        "validate { $a }, validate lax { $a }, validate strict { $a }",
        "(# p:q some contents #) (#r#) { 1 }, (# x:y #) { }",
        "/, //a, /a/b//c, a/b, child::a/descendant::b/attribute::c/self::d/descendant-or-self::e/following-sibling::f/following::g",
        "parent::a/ancestor::b/preceding-sibling::c/preceding::d/ancestor-or-self::e, ../@a/@*/p:*/*:l/*",
        "a[1][b = 2]/(c, d)[3]/.., $x[1], (1, 2)[. > 1], f(1)[2], .",
        "node(), text(), comment(), processing-instruction(), processing-instruction(t), processing-instruction('t'), document-node(), document-node(element(a)), document-node(schema-element(s)), element(), element(*), element(a, t), attribute(), attribute(*, t), attribute(a), schema-element(a), schema-attribute(b)",
        "1, 2.5, .5, 1e3, 'it''s', \"a&amp;b\", '&#xD;', $v, (), f(), p:g(1, (2, 3)), ordered { 1 }, unordered { 2 }",
        "document { 1 }, element a { }, element { 'a' } { 1 }, attribute b { 'v' }, attribute { 'c' } { }, text { 't' }, comment { 'c' }, processing-instruction p { 'x' }, processing-instruction { 'q' } { }",
        "<a b='1' c=\"{ 2 }x&lt;\" xmlns:p='u'>t{1}<!--c--><?p i?><![CDATA[<&>]]>&#x20;&amp;{{}}<b/></a>",
        "<a>  {1}  <b> z </b>\n</a>, <!-- c -->, <?t?>, <?t  x y?>",
        "4 treat as item()+ - 5, (4 treat as item()) + -5, (/) * 5, 5 * /, / < 3, / = 1",
        "for (: whom the bell :) $tolls in 3 return $tolls, 5 instance (: strange :) of xs:integer, address (: may be empty :)",
    ] {
        let once = written(text);
        assert_eq!(written(&once), once, "{text}");
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery Appendix A.1] a
/// tree is written back in one form: keywords and operators spaced,
/// string literals in quotation marks, `|` as `union`, `<a></a>` as
/// `<a/>`, comments dropped.
#[test]
fn trees_are_written_in_one_form() {
    assert_eq!(written("1+2*3"), "1 + 2 * 3");
    assert_eq!(written("'a''b'"), "\"a'b\"");
    assert_eq!(written("\"q\"\"&lt;\""), "\"q\"\"<\"");
    assert_eq!(written("$a|$b"), "$a union $b");
    assert_eq!(written("<a></a>"), "<a/>");
    assert_eq!(written("1 (: gone :)"), "1");
    assert_eq!(written("<a x='\"{{'/>"), "<a x=\"\"\"{{\"/>");
    assert_eq!(written("<a>&lt;&#65;</a>"), "<a>&lt;&#x41;</a>");
    assert_eq!(written("'&#xD;&#x85;'"), "\"&#xD;&#x85;\"");
    assert_eq!(written("for $x in 1 return $x"), "for $x in 1 return $x");
    assert_eq!(written("xquery version '1.0'; 1"), "xquery version \"1.0\"; 1");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3, §23.1] [XQuery
/// Appendix F] [XQuery §2.3.2] an XQuery error is SQLSTATE 10000, XQuery
/// error, its message naming the error's code and where the text breaks
/// the rule; a syntax error is err:XPST0003.
#[test]
fn errors_are_xquery_errors() {
    let error = parse("1 +", Version::V11).unwrap_err();
    assert_eq!(error.sqlstate(), SqlState::XQUERY_ERROR);
    assert_eq!(error.sqlstate().to_string(), "10000");
    assert_eq!(error.code, Code::Data);
    assert!(error.msg.starts_with("XQuery error err:XPST0003 at line 1, column 4: the end of the query is not what"), "{error}");
    assert_eq!(code("1 2"), "XPST0003");
    assert_eq!(ErrorCode::XQST0031.to_string(), "err:XQST0031");
    assert_eq!(ErrorCode::XPST0003.as_str(), "XPST0003");
}

/// Queries nested `n` levels by one shape each: parentheses, predicates,
/// direct elements and their enclosed expressions, function calls,
/// conditional and FLWOR expressions, unary operators, and operators of
/// every precedence, whose right operands nest one level each.
pub(super) fn nesting_shapes() -> [(&'static str, &'static str); 10] {
    [
        ("(", ")"),
        ("a[", "]"),
        ("<a>{", "}</a>"),
        ("<a>", "</a>"),
        ("f(", ")"),
        ("if (1) then ", " else 2"),
        ("for $x in ", " return $x"),
        ("-(", ")"),
        ("<a b='{", "}'/>"),
        ("1 or 1 and 1 = 1 to 1 + 1 * 1 | 1 intersect (", ")"),
    ]
}

/// Runs `parse` on a thread with half the default stack, 1 MiB; a stack
/// overflow aborts the test.
pub(super) fn on_half_stack(parse: impl FnOnce() -> bool + Send + 'static) -> bool {
    const HALF_DEFAULT_STACK: usize = 1024 * 1024;
    std::thread::Builder::new().name("xquery-nesting".to_owned()).stack_size(HALF_DEFAULT_STACK).spawn(parse).unwrap().join().unwrap()
}

/// How many times `open` and `close` around `1` make the first query
/// refused for its nesting.
fn first_rejected(open: &str, close: &str) -> usize {
    let deep = |n: usize| format!("{}1{}", open.repeat(n), close.repeat(n));
    (1..=MAX_NESTING + 1).find(|&n| parse(&deep(n), Version::V11).is_err()).unwrap()
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [DESIGN] RULES.md R-CF-6,
/// D106: `MAX_NESTING` levels are the query body and each expression or
/// direct element nested in it, so 63 parentheses or elements around an
/// operand parse and 64 do not.
#[test]
fn nesting_counts_each_level_once() {
    assert_eq!(first_rejected("(", ")"), MAX_NESTING);
    assert_eq!(first_rejected("<a>", "</a>"), MAX_NESTING);
    assert_eq!(first_rejected("<a>{", "}</a>"), MAX_NESTING.checked_div(2).unwrap());
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [DESIGN] RULES.md R-CF-6,
/// D106: a query nested deeper than `MAX_NESTING`, of each shape, is
/// refused with SQLCODE -900, never a stack overflow; the deepest one
/// accepted parses, is written back, parsed again, compared and dropped
/// on a thread of half the default stack, so the bound keeps the parser
/// and every walk of its tree within the stack.
#[test]
fn nesting_is_bounded_within_the_stack() {
    for (open, close) in nesting_shapes() {
        let deep = move |n: usize| format!("{}1{}", open.repeat(n), close.repeat(n));
        let first_rejected = first_rejected(open, close);
        let error = parse(&deep(first_rejected), Version::V11).unwrap_err();
        assert_eq!(error.code, Code::Unsupported, "{open}: {error}");
        assert!(error.msg.contains("deeper than turborel's limit of 64 levels"), "{error}");
        assert_eq!(parse(&deep(100_000), Version::V11).unwrap_err().code, Code::Unsupported, "{open}");
        let deepest = deep(first_rejected - 1);
        assert!(
            on_half_stack(move || {
                let first = parse(&deepest, Version::V11).unwrap();
                first == parse(&first.to_string(), Version::V11).unwrap()
            }),
            "{open}"
        );
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery §4.2, §4.11,
/// §5.2.5] a library module is parsed as its module declaration and
/// prolog, a module import as its prefix, target namespace and location
/// hints, as turborel supports the Module Feature; a main module of no
/// prolog has an empty one.
#[test]
fn imports_and_library_modules_are_parsed() {
    let library = module("module namespace m = 'urn:m'; declare function m:f() { 1 };");
    let ModuleBody::Library(decl, prolog) = &library.body else { panic!("{library:?}") };
    assert_eq!((decl.prefix.as_str(), decl.uri.as_str(), prolog.declarations.len()), ("m", "urn:m", 1));
    let main = module("import module namespace n = 'urn:n' at 'a', 'B'; n:f()");
    let [Setup::Import(Import::Module(import))] = main.prolog().setup.as_slice() else { panic!("{main:?}") };
    assert_eq!(
        (import.prefix.as_deref(), import.uri.as_str(), import.at.clone()),
        (Some("n"), "urn:n", vec!["a".to_owned(), "B".to_owned()])
    );
    assert_eq!(module("1").prolog(), &Prolog::default());
}

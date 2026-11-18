//! Tests of the static typing [XQuery §5.2.3] [XQueryFS].

use super::super::ast::Occurrence;
use super::super::testing::collations;
use super::super::{Environment, Library, Query};
use super::{Formal, FormalKind, Passed, Typed};
use crate::error::Result;
use crate::xml::AtomicType;

/// `text` compiled with static typing in effect, the variables `variables`
/// passed of their formal types, and the context item of `context`'s.
fn typed(text: &str, context: Option<Formal>, variables: &[(&str, Formal)]) -> Result<Query> {
    let (library, collations) = (Library::builtin(), collations());
    let env = Environment {
        namespaces: Vec::new(),
        default_element: None,
        variables: variables.iter().map(|(n, _)| (*n).to_owned()).collect(),
        library: &library,
        collations: &collations,
        schemas: Vec::new(),
        importable: Vec::new(),
        modules: Vec::new(),
        linked: Vec::new(),
        static_typing: true,
        passed: Passed { context, variables: variables.iter().map(|(_, f)| f.clone()).collect(), item: None },
    };
    Query::compile(text, &env)
}

/// What compiling `text` statically typed gives: `ok`, or the XQuery
/// error's code its message names.
fn outcome(text: &str, context: Option<Formal>, variables: &[(&str, Formal)]) -> String {
    match typed(text, context, variables) {
        Ok(_) => "ok".to_owned(),
        Err(e) => e.msg.split_whitespace().find(|w| w.starts_with("err:")).map_or(e.msg.clone(), |w| w.trim_end_matches(':').to_owned()),
    }
}

/// `text` alone, of no context item and no variable.
fn alone(text: &str) -> String {
    outcome(text, None, &[])
}

/// The formal type of `kind` and `occurrence`.
fn formal(kind: FormalKind, occurrence: Occurrence) -> Formal {
    Formal { kind, occurrence }
}

/// XML(CONTENT(UNTYPED)) passed, `?` as a variable [§10.21 SR3d].
fn untyped_content() -> Formal {
    formal(FormalKind::Content(Typed::Untyped), Occurrence::Optional)
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 SR6] [XQuery §5.2.3]
/// [XQueryFS §4.1.1, §4.4, Appendix B.2] literals are of their types and
/// arithmetic of the operator mapping's: a pair it has none of is a type
/// error as the query is analysed, of no input.
#[test]
fn arithmetic_is_typed_as_analysed() {
    assert_eq!(alone("1 + 2.5 * 2"), "ok");
    assert_eq!(alone("1 + 'a'"), "err:XPTY0004");
    assert_eq!(alone("xs:date('2026-01-01') + xdt:dayTimeDuration('P1D')"), "ok");
    assert_eq!(alone("xs:date('2026-01-01') + 1"), "err:XPTY0004");
    assert_eq!(alone("-'a'"), "err:XPTY0004");
    assert_eq!(alone("(1, 2) + 1"), "err:XPTY0004", "an operand of more than one value");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.21 SR3, SR5d] [XQueryFS
/// §4.2.1, §8.2.2.1.3, §7.2.6, §7.1.1] a passed XML(CONTENT(UNTYPED)) is
/// an optional document of untyped elements; its child's untyped value is
/// a double in arithmetic, of one element; of any number of them, a type
/// error, though a value of one would evaluate.
#[test]
fn passed_values_are_of_their_formal_types() {
    let x = [("x", untyped_content())];
    assert_eq!(outcome("($x/a)[1] + 1", None, &x), "ok");
    assert_eq!(outcome("$x/a + 1", None, &x), "err:XPTY0004");
    assert_eq!(outcome("exactly-one($x/a) * 2", None, &x), "ok");
    assert_eq!(outcome("$x/a/b/@c = 'q'", None, &x), "ok");
    let number = [("n", formal(FormalKind::Atomic(AtomicType::Integer), Occurrence::One))];
    assert_eq!(outcome("$n + 1", None, &number), "ok");
    assert_eq!(outcome("$n/a", None, &number), "err:XPTY0019");
    assert_eq!(outcome("$n[a]", None, &number), "err:XPTY0020");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 SR6] [XQuery §2.3.1] [XQueryFS §4] an expression of the empty type
/// is err:XPST0005, but `()` and `fn:data(())`: an attribute's children, a
/// variable bound to `()`.
#[test]
fn the_empty_type_is_a_static_error() {
    let x = [("x", untyped_content())];
    assert_eq!(outcome("$x/a/@b/c", None, &x), "err:XPST0005");
    assert_eq!(outcome("$x/a/text()/d", None, &x), "err:XPST0005");
    assert_eq!(alone("()"), "ok");
    assert_eq!(alone("data(())"), "ok");
    assert_eq!(alone("let $e := () return $e"), "err:XPST0005");
    assert_eq!(alone("<a/>/@b"), "ok", "an element of xs:anyType may have any attribute");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 SR6] [XQuery Appendix C.1] [ISO/IEC 9075-14:2006 draft §6.18
/// SR5b] the context item's static type is none where none is passed, so
/// reading it is err:XPST0001; a passed one is exactly one item [§10.21
/// SR3a].
#[test]
fn the_context_item_is_typed() {
    assert_eq!(alone("."), "err:XPST0001");
    assert_eq!(alone("a"), "err:XPST0001");
    assert_eq!(alone("declare function local:f() { . }; 1"), "err:XPST0001");
    let doc = formal(FormalKind::Document(Typed::Untyped), Occurrence::Optional);
    assert_eq!(outcome("/a/b", Some(doc.clone()), &[]), "ok");
    assert_eq!(outcome("count(.) + position()", Some(doc), &[]), "ok");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 SR6] [XQueryFS §7.2.4, §4.10, §4.6] a condition's effective
/// boolean value is defined: of nodes, of no value, of one boolean,
/// string, untyped value or number; of two numbers not.
#[test]
fn conditions_have_effective_boolean_values() {
    assert_eq!(alone("if (1) then 1 else 2"), "ok");
    assert_eq!(alone("if (<a/>/b) then 1 else 2"), "ok");
    assert_eq!(alone("if ((1, 2)) then 1 else 2"), "err:XPTY0004");
    assert_eq!(alone("(1, 2) and true()"), "err:XPTY0004");
    assert_eq!(alone("boolean(xs:date('2026-01-01'))"), "err:XPTY0004");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 SR6] [XQueryFS §4.1.5, §8.5.1, §5.15] a call's arguments are
/// converted by its parameters' types and promoted to them; a declared
/// function's body is of its result type; a built-in function's
/// signature is [XQueryFO]'s.
#[test]
fn calls_are_typed_by_their_signatures() {
    assert_eq!(alone("upper-case('a')"), "ok");
    assert_eq!(alone("upper-case(1)"), "err:XPTY0004");
    assert_eq!(alone("substring('abc', 1)"), "ok", "an integer promoted to a double");
    assert_eq!(alone("string-length(('a', 'b'))"), "err:XPTY0004");
    assert_eq!(alone("declare function local:f($a as xs:string) as xs:integer { string-length($a) }; local:f('a')"), "ok");
    assert_eq!(alone("declare function local:f() as xs:integer { 'a' }; local:f()"), "err:XPTY0004");
    assert_eq!(alone("declare function local:f($a as xs:integer) { $a }; local:f('1')"), "err:XPTY0004");
    assert_eq!(alone("concat('a', 1, 'b')"), "ok");
    assert_eq!(alone("sum((1, 2.5))"), "ok");
    assert_eq!(alone("avg(('a', 'b'))"), "err:XPTY0004");
    assert_eq!(alone("round(1.5) + 1"), "ok");
    assert_eq!(alone("zero-or-one((1, 2)) + 1"), "ok");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 SR6] [XQueryFS §4.2, §7.1.4] [XQuery §3.2] a step of a path is of
/// nodes \[err:XPTY0019\], an axis step's context item a node
/// \[err:XPTY0020\], and a path's result nodes or atomic values alone
/// \[err:XPTY0018\].
#[test]
fn paths_are_typed() {
    assert_eq!(alone("(1, 2)/a"), "err:XPTY0019");
    assert_eq!(alone("<a><b/></a>/(b, 1)"), "err:XPTY0018");
    assert_eq!(alone("<a><b/></a>/b/string()"), "ok");
    assert_eq!(alone("<a><b/></a>//b/following-sibling::*/.."), "ok");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 SR6] [XQueryFS §4.8, §4.11, §4.12] variables declared of a type
/// are bound to values of it; casts take one atomic value, or none if
/// `?`; treat gives its type; typeswitch the union of its clauses'.
#[test]
fn bindings_and_sequence_types_are_typed() {
    assert_eq!(alone("for $i as xs:integer in (1, 2) return $i + 1"), "ok");
    assert_eq!(alone("for $i as xs:string in (1, 2) return $i"), "err:XPTY0004");
    assert_eq!(alone("let $s as xs:string* := ('a', 1) return $s"), "err:XPTY0004");
    assert_eq!(alone("every $i in (1, 2) satisfies $i"), "ok");
    assert_eq!(alone("(1, 2) cast as xs:string"), "err:XPTY0004");
    assert_eq!(alone("() cast as xs:string?"), "ok");
    assert_eq!(alone("((1, 'a')[1] treat as xs:integer) + 1"), "ok");
    assert_eq!(alone("typeswitch (1) case $i as xs:integer return $i + 1 default return 0"), "ok");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 SR6] [XQueryFS §4.7, §7.1.5] [XQuery §3.7.1.3, §3.13] an
/// element's content holds no attribute after another node
/// \[err:XQTY0024\]; a document's no attribute; validate is given one
/// element or document \[err:XQTY0030\].
#[test]
fn constructors_are_typed() {
    assert_eq!(alone("<a b='{1}'>{2, <c/>}</a>"), "ok");
    assert_eq!(alone("element a { <b/>, attribute c { 1 } }"), "err:XQTY0024");
    assert_eq!(alone("document { attribute c { 1 } }"), "err:XPTY0004");
    assert_eq!(alone("element { 1 } { 2 }"), "err:XPTY0004");
    assert_eq!(alone("validate lax { 1 }"), "err:XQTY0030");
    assert_eq!(alone("validate lax { <a/> }"), "ok");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 SR6] [XQuery §4.16] [EXTENSION] the option static-typing of
/// turborel's namespace puts the Static Typing Feature in effect for its
/// query, and none takes it out; a query of no such option is typed as
/// it is evaluated.
#[test]
fn the_option_puts_static_typing_in_effect() {
    let (library, collations) = (Library::builtin(), collations());
    let env = Environment {
        namespaces: Vec::new(),
        default_element: None,
        variables: Vec::new(),
        library: &library,
        collations: &collations,
        schemas: Vec::new(),
        importable: Vec::new(),
        modules: Vec::new(),
        linked: Vec::new(),
        static_typing: false,
        passed: Passed::default(),
    };
    let on = "declare namespace t = 'urn:turborel:option'; declare option t:static-typing 'on'; ";
    assert!(Query::compile("if ((1, 2)) then 1 else 2", &env).is_ok());
    assert!(Query::compile(&format!("{on}if ((1, 2)) then 1 else 2"), &env).is_err());
    let off = "declare namespace t = 'urn:turborel:option'; declare option t:static-typing 'off'; ";
    assert!(Query::compile(&format!("{off}if ((1, 2)) then 1 else 2"), &env).is_ok());
    assert_eq!(alone(&format!("{off}if ((1, 2)) then 1 else 2")), "err:XPTY0004", "off suppresses no static error");
    assert!(Query::compile("declare namespace t = 'urn:turborel:option'; declare option t:static-typing 'yes'; 1", &env).is_err());
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 SR6] [XQueryFS §8.2.2.1, §8.2.3.1] [XQuery §5.2.4] every axis is
/// typed: the descendants and ancestors by closure, the siblings and the
/// following and preceding by their normalizations; kind tests select
/// what they admit, of a type neither is a subtype of what both admit,
/// and nothing of what none admits.
#[test]
fn axes_and_kind_tests_are_typed() {
    let doc = || Some(formal(FormalKind::Document(Typed::Untyped), Occurrence::Optional));
    for q in [
        "//b/ancestor::*/@c",
        "a/descendant-or-self::node()/text()",
        "a/b/following-sibling::c/preceding-sibling::*",
        "a/following::d/preceding::e/ancestor-or-self::f",
        "a/@b/..",
        "a/self::a/element(a)",
        "a/element(*, xdt:untyped)",
        "a/attribute::attribute(b)",
        "a/processing-instruction(p)",
        "a/comment()",
        "a/descendant::*[1]",
        "a/b[last()]",
        ". instance of document-node(element(a))",
        "(a | a/b) intersect a",
        "a except a/b",
        "a[1] is (a/b)[1]",
        "zero-or-one(a/b) << exactly-one(a/c)",
    ] {
        assert_eq!(outcome(q, doc(), &[]), "ok", "{q}");
    }
    assert_eq!(outcome("a/element(a, xs:integer)", doc(), &[]), "err:XPST0005", "an untyped element is no integer's");
    assert_eq!(outcome("a/text()/@b", doc(), &[]), "err:XPST0005");
    assert_eq!(outcome("/a/@b/..", doc(), &[]), "ok", "the root of a document is of its type");
    assert_eq!(outcome("(1 | a)", doc(), &[]), "err:XPTY0004");
    assert_eq!(outcome("(a, a) is a", doc(), &[]), "err:XPTY0004");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 SR6] [XQueryFS §4.5, Appendix B.2] [XQuery Appendix B.1, B.2]
/// comparisons: values of a pair the operator mapping compares, an
/// untyped value converted to a string, or of a general comparison to
/// the other's type; a pair it does not compare a type error.
#[test]
fn comparisons_are_typed() {
    let doc = || Some(formal(FormalKind::Content(Typed::Untyped), Occurrence::One));
    for q in [
        "1 eq 1.0",
        "'a' lt xs:anyURI('b')",
        "xs:date('2026-01-01') le xs:date('2026-01-02')",
        "xdt:yearMonthDuration('P1Y') eq xs:duration('P1Y')",
        "xs:gYear('2026') ne xs:gYear('2025')",
        "a = 1",
        "a = 'x'",
        "a = a",
        "(a)[1] eq 'x'",
        "xs:hexBinary('0A') eq xs:hexBinary('0a')",
    ] {
        assert_eq!(outcome(q, doc(), &[]), "ok", "{q}");
    }
    for q in ["1 eq 'a'", "xs:gYear('2026') lt xs:gYear('2025')", "a eq 'x'", "true() = 1", "xs:QName('a') lt xs:QName('b')"] {
        assert_eq!(outcome(q, doc(), &[]), "err:XPTY0004", "{q}");
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 SR6] [XQuery Appendix B.2] [XQueryFS §4.4] arithmetic of dates,
/// times and durations, numbers promoted to the wider type, `idiv` an
/// integer, `div` of integers a decimal, unary minus of a number.
#[test]
fn arithmetic_of_each_type_is_typed() {
    for q in [
        "xs:dateTime('2026-01-01T00:00:00') - xs:dateTime('2025-01-01T00:00:00')",
        "xs:time('10:00:00') + xdt:dayTimeDuration('PT1H')",
        "xdt:dayTimeDuration('PT1H') * 2",
        "2 * xdt:yearMonthDuration('P1Y')",
        "xdt:yearMonthDuration('P2Y') div xdt:yearMonthDuration('P1Y')",
        "xdt:dayTimeDuration('PT2H') div 2.5",
        "7 idiv 2.5",
        "(7 div 2) instance of xs:decimal",
        "7 mod 2e0",
        "-xs:float('1.5')",
        "+1",
        "1 to 3",
    ] {
        assert_eq!(alone(q), "ok", "{q}");
    }
    assert_eq!(alone("1 to 'a'"), "err:XPTY0004");
    assert_eq!(alone("() + 1"), "err:XPST0005", "of an empty operand, the empty type");
    assert_eq!(alone("xs:integer('1') castable as xs:date"), "err:XPTY0004", "a constructor function's value is optional");
    assert_eq!(alone("xs:time('10:00:00') - xs:date('2026-01-01')"), "err:XPTY0004");
    assert_eq!(alone("xdt:yearMonthDuration('P1Y') mod 2"), "err:XPTY0004");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 SR6] [XQueryFS §7.2] the functions of typing rules of their own:
/// rounding, the aggregates, fn:remove, fn:reverse, fn:subsequence,
/// fn:insert-before, fn:distinct-values, fn:unordered, fn:error, fn:doc,
/// fn:collection, fn:data and the cardinality functions.
#[test]
fn functions_of_their_own_rules_are_typed() {
    let x = [("x", untyped_content())];
    for q in [
        "round-half-to-even(1.25, 1) + 1",
        "ceiling(xs:float('1.5')) instance of xs:float",
        "floor(($x//a)[1]) + 1",
        "abs(())",
        "min((1, 2.5)) + 1",
        "max(('a', 'b'))",
        "max(('a', 'b'), 'http://www.w3.org/2005/xpath-functions/collation/codepoint')",
        "avg((1, 2)) + 1",
        "sum((1, 2)) + 1",
        "sum((), 0.5)",
        "sum($x//a)",
        "remove((1, 2), 1)",
        "reverse((1, 'a'))",
        "subsequence((1, 2, 3), 2)",
        "subsequence((1, 2, 3), 1, 1) + 1",
        "subsequence(($x//a), 2, 1)",
        "insert-before((1, 2), 1, 'a')",
        "distinct-values((1, 1))",
        "unordered((1, 2))",
        "if (1) then 1 else error()",
        "error(QName('http://www.w3.org/2005/xqt-errors', 'FOER0000'), 'no')",
        "count(doc('a')/b)",
        "collection('a')",
        "one-or-more((1, 2))",
        "boolean(())",
        "not((1, 2))",
        "concat('a', 'b', 'c', 'd')",
        "string-join(('a', 'b'), ',')",
        "$x/a/name(.)",
    ] {
        assert_eq!(outcome(q, None, &x), "ok", "{q}");
    }
    for q in ["round(('a'))", "round((1, 2))", "max((1, xs:date('2026-01-01')))", "sum(('a'))", "remove((1, 2), 'a')", "boolean((1, 2))"] {
        assert_eq!(outcome(q, None, &x), "err:XPTY0004", "{q}");
    }
    assert_eq!(alone("position()"), "err:XPST0001");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 SR6] [XQueryFS §4.7.3, §7.1.5] [XQuery §3.7] the computed
/// constructors: a computed name of one QName, string or untyped value;
/// a text, comment or processing instruction of any content; an
/// attribute of an untyped value; a document of no attribute; in
/// construction mode strip an element of `xdt:untyped`.
#[test]
fn computed_constructors_are_typed() {
    for q in [
        "element {'a'} {attribute b {1}, text {'c'}, comment {'d'}, processing-instruction e {'f'}}",
        "attribute {xs:QName('a')} {1, 2}",
        "processing-instruction {'p'} {1}",
        "document {<a/>, 'b'}",
        "declare construction strip; <a/> instance of element(*, xdt:untyped)",
        "<a>{<!--c-->, <?p q?>}</a>",
        "text {()}",
    ] {
        assert_eq!(alone(q), "ok", "{q}");
    }
    assert_eq!(alone("processing-instruction {1} {2}"), "err:XPTY0004");
    assert_eq!(alone("<a b='{1}{2}'/>/@b + 1"), "err:XPTY0004", "an element of xs:anyType has attributes of any type");
    assert_eq!(alone("attribute {(1, 2)} {3}"), "err:XPTY0004");
    assert_eq!(alone("document {<a/>, attribute b {1}}"), "err:XQTY0024");
    assert_eq!(alone("element a {document {<b/>}}"), "err:XPTY0004", "no document node in an element's content [XQueryFS §7.1.5]");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 SR6] [XQueryFS §5.14, §5.15, §4.8.4, §4.12.2] a prolog's
/// variables of their declared or initializing types, external ones of
/// their declared types; a function's body of no focus; order by keys
/// compared by gt; typeswitch's default variable of the operand's type.
#[test]
fn prologs_and_clauses_are_typed() {
    for q in [
        "declare variable $v as xs:integer := 1; $v + 1",
        "declare variable $v := 'a'; upper-case($v)",
        "declare variable $e as xs:string external; 1",
        "declare variable $e external; 1",
        "for $i at $p in ('a', 'b') order by $i descending, $p return $p + 1",
        "typeswitch ((1, 'a')) case $i as xs:integer return $i + 1 default $d return count($d)",
        "some $i as xs:integer in (1, 2) satisfies $i eq 1",
        "for $i in (1, 2) where $i eq 1 return $i",
        "1 castable as xs:date",
        "validate strict { <a/> }",
        "ordered { 1 }",
    ] {
        assert_eq!(alone(q), "ok", "{q}");
    }
    assert_eq!(alone("declare variable $v as xs:integer := 'a'; $v"), "err:XPTY0004");
    assert_eq!(alone("declare construction strip; for $i in (<a/>, 'x') order by $i return $i"), "ok", "an untyped value orders");
    assert_eq!(alone("for $i in (<a/>, 1) order by $i return $i"), "err:XPTY0004", "an element of xs:anyType's typed value is of any type");
    assert_eq!(alone("for $i in (1, 'a') order by $i return $i"), "err:XPTY0004");
    assert_eq!(alone("some $i as xs:string in (1, 2) satisfies true()"), "err:XPTY0004");
    assert_eq!(alone("declare function local:f() { a }; local:f()"), "err:XPST0001");
}

/// A thread's default stack, 2 MiB.
const DEFAULT_STACK: usize = 2 * 1024 * 1024;

/// Verifies: [DESIGN] RULES.md R-CF-6: the typing of the deepest query
/// the parser takes, of each shape, runs within half of a thread's default
/// stack, refused at worst, never overflowing it.
#[test]
fn deep_queries_are_typed_within_the_stack() {
    for (open, close) in super::super::tests::nesting_shapes() {
        let deepest = (1..=super::super::MAX_NESTING)
            .map(|n| format!("{}1{}", open.repeat(n), close.repeat(n)))
            .take_while(|q| super::super::parse(q, crate::xml::Version::V11).is_ok())
            .last()
            .unwrap();
        let analysed = std::thread::Builder::new().name("xquery-typing".to_owned()).stack_size(DEFAULT_STACK).spawn(move || {
            let context = formal(FormalKind::Content(Typed::Untyped), Occurrence::One);
            let _ = typed(&deepest, Some(context), &[]);
        });
        analysed.unwrap().join().unwrap();
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 SR6] [XQueryFS §8.3.2, §8.4, §8.5.1] subtyping of each shape the
/// rules compare with: factored types by item types and occurrence; a
/// sequence of attributes then other nodes, an attribute after another
/// node refused; an interleaving of parts, each part's items counted; a
/// choice alternative by alternative; and the empty and none types.
#[test]
fn subtyping_decides_each_shape() {
    use super::sequence::{any_attribute, any_element};
    use super::subtype::subtype;
    use super::types::{Item, Occ, Type};
    let item = |i: Item| Type::Item(i);
    let star = |t: Type| Type::occur(t, Occ::Star);
    let content = Type::Seq(vec![star(item(any_attribute())), star(Type::of_prime(vec![any_element(), Item::Text], Occ::One))]);
    let good = Type::Seq(vec![item(any_attribute()), item(Item::Text), item(any_element())]);
    let bad = Type::Seq(vec![item(Item::Text), item(any_attribute())]);
    assert!(subtype(&good, &content).unwrap());
    assert!(!subtype(&bad, &content).unwrap(), "an attribute after a text node");
    assert!(!subtype(&item(Item::Comment), &content).unwrap());
    let interleaved = Type::All(vec![item(any_element()), star(item(Item::Comment))]);
    assert!(subtype(&Type::Seq(vec![item(Item::Comment), item(any_element())]), &interleaved).unwrap());
    assert!(!subtype(&Type::Seq(vec![item(any_element()), item(any_element())]), &interleaved).unwrap(), "two elements");
    assert!(!subtype(&item(Item::Text), &interleaved).unwrap());
    let either = Type::Choice(vec![Type::Seq(vec![item(Item::Text), item(Item::Text)]), item(Item::Comment)]);
    assert!(subtype(&item(Item::Comment), &either).unwrap());
    assert!(
        subtype(
            &Type::Choice(vec![item(Item::Text), item(Item::Comment)]),
            &star(Type::of_prime(vec![Item::Text, Item::Comment], Occ::One))
        )
        .unwrap()
    );
    assert!(subtype(&Type::Empty, &Type::occur(item(Item::Text), Occ::Optional)).unwrap());
    assert!(subtype(&Type::None, &Type::Empty).unwrap());
    assert!(!subtype(&Type::Empty, &Type::None).unwrap());
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 SR6] [XQueryFS §8.5.1] type promotion: an integer to a double, an
/// untyped value to any atomic type, never a double to an integer, nor
/// more items than the type allows.
#[test]
fn promotion_is_typed() {
    use super::subtype::can_promote;
    use super::types::Type;
    let integer = Type::atomic(AtomicType::Integer);
    let untyped = Type::atomic(AtomicType::UntypedAtomic);
    let double = Type::atomic(AtomicType::Double);
    assert!(can_promote(&integer, &double).unwrap());
    assert!(can_promote(&untyped, &integer).unwrap());
    assert!(!can_promote(&double, &integer).unwrap());
    assert!(!can_promote(&Type::seq(vec![integer.clone(), integer]), &double).unwrap());
}

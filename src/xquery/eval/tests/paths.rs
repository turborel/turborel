//! Tests of paths, axes and predicates [XQuery §3.2, §3.3.2].

use super::super::super::testing::{code_of, compile_with, evaluate, run, shown};
use crate::xml::{Item, Node, Sequence, Syntax, Whitespace, parse};

/// A document to walk: a root of three children, the second with two of
/// its own and an attribute.
const DOC: &str = "let $d := document { <r><a i='1'>x</a><b i='2' j='3'><c>y</c><c>z</c></b><a i='3'/></r> } return";

/// The value of `path` after `DOC`.
fn of(path: &str) -> String {
    run(&format!("{DOC} ({path})"))
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2] [XQuery §3.2,
/// §3.2.4] steps of the child, attribute and descendant axes in their
/// abbreviated syntax, each step's nodes in document order each once; a
/// leading `/` the document node, `//` its descendants.
#[test]
fn abbreviated_paths() {
    assert_eq!(of("$d/r/a/@i/string()"), "1 3");
    assert_eq!(of("for $i in $d/r/a/@i return string($i)"), "1 3");
    assert_eq!(of("count($d//c), string($d/r/b/c[2]), count($d/r/*), count($d//@i)"), "2 z 3 3");
    assert_eq!(of("count($d/r/a | $d/r/a), count(($d/r/a, $d/r/a)/.)"), "2 2");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2] [XQuery §3.2.1.1]
/// every axis, forward and reverse, in document order, a reverse axis's
/// positions counted from the context node [XQuery §3.2.2].
#[test]
fn every_axis() {
    let c = "$d/r/b/c[1]";
    assert_eq!(of(&format!("count({c}/ancestor::*), count({c}/ancestor-or-self::node()), count({c}/parent::b)")), "2 4 1");
    assert_eq!(of(&format!("string({c}/following-sibling::c), count({c}/preceding-sibling::*), count({c}/following::*)")), "z 0 2");
    assert_eq!(of(&format!("count({c}/preceding::*), string({c}/preceding::a[1]), string(({c}/preceding::a)[1])")), "1 x x");
    assert_eq!(
        of("count($d/r/b/descendant::*), count($d/r/b/descendant-or-self::*), count($d/r/b/self::b), count($d/r/b/self::a)"),
        "2 3 1 0"
    );
    assert_eq!(of("string(($d//c)[2]/ancestor::*[1]/@i), count($d/r/b/attribute::*), count($d/r/b/@i/following-sibling::*)"), "2 2 0");
    assert_eq!(of("count($d/r/b/@j/following::*), count($d/r/b/@j/preceding::*), count($d/r/b/@i/parent::b)"), "3 1 1");
    assert_eq!(of("count($d/r/node()), count($d/r/a[1]/text()), count($d/r/b/c/..)"), "3 1 1");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2] [XQuery §3.2.1.2]
/// name tests of a namespace, any name, any local name and any
/// namespace, an unprefixed one of an element in the default element
/// namespace; kind tests.
#[test]
fn node_tests() {
    let q = "declare namespace p = 'u'; let $e := <r xmlns:p='u' xmlns:q='v'><p:a/><q:a/><a/><!--c--><?t x?>text</r> return";
    assert_eq!(run(&format!("{q} (count($e/p:a), count($e/*:a), count($e/p:*), count($e/a), count($e/*))")), "1 3 1 1 3");
    assert_eq!(
        run(&format!("{q} (count($e/comment()), count($e/processing-instruction(t)), count($e/text()), count($e/element(a)))")),
        "1 1 1 1"
    );
    assert_eq!(run("declare default element namespace 'u'; count(<r xmlns='u'><a/></r>/a)"), "1");
    assert_eq!(code_of("<a/>/q:b"), "err:XPST0008");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2] [XQuery §3.2.2,
/// §3.3.2] a predicate of a number keeps the item at that position, else
/// by its effective boolean value; predicates in turn; a filter
/// expression's positions those of its primary expression's order.
#[test]
fn predicates() {
    assert_eq!(run("(1 to 10)[. mod 3 = 0], (1 to 10)[3], (1 to 10)[3.5], (1 to 10)[xs:double(2)]"), "3 6 9 3 2");
    assert_eq!(run("(1 to 10)[. gt 2][2], (1 to 5)[position() gt 3]"), "4 4 5");
    assert_eq!(of("string($d/r/a[@i = '3']/@i), count($d/r/*[c])"), "3 1");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQuery §3.2,
/// §3.2.1] a step's context item no node err:XPTY0020; a step other than
/// the last of atomic values err:XPTY0019; a last step of nodes and atomic
/// values err:XPTY0018; a leading `/` of a tree whose root is no document
/// node err:XPDY0050; the last step may give atomic values.
#[test]
fn path_errors() {
    assert_eq!(code_of("(1, 2)/a"), "err:XPTY0019");
    assert_eq!(code_of("<a/>/1/x"), "err:XPTY0019");
    assert_eq!(code_of("<a><b/><c/></a>/*/(if (self::b) then . else 1)"), "err:XPTY0018");
    assert_eq!(code_of("let $x := 1 return $x[child::a]"), "err:XPTY0020");
    assert_eq!(code_of("<a/>/(/)"), "err:XPDY0050");
    assert_eq!(run("<a><b>1</b><b>2</b></a>/b/(. + 1)"), "2 3");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR1a ii] [XQuery §2.4.1]
/// a document passed as the context item: `/` is its root, and the
/// nodes of two trees are in one stable order, all of one tree before
/// all of the other.
#[test]
fn context_documents_and_order_across_trees() {
    let tree = parse("<r><a>1</a><a>2</a></r>", Syntax::Document, Whitespace::Preserve).unwrap();
    let doc = Item::Node(Node::root(tree));
    let query = compile_with("for $a in //a return string($a), count(/r)", &[]).unwrap();
    assert_eq!(shown(&evaluate(&query, Some(doc.clone()), Vec::new()).unwrap()), "1 2 1");
    let order =
        compile_with("let $x := (/r/a[2], <z/>, /r/a[1]) return (($x | ())[1] is /r/a[1]) or (($x | ())[1] instance of element(z))", &[])
            .unwrap();
    assert_eq!(shown(&evaluate(&order, Some(doc), Vec::<Sequence>::new()).unwrap()), "true");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR1a ii, GR3] [XQuery §3.2,
/// §3.2.1, §3.2.1.1] a leading `/` and an axis step of no context item
/// err:XPDY0002, `/` of an atomic context item err:XPTY0020; the
/// sibling and following axes of a node with none empty.
#[test]
fn steps_at_the_edges() {
    assert_eq!(code_of("/a"), "err:XPDY0002");
    assert_eq!(code_of("child::a"), "err:XPDY0002");
    let slash = compile_with("/", &[]).unwrap();
    let one = Item::Atomic(crate::xml::Atomic::new(crate::xml::AtomicType::Integer, "1"));
    assert!(evaluate(&slash, Some(one), Vec::new()).unwrap_err().msg.contains("err:XPTY0020"));
    let edges = "count(<a/>/preceding-sibling::node()), count(<a/>/following::node()), count(<a><b/></a>/b/following::node())";
    assert_eq!(run(edges), "0 0 0");
}

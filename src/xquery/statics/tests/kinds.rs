//! Tests of the static typing of axes, kind tests, sequence types, typed
//! values, the prolog and the context items of each formal type, of
//! untyped documents and of no schema.

use super::super::{FormalKind, Typed};
use super::{alone, formal, outcome, untyped_content};
use crate::xml::AtomicType;
use crate::xquery::ast::Occurrence;

/// Each of `cases`, a query and its outcome, compiled with the context
/// item of the formal type `kind`, one.
fn each_of(kind: &FormalKind, cases: &[(&str, &str)]) {
    let context = formal(kind.clone(), Occurrence::One);
    for (query, expected) in cases {
        assert_eq!(outcome(query, Some(context.clone()), &[]), *expected, "{query}");
    }
}

/// Each of `cases` compiled alone.
fn each_alone(cases: &[(&str, &str)]) {
    for (query, expected) in cases {
        assert_eq!(alone(query), *expected, "{query}");
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 SR6] [XQueryFS §8.2.2.1,
/// §8.2.3.1.2, §4.2.1] every axis of an untyped document is typed: the
/// following and preceding axes by the parents' children and the
/// ancestors' siblings' descendants; an attribute's parent an element; an
/// attribute and a text node have no children nor attributes, so a step
/// from them is err:XPST0005; a kind test keeps the item types it admits.
#[test]
fn every_axis_and_kind_test_of_untyped_documents() {
    each_of(
        &FormalKind::Document(Typed::Untyped),
        &[
            ("./a/following::b", "ok"),
            ("./a/preceding::node()", "ok"),
            ("./a/ancestor::*", "ok"),
            ("./a/ancestor-or-self::a", "ok"),
            ("./a/@b/parent::a", "ok"),
            ("./a/following-sibling::text()", "ok"),
            ("./a/preceding-sibling::comment()", "ok"),
            ("./a/self::a", "ok"),
            ("./a/descendant::processing-instruction()", "ok"),
            ("./a/descendant-or-self::processing-instruction(x)", "ok"),
            ("./a/attribute::b/child::c", "err:XPST0005"),
            ("./a/@b/@c", "err:XPST0005"),
            ("./a/text()/a", "err:XPST0005"),
            (". instance of document-node(element(a))", "ok"),
            ("./a instance of element(a)*", "ok"),
            ("./a/self::document-node()", "err:XPST0005"),
            ("./a/ancestor::document-node(element(a))", "ok"),
            ("./a/ancestor::document-node()", "ok"),
            ("./a/self::element(a, xdt:untyped)", "ok"),
            ("./a/@b/self::attribute(b)", "ok"),
            ("./a/self::attribute()", "err:XPST0005"),
            ("./a/self::text()", "err:XPST0005"),
            ("./comment()/self::comment()", "ok"),
            ("./a/@*/data(.) + 1", "err:XPTY0004"),
            ("./a/element()/attribute(*)", "ok"),
            ("./a/node()/self::processing-instruction(p)", "ok"),
        ],
    );
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 SR6, §10.21 SR5g]
/// [XQueryFS §4.2.1, §7.2.6, §8.1.9] a context item of XML(SEQUENCE) may
/// be an atomic value, so a path from it is err:XPTY0019; one of
/// XML(CONTENT(ANY)) a document of `xs:anyType`'s content, of any
/// children and attributes, its elements' typed values any number of
/// atomic values.
#[test]
fn context_items_of_sequence_and_any_content() {
    each_of(&FormalKind::Sequence, &[("./a", "err:XPTY0019"), (". + 1", "err:XPTY0004"), ("count(./node())", "err:XPTY0019")]);
    each_of(
        &FormalKind::Content(Typed::Any),
        &[
            ("./a", "ok"),
            (". + 1", "ok"),
            ("data(./a)", "ok"),
            ("./a/b + 1", "err:XPTY0004"),
            ("data(exactly-one(./a))", "ok"),
            ("./a/@c", "ok"),
            ("count(./node())", "ok"),
        ],
    );
    let s = [("s", formal(FormalKind::Atomic(AtomicType::String), Occurrence::Optional))];
    assert_eq!(outcome("$s + 1", None, &s), "err:XPTY0004");
    assert_eq!(outcome("upper-case($s)", None, &s), "ok");
    assert_eq!(outcome("root($x)/a", None, &[("x", untyped_content())]), "ok");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 SR6] [XQueryFS §7.2.6,
/// §7.2.4, §4.2.1, §4.7.3.4] typed values of each node kind, a comment's
/// and a processing instruction's strings, a text node's and a
/// document's untyped; the effective boolean value of no value, one
/// string or number, never of nodes and a number, nor of `xs:anyURI`
/// [7.2.4]; a step from an atomic value err:XPTY0019.
#[test]
fn typed_values_and_effective_boolean_values() {
    each_alone(&[
        ("1/a", "err:XPTY0019"),
        ("(1, <a/>)/b", "err:XPTY0019"),
        ("data(comment {'x'})", "ok"),
        ("data(processing-instruction p {'x'}) eq 'x'", "ok"),
        ("data(text {'x'}) + 1", "ok"),
        ("data(document {<a/>}) + 1", "ok"),
        ("if ((<a/>, 1)) then 1 else 2", "err:XPTY0004"),
        ("if (xdt:untypedAtomic('a')) then 1 else 2", "ok"),
        ("if (1.5e0) then 1 else 2", "ok"),
        ("if ('a') then 1 else 2", "ok"),
        ("if (()) then 1 else 2", "ok"),
        ("if (xs:anyURI('a')) then 1 else 2", "err:XPTY0004"),
    ]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 SR6] [XQueryFS §3.5.4,
/// §4.8.3, §4.7.3.4] each sequence type is normalized: `empty-sequence()`,
/// occurrences, processing instructions of a name or none, documents of
/// an element test or none, attributes of a name, items and nodes; a
/// value bound to a variable of it is a subtype of it or err:XPTY0004, a
/// text constructor's value optional.
#[test]
fn sequence_types_of_each_kind() {
    each_alone(&[
        ("() instance of empty-sequence()", "ok"),
        ("(1, 2) instance of xs:integer+", "ok"),
        ("let $x as xs:integer+ := (1, 2) return $x", "ok"),
        ("let $x as xs:integer+ := () return $x", "err:XPTY0004"),
        ("let $x as empty-sequence() := () return 1", "ok"),
        ("let $x as processing-instruction() := processing-instruction p {''} return $x", "ok"),
        ("let $x as processing-instruction(p) := processing-instruction p {''} return $x", "ok"),
        ("let $d as document-node() := document {<a/>} return $d", "ok"),
        ("let $d as document-node(element(a)) := document {<a/>} return $d", "ok"),
        ("let $d as document-node(element(a)) := document {<b/>} return $d", "err:XPTY0004"),
        ("let $d as document-node(element(a)) := document {<a/>, <a/>} return $d", "err:XPTY0004"),
        ("let $n as node() := text {'a'} return $n", "err:XPTY0004"),
        ("let $n as node()? := text {'a'} return $n", "ok"),
        ("let $i as item() := 1 return $i", "ok"),
        ("let $a as attribute(b) := attribute b {1} return $a", "ok"),
        ("let $a as attribute(b) := attribute c {1} return $a", "err:XPTY0004"),
        ("let $e as element(*, xs:anyType) := <a/> return $e", "ok"),
        ("let $c as comment() := comment {'a'} return $c", "ok"),
    ]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 SR6] [XQueryFS §5.14,
/// §5.15] a prolog's variables are of their declared types, an
/// initializing value a subtype of it, of none its value's, external
/// ones their declared types; a function's body is a subtype of its
/// result type, called or not, recursive or not; one of no declared
/// types takes and gives `item()*`.
#[test]
fn prolog_variables_and_functions() {
    each_alone(&[
        ("declare variable $v as xs:integer := 1; $v + 1", "ok"),
        ("declare variable $v as xs:string := 1; $v", "err:XPTY0004"),
        ("declare variable $v := 1; $v + 1", "ok"),
        ("declare variable $v external; $v", "ok"),
        ("declare variable $v as xs:integer external; $v + 1", "ok"),
        ("declare function local:f($x as xs:integer) as xs:string { $x }; local:f(1)", "err:XPTY0004"),
        ("declare function local:f($x as xs:integer) as xs:integer { $x + 1 }; local:f(1)", "ok"),
        ("declare function local:g() as xs:string { 1 }; 2", "err:XPTY0004"),
        ("declare function local:f($n as xs:integer) as xs:integer { if ($n = 0) then 0 else local:f($n - 1) }; local:f(3)", "ok"),
        ("declare function local:h($n) { $n }; local:h(1) + 1", "err:XPTY0004"),
    ]);
}

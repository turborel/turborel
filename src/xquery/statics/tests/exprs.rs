//! Tests of the static typing of each kind of expression, alone.

use super::alone;

/// Each of `cases`, a query and its outcome, compiled alone.
fn each(cases: &[(&str, &str)]) {
    for (query, expected) in cases {
        assert_eq!(alone(query), *expected, "{query}");
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 SR6] [XQuery §2.3.1]
/// [XQueryFS §4] an expression of each kind whose type is empty is
/// err:XPST0005: a filter, a union, an intersection, a sequence, a FLWOR,
/// a conditional, a typeswitch, a treat and a `let`'s value; a call of a
/// function declared `empty-sequence()` is not.
#[test]
fn the_empty_type_of_each_kind_of_expression() {
    each(&[
        ("(<a/>/@b/c)[1]", "err:XPST0005"),
        ("(<a/>/@b/c | <a/>/@b/d)", "err:XPST0005"),
        ("(<a/>/@b/c intersect <a/>)", "err:XPST0005"),
        ("(<a/>/@b/c, <a/>/@b/d)", "err:XPST0005"),
        ("for $i in <a/>/@b/c return 1", "err:XPST0005"),
        ("if (1) then <a/>/@b/c else <a/>/@b/d", "err:XPST0005"),
        ("typeswitch (1) case xs:string return <a/>/@b/c default return <a/>/@b/d", "err:XPST0005"),
        ("<a/>/@b/c treat as empty-sequence()", "err:XPST0005"),
        ("let $x := <a/>/@b/c return 1", "err:XPST0005"),
        ("declare function local:e() as empty-sequence() { () }; local:e()", "ok"),
    ]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 SR6] [XQueryFS §4.5,
/// Appendix B.2] [XQuery §3.5] comparisons: node comparisons of nodes
/// alone; value comparisons of one value each, an untyped one a string;
/// general comparisons of each pair of values, by the operator mapping,
/// so of `xs:anyAtomicType` none.
#[test]
fn comparisons_of_each_kind() {
    each(&[
        ("<a/> is <b/>", "ok"),
        ("<a/> << <b/>", "ok"),
        ("1 is <a/>", "err:XPTY0004"),
        ("<a>1</a>/text() eq 'x'", "err:XPTY0004"),
        ("exactly-one(<a>1</a>/text()) eq 'x'", "ok"),
        ("<a>1</a> eq 1", "err:XPTY0004"),
        ("1 eq 'a'", "err:XPTY0004"),
        ("1 = 'a'", "err:XPTY0004"),
        ("<a>1</a> = 1", "err:XPTY0004"),
        ("(1, 2) = (1, 'a')", "err:XPTY0004"),
        ("(1, 2) = (3, 4.5)", "ok"),
    ]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 SR6] [XQueryFS §4.3,
/// §4.4, §4.11, §4.12, §4.13] ranges of one integer each; unary and
/// binary arithmetic of numbers, durations and dates; unions, intersect
/// and except of nodes alone; quantified expressions, typeswitch cases
/// and their variables; cast of one value, or of none to an optional
/// type; castable; treat.
#[test]
fn operators_of_each_kind() {
    each(&[
        ("1 to 3", "ok"),
        ("(1, 2) to 3", "err:XPTY0004"),
        ("'a' to 3", "err:XPTY0004"),
        ("-<a>1</a>", "err:XPTY0004"),
        ("+'a'", "err:XPTY0004"),
        ("1 idiv 2 + 1 mod 2 + 1 div 2 - 2", "ok"),
        ("xs:date('2026-01-01') - xs:date('2026-01-02')", "ok"),
        ("<a/> + 1", "err:XPTY0004"),
        ("(1) | (2)", "err:XPTY0004"),
        ("<a/> except <b/>", "ok"),
        ("(<a/>, 1) intersect <b/>", "err:XPTY0004"),
        ("some $x in (1, 2) satisfies $x", "ok"),
        ("every $x in (<a/>, <b/>) satisfies $x/c", "ok"),
        ("some $x as xs:string in (1, 2) satisfies true()", "err:XPTY0004"),
        ("typeswitch (1) case $i as xs:integer return $i + 1 case xs:string return 2 default $d return 3", "ok"),
        ("'1' cast as xs:integer", "ok"),
        ("() cast as xs:integer", "err:XPTY0004"),
        ("() cast as xs:integer?", "ok"),
        ("(1, 2) cast as xs:integer", "err:XPTY0004"),
        ("'1' castable as xs:integer", "ok"),
        ("(1, 2) treat as xs:integer+", "ok"),
    ]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 SR6] [XQueryFS §4.2,
/// §4.8, §4.13, §7.1.4] paths whose last step is of nodes or of atomic
/// values, not both (err:XPTY0018); predicates numeric or of an
/// effective boolean value; FLWOR clauses: positional variables,
/// declared types, `where` and `order by` keys compared by `gt` per item;
/// validate of one element or document (err:XQTY0030).
#[test]
fn paths_flwors_and_validate() {
    each(&[
        ("<a/>/(b, 1)", "err:XPTY0018"),
        ("<a/>/(1, 2)", "ok"),
        ("(1, 2, 3)[2] + (1, 2, 3)[last()]", "ok"),
        ("(1, 2, 3)[position() = 1]", "ok"),
        ("(1, 2, 3)[. > 1]", "ok"),
        ("(1, 2, 3)[(1, 2)]", "err:XPTY0004"),
        ("(1, 2, 3)['a']", "ok"),
        ("for $i in (3, 1) order by $i return $i", "ok"),
        ("for $i in (3, 1) order by (1, 2) return $i", "ok"),
        ("for $i in (3, 1) order by <a/> return $i", "err:XPTY0004"),
        ("for $i at $p in (1, 2) return $p + 1", "ok"),
        ("for $i as xs:string in (1, 2) return $i", "err:XPTY0004"),
        ("for $i in (1, 2) let $j := $i where $j > 1 return $j", "ok"),
        ("for $i in (1, 2) where (1, 2) return $i", "err:XPTY0004"),
        ("validate { <a/> }", "ok"),
        ("validate { document { <a/> } }", "ok"),
        ("validate { 1 }", "err:XQTY0030"),
        ("validate { (<a/>, <b/>) }", "err:XQTY0030"),
        ("ordered { 1 } + unordered { 1 }", "ok"),
        ("declare namespace t = \"urn:t\"; (# t:x #) { 1 }", "ok"),
    ]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 SR6] [XQueryFS §4.7]
/// [XQuery §3.7] constructors: a computed name of one string or QName; a
/// document of no attribute; attributes before an element's other
/// content (err:XQTY0024); direct content of every kind.
#[test]
fn constructors_of_each_kind() {
    each(&[
        ("element {'a'} {1}", "ok"),
        ("element {1} {1}", "err:XPTY0004"),
        ("element {(<a/>, <b/>)} {1}", "err:XPTY0004"),
        ("attribute {'a'} {1}", "ok"),
        ("attribute {xs:QName('a')} {}", "ok"),
        ("processing-instruction {'p'} {'x'}", "ok"),
        ("processing-instruction {1} {'x'}", "err:XPTY0004"),
        ("document {1}", "ok"),
        ("document {attribute a {1}}", "err:XPTY0004"),
        ("<a>{attribute b {1}}</a>", "ok"),
        ("<a>{<c/>, attribute b {1}}</a>", "err:XQTY0024"),
        ("<a b='{1}'>{1}</a>", "ok"),
        ("<a><!--c--><?p x?>text</a>", "ok"),
    ]);
}

//! Tests that a type error in any operand of an expression is the whole
//! query's: each kind of expression, an operand of each of its places of
//! a type no operator has, `1 + 'a'`.

use super::alone;

/// An operand of no type an operator of it has [XQueryFS Appendix B.2].
const BAD: &str = "(1 + 'a')";

/// Each of `templates`, its `B` the operand `BAD`, compiled alone: each
/// err:XPTY0004.
fn each_fails(templates: &[&str]) {
    for template in templates {
        let query = template.replace('B', BAD);
        assert_eq!(alone(&query), "err:XPTY0004", "{query}");
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 SR6] [XQuery §2.2.3.1]
/// [XQueryFS §4.3, §4.4, §4.5, §4.6, §4.10, §7.2.14] an operand of a
/// type error makes its range, arithmetic, comparison, logical,
/// conditional, union, intersect and except expression one, in each of
/// its places.
#[test]
fn operators_pass_on_their_operands_type_errors() {
    each_fails(&[
        "B to 2",
        "1 to B",
        "-B",
        "B and true()",
        "true() or B",
        "if (B) then 1 else 2",
        "if (1) then B else 2",
        "if (1) then 1 else B",
        "1 * B",
        "B = 1",
        "1 = B",
        "B eq 1",
        "1 eq B",
        "B is <a/>",
        "<a/> | B",
        "B intersect <a/>",
        "<a/> except B",
        "<a/> intersect 1",
        "(B, 1)",
    ]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 SR6] [XQuery §2.2.3.1]
/// [XQueryFS §4.2, §4.3.2, §4.8, §4.11, §4.12] an operand of a type error
/// makes its path step, predicate, filter, FLWOR clause, quantified,
/// typeswitch and sequence type expression one.
#[test]
fn clauses_pass_on_their_operands_type_errors() {
    each_fails(&[
        "(B)[1]",
        "(1, 2)[B]",
        "<a/>/(B)",
        "<a/>/b[B]",
        "for $i in B return 1",
        "let $i := B return 1",
        "for $i in 1 where B return 1",
        "for $i in 1 order by B return 1",
        "for $i in 1 return B",
        "some $i in B satisfies 1",
        "every $i in 1 satisfies B",
        "typeswitch (B) case xs:integer return 1 default return 2",
        "typeswitch (1) case xs:integer return B default return 2",
        "typeswitch (1) case xs:integer return 1 default return B",
        "B instance of xs:integer",
        "B treat as item()",
        "B cast as xs:string",
        "B castable as xs:integer",
    ]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 SR6] [XQuery §2.2.3.1]
/// [XQueryFS §4.1.5, §4.7, §4.13] an operand of a type error makes its
/// constructor, validate expression, function call, declared function
/// and declared variable one.
#[test]
fn constructors_and_calls_pass_on_their_operands_type_errors() {
    each_fails(&[
        "<a>{B}</a>",
        "<a b='{B}'/>",
        "element a { B }",
        "element { B } { 1 }",
        "attribute a { B }",
        "attribute { B } { 1 }",
        "document { B }",
        "text { B }",
        "comment { B }",
        "processing-instruction p { B }",
        "processing-instruction { B } { 1 }",
        "validate lax { B }",
        "string(B)",
        "data(B)",
        "round(B)",
        "sum(B)",
        "subsequence(B, 1)",
        "zero-or-one(B)",
        "distinct-values(B)",
        "declare function local:f() { B }; local:f()",
        "declare variable $v := B; $v",
    ]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 SR6] [XQueryFS §4.1.5,
/// §7.2] a built-in function typed by a rule of its own still has its
/// arguments checked by its signature [XQueryFO]: a collation, a QName,
/// a position, a length or a zero of another type is err:XPTY0004.
#[test]
fn special_functions_check_their_signatures() {
    for query in [
        "distinct-values((1, 2), 1)",
        "error(1)",
        "remove((1, 2), 'a')",
        "insert-before((1, 2), 'a', 3)",
        "subsequence((1, 2), 'a')",
        "subsequence((1, 2), 1, 'a')",
        "min((1, 2), 1)",
        "sum((1, 2), 'a')",
        "abs('a')",
        "round-half-to-even(1.5, 'a')",
    ] {
        assert_eq!(alone(query), "err:XPTY0004", "{query}");
    }
}

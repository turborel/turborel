//! Tests of the static typing of calls of the functions of rules of
//! their own [XQueryFS §7.2] and of those typed by their signatures.

use super::alone;

/// Each of `cases`, a query and its outcome, compiled alone.
fn each(cases: &[(&str, &str)]) {
    for (query, expected) in cases {
        assert_eq!(alone(query), *expected, "{query}");
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 SR6] [XQueryFS §7.2.1 to
/// §7.2.9] the rounding functions keep their argument's numeric type, of
/// one number or none; fn:boolean of what has an effective boolean value;
/// fn:collection and fn:doc of documents; fn:data and
/// fn:distinct-values of atomic values; fn:unordered and fn:reverse their
/// argument's type, of none the empty type; fn:error of none, so a
/// branch of it adds nothing; fn:position and fn:last of a focus alone.
#[test]
fn rules_of_their_own_7_2_1_to_7_2_9() {
    each(&[
        ("abs(-1) + 1", "ok"),
        ("abs(xdt:untypedAtomic('1'))", "ok"),
        ("abs('a')", "err:XPTY0004"),
        ("abs(())", "ok"),
        ("abs((1, 2))", "err:XPTY0004"),
        ("ceiling(1.5) instance of xs:decimal", "ok"),
        ("floor(1.5e0) instance of xs:double", "ok"),
        ("round(xs:float(1)) instance of xs:float?", "ok"),
        ("round-half-to-even(1, 2)", "ok"),
        ("boolean((<a/>, <b/>)) and boolean(1)", "ok"),
        ("collection('x') | collection()", "ok"),
        ("doc('x')/a", "ok"),
        ("data((<a/>, 1))", "ok"),
        ("distinct-values((1, 2.5)) | distinct-values(<a/>)", "err:XPTY0004"),
        ("distinct-values((1, 'a'))", "ok"),
        ("unordered((1, 2)) + 1", "err:XPTY0004"),
        ("reverse(1) + 1", "ok"),
        ("reverse(())", "err:XPST0005"),
        ("error()", "ok"),
        ("error() + 1", "ok"),
        ("error(QName('urn:x', 'x'), 'm')", "ok"),
        ("if (1) then error() else 1", "ok"),
        ("position()", "err:XPST0001"),
        ("(1, 2)[position() + last() = 3]", "ok"),
    ]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 SR6] [XQueryFS §7.2.10 to
/// §7.2.16] fn:min, fn:max, fn:avg and fn:sum of values of one comparable
/// or summable type, numbers promoted, durations and dates as well, of
/// none optional, fn:sum's zero promoted; fn:remove, fn:subsequence and
/// fn:insert-before of their arguments' item types; fn:zero-or-one,
/// fn:one-or-more and fn:exactly-one of the occurrence each gives, of
/// the empty type `none`, which arithmetic keeps.
#[test]
fn rules_of_their_own_7_2_10_to_7_2_16() {
    each(&[
        ("min((1, 2)) + 1", "ok"),
        ("max((1, 2.5)) + 1", "ok"),
        ("max(('a', 'b'))", "ok"),
        ("min((1, 'a'))", "err:XPTY0004"),
        ("min(<a/>)", "err:XPTY0004"),
        ("max((xs:date('2026-01-01'), xs:date('2026-01-02')))", "ok"),
        ("min((xdt:dayTimeDuration('P1D'), xdt:dayTimeDuration('P2D')))", "ok"),
        ("min(())", "ok"),
        ("avg((1, 2)) + 1", "ok"),
        ("avg((xdt:dayTimeDuration('P1D'), xdt:dayTimeDuration('P2D')))", "ok"),
        ("avg(('a'))", "err:XPTY0004"),
        ("avg(())", "ok"),
        ("sum((1, 2)) + 1", "ok"),
        ("sum((1.5, 2)) + sum(()) + sum((), 0.0)", "ok"),
        ("sum((xdt:yearMonthDuration('P1Y')), xdt:yearMonthDuration('P0Y'))", "ok"),
        ("sum(('a'))", "err:XPTY0004"),
        ("sum((1, 2), ())", "ok"),
        ("remove((1, 2), 1) + 1", "err:XPTY0004"),
        ("remove(1, 1)", "ok"),
        ("remove((), 1)", "err:XPST0005"),
        ("subsequence((1, 2, 3), 2)", "ok"),
        ("subsequence((1, 2, 3), 2, 1) + subsequence(1, 1, 1)", "ok"),
        ("insert-before((1, 2), 1, 'a')", "ok"),
        ("insert-before((), 1, ())", "err:XPST0005"),
        ("zero-or-one((1, 2))", "ok"),
        ("zero-or-one(1) + 1", "ok"),
        ("one-or-more(())", "ok"),
        ("exactly-one(()) + 1", "ok"),
        ("exactly-one((1, 2)) + 1", "ok"),
    ]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 SR6] [XQueryFS §4.1.5,
/// §8.5.1] [XQueryFO] a built-in function of no rule of its own is typed
/// by its signature: its arguments' types promoted to its parameters',
/// of as many values, its result the signature's, so a constructor
/// function's optional value is no argument of one value; a variadic
/// function takes its least number of arguments or more; one of no
/// signature of its arity is err:XPST0017.
#[test]
fn signatures_type_the_other_functions() {
    each(&[
        ("concat('a', 'b', 'c', 1)", "ok"),
        ("concat('a')", "err:XPST0017"),
        ("string-join(('a', 'b'), '-')", "ok"),
        ("count((1, 2)) + 1", "ok"),
        ("not(1) or not((1, 2))", "ok"),
        ("upper-case(())", "ok"),
        ("upper-case(('a', 'b'))", "err:XPTY0004"),
        ("substring('abc', 1, 2)", "ok"),
        ("substring('abc', '1')", "err:XPTY0004"),
        ("number(<a>1</a>) + 1", "err:XPTY0004"),
        ("number(exactly-one(data(<a>1</a>))) + 1", "ok"),
        ("string(1)", "ok"),
        ("local-name(<a/>)", "ok"),
        ("root(<a/>)/a", "ok"),
        ("index-of((1, 2), 1)", "ok"),
        ("deep-equal(1, 1) and compare('a', 'b') = 0", "ok"),
        ("trace(1, 'x') + 1", "err:XPTY0004"),
        ("QName('urn:x', 'a')", "ok"),
        ("resolve-QName('a', <a/>)", "ok"),
        ("in-scope-prefixes(<a/>)", "ok"),
        ("current-dateTime() + xdt:dayTimeDuration('P1D')", "ok"),
        ("empty(()) or exists(1)", "ok"),
        ("local:none()", "err:XPST0017"),
        ("fn:data()", "err:XPST0017"),
        ("matches('a', 'a') and exists(tokenize('a b', ' '))", "ok"),
        ("codepoints-to-string((65, 66))", "ok"),
        ("string-to-codepoints('a') + 1", "err:XPTY0004"),
        ("dateTime(xs:date('2026-01-01'), xs:time('00:00:00'))", "err:XPTY0004"),
        ("dateTime(exactly-one(xs:date('2026-01-01')), exactly-one(xs:time('00:00:00')))", "ok"),
        ("adjust-date-to-timezone(xs:date('2026-01-01'))", "ok"),
        ("years-from-duration(xdt:yearMonthDuration('P1Y')) + 1", "ok"),
        ("nilled(<a/>)", "ok"),
        ("document-uri(document {<a/>})", "ok"),
        ("base-uri(<a/>)", "ok"),
        ("static-base-uri()", "ok"),
        ("default-collation()", "ok"),
        ("implicit-timezone()", "ok"),
        ("escape-html-uri('a')", "ok"),
    ]);
}

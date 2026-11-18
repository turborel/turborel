//! Tests of the evaluation of queries [XQuery §3]: primary expressions,
//! sequences, arithmetic, comparisons and logic here; paths, FLWOR
//! expressions, types and constructors in the modules below.

mod constructors;
mod depth;
mod flwor;
mod operators;
mod paths;
mod plugins;

use super::super::testing::{code_of, compile_with, error_of, evaluate, run, shown};
use crate::error::Code;
use crate::xml::{Atomic, AtomicType, Item, Sequence};

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2] [XQuery §3.1.1,
/// §3.3.1] literals are of xs:integer, xs:decimal, xs:double and
/// xs:string, cast from their text; the comma operator concatenates, and
/// `()` is the empty sequence.
#[test]
fn literals_and_sequences() {
    assert_eq!(run("1, 2.50, 1.5e1, 'a''b', \"c\"\"d\""), "1 2.5 15 a'b c\"d");
    assert_eq!(run("(1, (2, 3), (), 4)"), "1 2 3 4");
    assert_eq!(run("()"), "");
    assert_eq!(run("1 instance of xs:integer, 1.0 instance of xs:decimal, 1e0 instance of xs:double"), "true true true");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2] [XQuery §3.4]
/// [XQueryFO §6.2] arithmetic promotes its operands to their common type
/// [XQuery Appendix B]: integers stay integers, `div` of integers is a
/// decimal, `idiv` truncates, `mod` takes the dividend's sign; an untyped
/// operand is cast to xs:double; an empty operand gives the empty
/// sequence.
#[test]
fn arithmetic_of_numbers() {
    assert_eq!(run("1 + 2, 7 - 10, 6 * 7, 7 div 2, 7 idiv 2, -7 mod 2, 7.5 mod 2"), "3 -3 42 3.5 3 -1 1.5");
    assert_eq!(run("1 div 3"), "0.333333333333333333");
    assert_eq!(run("1 + 2.5, 1 + 2.5e0, (1 + 2.5) instance of xs:decimal, (1 + 2e0) instance of xs:double"), "3.5 3.5 true true");
    assert_eq!(run("-3 idiv 2, 3 idiv -2, -3.5 idiv 3, 3.1e1 idiv 7"), "-1 -1 -1 4");
    assert_eq!(run("1 div 0e0, -1 div 0e0, 0 div 0e0"), "INF -INF NaN");
    assert_eq!(run("() + 1, 1 * ()"), "");
    assert_eq!(run("- - 3, -(1 + 2), +4"), "3 -3 4");
    assert_eq!(run("xs:float(1.5) + xs:float(1)"), "2.5");
    assert_eq!(run("(xs:float(1) + 1) instance of xs:float"), "true");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §6.2] the
/// errors of arithmetic: division of exact numbers by zero err:FOAR0001,
/// an overflow past 1,024 digits err:FOAR0002, an operand of more than one
/// value or of no arithmetic err:XPTY0004, an untyped operand no number
/// err:FORG0001; each SQLSTATE 10000.
#[test]
fn arithmetic_errors() {
    assert_eq!(code_of("1 div 0"), "err:FOAR0001");
    assert_eq!(code_of("1 idiv 0"), "err:FOAR0001");
    assert_eq!(code_of("1.5 mod 0"), "err:FOAR0001");
    let big = "xs:integer(string-join(for $i in 1 to 600 return '9', ''))";
    assert_eq!(code_of(&format!("{big} * {big}")), "err:FOAR0002");
    assert_eq!(run("99999999999999999999999999999999999999 * 10"), "999999999999999999999999999999999999990");
    assert_eq!(code_of("(1, 2) + 1"), "err:XPTY0004");
    assert_eq!(code_of("'a' + 1"), "err:XPTY0004");
    assert_eq!(code_of("<a>x</a> + 1"), "err:FORG0001");
    let e = error_of("1 idiv 0");
    assert_eq!(e.state, Some(crate::error::SqlState::XQUERY_ERROR));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2] [XQuery §3.4]
/// [XQueryFO §10.6, §10.8] dates and durations add and subtract by the
/// operator mapping: a month added pins the day to the month's last;
/// dates subtract to a dayTimeDuration; durations multiply and divide.
#[test]
fn arithmetic_of_dates_and_durations() {
    assert_eq!(run("xs:date('2026-01-31') + xdt:yearMonthDuration('P1M')"), "2026-02-28");
    assert_eq!(run("xs:dateTime('2026-11-17T10:00:00Z') - xs:dateTime('2026-11-16T09:30:00Z')"), "P1DT30M");
    assert_eq!(run("xs:date('2026-03-01') - xs:date('2026-02-01')"), "P28D");
    assert_eq!(run("xs:time('23:30:00') + xdt:dayTimeDuration('PT1H')"), "00:30:00");
    assert_eq!(run("xdt:yearMonthDuration('P1Y') * 1.5, xdt:dayTimeDuration('PT1H') div 2"), "P1Y6M PT30M");
    assert_eq!(run("xdt:yearMonthDuration('P1Y') div xdt:yearMonthDuration('P3M')"), "4");
    assert_eq!(code_of("xs:date('9999-12-31') + xdt:dayTimeDuration('P1D')"), "err:FODT0001");
    assert_eq!(code_of("xs:date('2026-01-01') + xs:date('2026-01-01')"), "err:XPTY0004");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2] [XQuery §3.5.1] a
/// value comparison of one value each, an untyped one a string, numbers
/// promoted, strings by the codepoint collation; none if either is
/// empty; more than one value, or types not compared, err:XPTY0004.
#[test]
fn value_comparisons() {
    assert_eq!(run("1 eq 1.0, 1 lt 2e0, 'a' lt 'b', 'b' ge 'a', true() gt false(), 1 ne 2"), "true true true true true true");
    assert_eq!(run("<a>10</a> eq '10', <a>10</a> eq <b>10</b>"), "true true");
    assert_eq!(run("() eq 1"), "");
    assert_eq!(run("xs:double('NaN') eq xs:double('NaN'), xs:double('NaN') ne 1"), "false true");
    assert_eq!(run("xs:date('2026-01-01') lt xs:date('2026-01-02'), xs:time('10:00:00+01:00') eq xs:time('09:00:00Z')"), "true true");
    assert_eq!(run("xdt:yearMonthDuration('P12M') eq xdt:yearMonthDuration('P1Y')"), "true");
    assert_eq!(code_of("(1, 2) eq 1"), "err:XPTY0004");
    assert_eq!(code_of("1 eq 'a'"), "err:XPTY0004");
    assert_eq!(code_of("xs:hexBinary('AB') lt xs:hexBinary('CD')"), "err:XPTY0004");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2] [XQuery §3.5.2] a
/// general comparison is true if some pair compares so, an untyped value
/// cast to a number beside a number, else to the other's type; and the
/// node comparisons by identity and document order [XQuery §3.5.3].
#[test]
fn general_and_node_comparisons() {
    assert_eq!(run("(1, 2) = (2, 3), (1, 2) != (2, 3), (1, 2) = (3, 4), () = ()"), "true true false false");
    assert_eq!(run("<a>1.0</a> = 1, <a>1.0</a> = '1', <a>b</a> = <c>b</c>"), "true false true");
    assert_eq!(code_of("<a>x</a> = 1"), "err:FORG0001");
    assert_eq!(run("let $a := <a><b/><c/></a> return ($a/b << $a/c, $a/b is $a/b, $a/c >> $a/b, $a/b is $a/c)"), "true true true false");
    assert_eq!(run("<a/> is <a/>"), "false");
    assert_eq!(code_of("(<a/>, <b/>) is <a/>"), "err:XPTY0004");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2] [XQuery §3.6, §3.10,
/// §2.4.3] `and`, `or`, `if` and fn:not by the effective boolean value:
/// of a node first true, of a string whether it is empty, of a number
/// whether it is neither zero nor NaN; of two atomic values
/// err:FORG0006.
#[test]
fn logic_by_effective_boolean_value() {
    assert_eq!(run("1 and 'a', 0 or '', not(()), not(<a/>), boolean(xs:double('NaN'))"), "true false true false false");
    assert_eq!(run("if (()) then 1 else 2, if ((<a/>, 1)) then 1 else 2"), "2 1");
    assert_eq!(run("if (1) then 1 else 1 div 0"), "1");
    assert_eq!(code_of("if ((1, 2)) then 1 else 2"), "err:FORG0006");
    assert_eq!(code_of("boolean(xs:date('2026-01-01'))"), "err:FORG0006");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2] [XQuery §3.3.1,
/// §3.3.3] ranges of integers, empty if the first is the greater; union,
/// intersect and except of nodes in document order each once, an atomic
/// operand err:XPTY0004.
#[test]
fn ranges_and_node_sets() {
    assert_eq!(run("1 to 5, 3 to 3, 5 to 1"), "1 2 3 4 5 3");
    assert_eq!(run("<a>3</a> to 4"), "3 4");
    assert_eq!(code_of("1.5 to 3"), "err:XPTY0004");
    let doc = "for $d in <r><a/><b/><c/></r> return";
    assert_eq!(run(&format!("{doc} count(($d/c, $d/a) | $d/b)")), "3");
    assert_eq!(run(&format!("{doc} (((($d/c, $d/a) union $d/b)[1] is $d/a), (($d/c | $d/a)[2] is $d/c))")), "true true");
    assert_eq!(run(&format!("{doc} (count($d/* intersect ($d/a, $d/c)), count($d/* except $d/a))")), "2 2");
    assert_eq!(code_of("(1, 2) union <a/>"), "err:XPTY0004");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 SR5, GR1] [XQuery §3.1.2,
/// §3.1.4] a variable passed is in scope as a name of no namespace, its
/// value the one passed; the context item passed is `.`; with none, `.`
/// is err:XPDY0002.
#[test]
fn passed_variables_and_context_item() {
    let query = compile_with("$x + ., $y", &["x", "y"]).unwrap();
    let one = |n: &str| Sequence::new(vec![Item::Atomic(Atomic::new(AtomicType::Integer, n))]);
    let context = Some(Item::Atomic(Atomic::new(AtomicType::Integer, "40")));
    let value = evaluate(&query, context, vec![one("2"), Sequence::default()]).unwrap();
    assert_eq!(shown(&value), "42");
    assert_eq!(code_of("."), "err:XPDY0002");
    assert_eq!(code_of("$z"), "err:XPST0008");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2] [XQuery §3.12.1,
/// §3.12.3 to §3.12.6] `instance of` by sequence type matching, `cast`,
/// `castable` and the constructor functions by [XQueryFO §17], `treat`
/// err:XPDY0050 of a value not of its type.
#[test]
fn sequence_types_and_casts() {
    assert_eq!(
        run("(1, 2) instance of xs:integer+, () instance of xs:integer?, 1 instance of xs:string, <a/> instance of element(a)"),
        "true true false true"
    );
    assert_eq!(run("1 instance of xdt:anyAtomicType, <a/> instance of node()*, 'x' instance of item()"), "true true true");
    assert_eq!(run("'12' cast as xs:integer + 1, xs:date('2026-01-02'), () cast as xs:integer?"), "13 2026-01-02");
    assert_eq!(run("'x' castable as xs:integer, '1' castable as xs:integer, () castable as xs:integer"), "false true false");
    assert_eq!(run("xs:int('5') instance of xs:integer, xs:short(5) instance of xs:int"), "true true");
    assert_eq!(code_of("'x' cast as xs:integer"), "err:FORG0001");
    assert_eq!(code_of("() cast as xs:integer"), "err:XPTY0004");
    assert_eq!(code_of("1 treat as xs:string"), "err:XPDY0050");
    assert_eq!(code_of("1 cast as xdt:anyAtomicType"), "err:XPST0080");
    assert_eq!(code_of("1 instance of xs:nosuchtype"), "err:XPST0051");
    assert_eq!(code_of("'a:b' cast as xs:QName"), "err:FONS0004");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2, GR3] [XQuery §3.12.3
/// to §3.12.5] [XQueryFO §5.1, §17.1.1] a cast and the constructor
/// function to `xs:QName` of a string literal, its whitespace collapsed,
/// its prefix resolved by the statically known namespaces, none by the
/// default element namespace, the prefix kept; castable whether the cast
/// succeeds; an operand no string literal err:XPST0083, a prefix bound to
/// none err:FONS0004, a literal no QName err:FORG0001.
#[test]
fn casts_to_qnames() {
    let names = "declare namespace p = 'urn:p'; declare default element namespace 'urn:d'; ";
    let q = |body: &str| run(&format!("{names}{body}"));
    assert_eq!(q("namespace-uri-from-QName(' p:a ' cast as xs:QName), prefix-from-QName(xs:QName('p:a'))"), "urn:p p");
    assert_eq!(q("namespace-uri-from-QName(xs:QName('a')), local-name-from-QName(xs:QName('a'))"), "urn:d a");
    assert_eq!(q("xs:QName('p:a') eq QName('urn:p', 'q:a'), xs:QName('xml:lang') instance of xs:QName"), "true true");
    assert_eq!(q("'p:a' castable as xs:QName, 'z:a' castable as xs:QName, '1a' castable as xs:QName"), "true false false");
    assert_eq!(run("namespace-uri-from-QName(xs:QName('a')) eq ''"), "true");
    for (query, expected) in [
        ("xs:QName(('p:a'))", "err:XPST0083"),
        ("let $s := 'a' return $s cast as xs:QName", "err:XPST0083"),
        ("QName('urn:p', 'p:a') cast as xs:QName", "err:XPST0083"),
        ("let $s := 'a' return $s castable as xs:QName", "err:XPST0083"),
        ("xs:QName('z:a')", "err:FONS0004"),
        ("'1a' cast as xs:QName", "err:FORG0001"),
        ("xs:QName('a:b:c')", "err:FORG0001"),
    ] {
        assert_eq!(code_of(query), expected, "{query}");
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2] [XQueryFO §2.3, §2.4,
/// §7.4.1, §15.1, §15.4.1, §16.1, §16.2] the core built-in functions.
#[test]
fn core_functions() {
    assert_eq!(run("true(), false(), count((1, 2, 3)), empty(()), exists(()), fn:concat('a', 1, (), 'b')"), "true false 3 true false a1b");
    assert_eq!(run("string(<a>x<b>y</b></a>), string(()), data(<a>1</a>) instance of xdt:untypedAtomic"), "xy  true");
    assert_eq!(run("(10, 20, 30)[position() = last()], (10, 20, 30)[last() - 1]"), "30 20");
    assert_eq!(run("count(root(<a><b/></a>/b)), <a><b/></a>/b/root() instance of element(a)"), "1 true");
    assert_eq!(code_of("(1)[root()]"), "err:FOTY0011");
    assert_eq!(run("data(<!--c-->) instance of xs:string, data(<?t c?>) instance of xs:string"), "true true");
    assert_eq!(code_of("doc('x.xml')"), "err:FODC0005");
    assert_eq!(code_of("position()"), "err:FONC0001");
    assert_eq!(code_of("no-such-function()"), "err:XPST0017");
    assert_eq!(code_of("fn:count(1, 2)"), "err:XPST0017");
}

/// Verifies: [DESIGN] RULES.md D44, D86: an evaluation looks whether its
/// statement was cancelled as each tuple of a FLWOR expression is made,
/// and fails with the cancellation's error.
#[test]
fn cancellation_stops_an_evaluation() {
    let query = compile_with("for $i in 1 to 100000 return $i", &[]).unwrap();
    let looks = std::cell::Cell::new(0_usize);
    let interrupted = || {
        looks.set(looks.get() + 1);
        if looks.get() > 10 { Err(crate::error::SqlError::new(Code::Cancelled, "[DESIGN] cancelled")) } else { Ok(()) }
    };
    let now = super::super::testing::now();
    let input = super::Input { context_item: None, variables: Vec::new(), now, timezone: 0, interrupted: &interrupted };
    assert_eq!(query.evaluate(input).unwrap_err().code, Code::Cancelled);
    assert_eq!(looks.get(), 11);
}

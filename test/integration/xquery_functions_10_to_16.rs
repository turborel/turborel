//! The functions and operators of XQuery 1.0 [XQueryFO] of §10 to §16,
//! as XMLQUERY calls them [ISO/IEC 9075-14:2006 draft §6.18]: durations,
//! dates and times, QNames, binary values, nodes, sequences and the
//! context, of the documents and with the helpers of
//! `xquery_functions.rs`.

use crate::xquery_functions::{code, codes, orders, value_of, values};

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2, GR3] [XQueryFO
/// §10.4, §10.5] durations compared and taken apart: a yearMonthDuration
/// and a dayTimeDuration ordered, durations of the two kinds equal by
/// months and seconds, their components; `lt` of `xs:duration`
/// err:XPTY0004.
#[test]
fn durations_xqueryfo_10_5() {
    let a = orders();
    values(
        &a,
        &[
            ("years-from-duration(xdt:yearMonthDuration(\"P20Y15M\")), months-from-duration(xdt:yearMonthDuration(\"-P20Y18M\"))", "21 -6"),
            ("days-from-duration(xdt:dayTimeDuration(\"P3DT10H\")), hours-from-duration(xdt:dayTimeDuration(\"P3DT10H\"))", "3 10"),
            (
                "minutes-from-duration(xdt:dayTimeDuration(\"-P5DT12H30M\")), seconds-from-duration(xdt:dayTimeDuration(\"P3DT10H12.5S\"))",
                "-30 12.5",
            ),
            (
                "xdt:yearMonthDuration(\"P1Y\") eq xs:duration(\"P12M\"), xdt:dayTimeDuration(\"PT24H\") lt xdt:dayTimeDuration(\"P1DT1S\")",
                "true true",
            ),
        ],
    );
    assert_eq!(code(&a, "xs:duration(\"P1Y\") lt xs:duration(\"P2Y\")"), "XPTY0004");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2] [XQueryFO §10.4,
/// §10.5, §10.7] dates and times compared in the implicit time zone, the
/// session's, taken apart, and adjusted to another time zone or none;
/// a time zone past fourteen hours err:FODT0003.
#[test]
fn dates_and_times_xqueryfo_10() {
    let a = orders();
    values(
        &a,
        &[
            ("xs:dateTime(\"2002-04-02T12:00:00-01:00\") eq xs:dateTime(\"2002-04-02T17:00:00+04:00\")", "true"),
            ("xs:date(\"2004-12-25Z\") lt xs:date(\"2004-12-26-05:00\"), xs:gDay(\"---12\") eq xs:gDay(\"---12\")", "true true"),
            (
                "year-from-dateTime(xs:dateTime(\"1999-12-31T19:20:00-05:00\")), year-from-dateTime(xs:dateTime(\"1999-12-31T24:00:00\")), hours-from-dateTime(xs:dateTime(\"1999-12-31T24:00:00\"))",
                "1999 2000 0",
            ),
            (
                "month-from-date(xs:date(\"1999-05-31-05:00\")), day-from-date(xs:date(\"2000-01-01+05:00\")), timezone-from-date(xs:date(\"1999-05-31-05:00\"))",
                "5 1 -PT5H",
            ),
            (
                "seconds-from-time(xs:time(\"13:20:10.5\")), minutes-from-time(xs:time(\"13:00:00Z\")), empty(timezone-from-time(xs:time(\"13:00:00\")))",
                "10.5 0 true",
            ),
            (
                "adjust-dateTime-to-timezone(xs:dateTime(\"2002-03-07T10:00:00-07:00\"), xdt:dayTimeDuration(\"-PT10H\"))",
                "2002-03-07T07:00:00-10:00",
            ),
            (
                "adjust-date-to-timezone(xs:date(\"2002-03-07-07:00\"), ()), adjust-time-to-timezone(xs:time(\"10:00:00\"), xdt:dayTimeDuration(\"PT5H\"))",
                "2002-03-07 10:00:00+05:00",
            ),
        ],
    );
    assert_eq!(code(&a, "adjust-time-to-timezone(xs:time(\"10:00:00\"), xdt:dayTimeDuration(\"PT15H\"))"), "FODT0003");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2, GR3] [XQueryFO §10.6,
/// §10.8] the arithmetic of durations, dates and times: durations added,
/// scaled and divided; dates and times subtracted to a dayTimeDuration;
/// a duration added to a date, a month past its last day pinned; a date
/// past 9999 err:FODT0001; a combination the mapping lacks err:XPTY0004.
#[test]
fn date_arithmetic_xqueryfo_10_6() {
    let a = orders();
    values(
        &a,
        &[
            (
                "xdt:yearMonthDuration(\"P2Y11M\") + xdt:yearMonthDuration(\"P3Y3M\"), xdt:yearMonthDuration(\"P2Y11M\") * 2.3",
                "P6Y2M P6Y9M",
            ),
            (
                "xdt:dayTimeDuration(\"P1DT2H30M10.5S\") div 1.5, xdt:yearMonthDuration(\"P3Y4M\") div xdt:yearMonthDuration(\"-P1Y4M\")",
                "PT17H40M7S -2.5",
            ),
            ("xs:date(\"2000-10-30\") - xs:date(\"1999-11-28\"), xs:time(\"11:12:00Z\") - xs:time(\"04:00:00-05:00\")", "P337D PT2H12M"),
            (
                "xs:date(\"2000-01-31\") + xdt:yearMonthDuration(\"P1M\"), xdt:dayTimeDuration(\"P3DT1H15M\") + xs:date(\"2004-10-30\")",
                "2000-02-29 2004-11-02",
            ),
            (
                "xs:dateTime(\"2000-10-30T11:12:00\") - xdt:dayTimeDuration(\"P3DT1H15M\"), xs:time(\"23:00:00\") + xdt:dayTimeDuration(\"PT2H\")",
                "2000-10-27T09:57:00 01:00:00",
            ),
        ],
    );
    codes(
        &a,
        &[
            ("xs:date(\"9999-12-31\") + xdt:dayTimeDuration(\"P1D\")", "FODT0001"),
            ("xs:date(\"2000-01-01\") + xs:time(\"00:00:00\")", "XPTY0004"),
        ],
    );
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2, GR3] [XQueryFO §11]
/// QNames made, resolved by an element's in-scope namespaces, taken apart
/// and compared by namespace URI and local name; a prefix bound nowhere
/// err:FONS0004, a lexical form that is none err:FOCA0002.
#[test]
fn qnames_xqueryfo_11() {
    let a = orders();
    values(
        &a,
        &[
            (
                "prefix-from-QName(QName(\"urn:q\", \"q:a\")), local-name-from-QName(QName(\"urn:q\", \"q:a\")), namespace-uri-from-QName(QName(\"urn:q\", \"a\"))",
                "q a urn:q",
            ),
            (
                "namespace-uri-from-QName(resolve-QName(\"p:x\", /order)), empty(prefix-from-QName(resolve-QName(\"x\", /order)))",
                "urn:p true",
            ),
            (
                "namespace-uri-for-prefix(\"p\", /order), count(in-scope-prefixes(/order)), in-scope-prefixes(/order) = \"xml\"",
                "urn:p 2 true",
            ),
            ("QName(\"urn:q\", \"a:x\") eq QName(\"urn:q\", \"b:x\"), QName(\"urn:q\", \"x\") eq QName(\"urn:r\", \"x\")", "true false"),
        ],
    );
    codes(
        &a,
        &[("resolve-QName(\"z:x\", /order)", "FONS0004"), ("QName(\"urn:q\", \"1a\")", "FOCA0002"), ("QName(\"\", \"p:a\")", "FOCA0002")],
    );
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2, GR3, §10.20 SR2]
/// [XQuery §3.12.3 to §3.12.5] [XQueryFO §5.1, §17.1.1] a cast or
/// constructor function to `xs:QName` of a string literal, its prefix
/// bound by the prolog, by the SQL XMLNAMESPACES in scope or by the
/// predeclared namespaces, none the default element namespace's; castable
/// whether it succeeds; an operand no literal err:XPST0083, a prefix
/// bound to none err:FONS0004.
#[test]
fn casts_to_qnames_xquery_3_12() {
    let a = orders();
    values(
        &a,
        &[
            ("declare namespace q = \"urn:q\"; namespace-uri-from-QName(xs:QName(\"q:x\")), \"q:y\" cast as xs:QName", "urn:q q:y"),
            (
                "namespace-uri-from-QName(\"xs:int\" cast as xs:QName), \"p:x\" castable as xs:QName",
                "http://www.w3.org/2001/XMLSchema false",
            ),
            ("declare default element namespace \"urn:d\"; namespace-uri-from-QName(xs:QName(\"x\"))", "urn:d"),
        ],
    );
    let scoped = "WITH XMLNAMESPACES ('urn:s' AS \"s\", DEFAULT 'urn:e') SELECT XMLSERIALIZE(CONTENT XMLQUERY('namespace-uri-from-QName(xs:QName(\"s:x\")), namespace-uri-from-QName(xs:QName(\"x\"))' EMPTY ON EMPTY) AS VARCHAR(40)) FROM T WHERE K = 1";
    assert_eq!(crate::common::rows(&a, scoped).unwrap(), ["urn:s urn:e"]);
    codes(&a, &[("xs:QName(string(\"q:x\"))", "XPST0083"), ("xs:QName(\"q:x\")", "FONS0004"), ("\"1x\" cast as xs:QName", "FORG0001")]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2, GR3] [XQueryFO §12,
/// §13] binary values equal by their octets, in either case; ordered they
/// are not, err:XPTY0004.
#[test]
fn binary_values_xqueryfo_12() {
    let a = orders();
    values(
        &a,
        &[
            ("xs:hexBinary(\"0fb7\") eq xs:hexBinary(\"0FB7\"), xs:base64Binary(\"D7c=\") eq xs:base64Binary(\"D7c=\")", "true true"),
            ("xs:hexBinary(\"00\") ne xs:hexBinary(\"01\"), xs:base64Binary(xs:hexBinary(\"0FB7\"))", "true D7c="),
        ],
    );
    codes(
        &a,
        &[("xs:hexBinary(\"00\") lt xs:hexBinary(\"01\")", "XPTY0004"), ("xs:hexBinary(\"00\") eq xs:base64Binary(\"AA==\")", "XPTY0004")],
    );
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2, GR3] [XQueryFO §14]
/// the functions on nodes: names, numbers, languages and roots; the node
/// comparisons by identity and document order; the context item no node
/// err:FOTY0011.
#[test]
fn nodes_xqueryfo_14() {
    let a = orders();
    values(
        &a,
        &[
            (
                "name(/order/*[1]), local-name(/order/*[1]), namespace-uri(/order/*[1]), name(/order/item[1]/@price)",
                "p:item item urn:p price",
            ),
            ("number(/order/item[1]/@price), number(\"x\"), number(()), number(/order/item[1])", "25 NaN NaN NaN"),
            ("lang(\"en\", /order/*[1]), lang(\"de\", /order/item[2]), lang(\"en\", /order/item[2])", "true true false"),
            ("root(/order/item[1]) is /, root(text { \"t\" }) instance of text(), empty(root(()))", "true true true"),
            ("/order/*[1] << /order/item[1], /order/item[2] >> /order/item[1], /order/item[1] is (//item)[1]", "true true true"),
            ("for $i in //item return string-length(name($i))", "4 4"),
        ],
    );
    codes(&a, &[("(1)[name()]", "FOTY0011"), ("name(1)", "XPTY0004")]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2, GR3] [XQueryFO
/// §15.1] the functions of sequences: positions, distinct values,
/// insertion and removal, reversal and subsequences; the cardinality
/// checks err:FORG0003, err:FORG0004 and err:FORG0005.
#[test]
fn sequences_xqueryfo_15_1() {
    let a = orders();
    values(
        &a,
        &[
            (
                "index-of((10, 20, 30, 40), 35), index-of((10, 20, 30, 30, 20, 10), 20), index-of((\"a\", \"sport\", \"and\", \"a\"), \"a\")",
                "2 5 1 4",
            ),
            ("distinct-values((1, 2.0, 3, 2)), count(distinct-values((xs:double(\"NaN\"), xs:float(\"NaN\"))))", "1 2 3 1"),
            (
                "insert-before((\"a\", \"b\", \"c\"), 0, \"z\"), insert-before((\"a\", \"b\", \"c\"), 2, \"z\"), insert-before((\"a\"), 9, \"z\")",
                "z a b c a z b c a z",
            ),
            ("remove((\"a\", \"b\", \"c\"), 2), remove((\"a\", \"b\"), 0), reverse((1, 2, 3)), empty(reverse(()))", "a c a b 3 2 1 true"),
            ("subsequence((1, 2, 3, 4, 5), 4), subsequence((1, 2, 3, 4, 5), 3, 2), subsequence((1, 2, 3), 0.5, 1.6)", "4 5 3 4 1 2"),
            ("unordered((3, 1)), zero-or-one(()), one-or-more(1), exactly-one(\"x\"), empty(()), exists(//item)", "3 1 1 x true true"),
        ],
    );
    codes(&a, &[("zero-or-one((1, 2))", "FORG0003"), ("one-or-more(())", "FORG0004"), ("exactly-one((1, 2))", "FORG0005")]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2, GR3] [XQueryFO
/// §15.2, §15.3, §15.5.1] fn:deep-equal of values and of trees, the
/// operators union, intersect and except, and ranges by op:to; an operand
/// of union that is no node err:XPTY0004.
#[test]
fn equality_and_node_sets_xqueryfo_15_2() {
    let a = orders();
    values(
        &a,
        &[
            (
                "deep-equal((1, \"a\"), (1.0, \"a\")), deep-equal(<a x=\"1\"><b/></a>, <a x=\"1\"><b/></a>), deep-equal(<a/>, <a x=\"1\"/>)",
                "true true false",
            ),
            (
                "deep-equal(\"A\", \"a\", \"urn:turborel:collation:CASE_INSENSITIVE\"), deep-equal(/order/item[1], /order/item[2])",
                "true false",
            ),
            ("count(//item | /order/*[1]), count(//item intersect /order/*[position() > 1]), count(/order/* except //item)", "3 2 1"),
            ("1 to 4, count(5 to 2), (-2 to -1)", "1 2 3 4 0 -2 -1"),
        ],
    );
    codes(&a, &[("count((1, 2) | //item)", "XPTY0004"), ("(1, 2) to 3", "XPTY0004")]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2, GR3] [XQueryFO §15.4]
/// the aggregates: count, avg, max, min and sum of numbers, of durations,
/// of strings by collation, of untyped values as doubles; the empty
/// sequence's; values of no common type err:FORG0006.
#[test]
fn aggregates_xqueryfo_15_4() {
    let a = orders();
    values(
        &a,
        &[
            ("count(//item), sum(//item/@price), avg(//item/@price), max(//item/@price), min(//item/@price)", "2 30 15 25 5"),
            ("sum((1, 2.5)), avg((1, 2)), sum(()), empty(avg(())), sum((), \"none\")", "3.5 1.5 0 true none"),
            ("max((\"a\", \"B\")), max((\"a\", \"B\"), \"urn:turborel:collation:CASE_INSENSITIVE\"), min((3, 4.5, 2e0))", "a B 2"),
            (
                "sum((xdt:dayTimeDuration(\"PT1H\"), xdt:dayTimeDuration(\"PT30M\"))), avg((xdt:yearMonthDuration(\"P1Y\"), xdt:yearMonthDuration(\"P2Y\")))",
                "PT1H30M P1Y6M",
            ),
            ("max((xs:date(\"2000-01-01\"), xs:date(\"2001-01-01\"))), max((1, xs:double(\"NaN\")))", "2001-01-01 NaN"),
        ],
    );
    codes(&a, &[("sum((1, \"a\"))", "FORG0006"), ("max((1, xs:date(\"2000-01-01\")))", "FORG0006"), ("avg((\"a\", \"b\"))", "FORG0006")]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2, GR3] [XQueryFO
/// §15.5] fn:id and fn:idref by the ID and IDREF attributes a document
/// type declaration makes; no document available to fn:doc, err:FODC0005,
/// and no collection, err:FODC0004 and err:FODC0002.
#[test]
fn documents_and_ids_xqueryfo_15_5() {
    let a = orders();
    assert_eq!(value_of(&a, 2, "count(id(\"a b\")), id(\"b\")/@id/string(), count(id(\"none\"))"), "2 b 0");
    assert_eq!(value_of(&a, 2, "count(idref(\"a\")), count(idref(\"b\"))"), "2 1");
    values(&a, &[("empty(doc(())), doc-available(\"http://e.org/d.xml\")", "true false")]);
    codes(&a, &[("doc(\"http://e.org/d.xml\")", "FODC0005"), ("collection(\"urn:c\")", "FODC0004"), ("collection()", "FODC0002")]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR1, GR2] [XQueryFO §16]
/// the context functions: the position and size of a predicate's focus;
/// the current dateTime, the same at each call, its date and time, in
/// the session's implicit time zone; the default collation the code point
/// collation, or the prolog's; the static base URI none, or the prolog's.
#[test]
fn context_xqueryfo_16() {
    let a = orders();
    values(
        &a,
        &[
            ("//item[position() = last()]/string(), (10, 20, 30)[last() - 1]", "cap 20"),
            (
                "current-dateTime() eq current-dateTime(), current-date() instance of xs:date, current-time() instance of xs:time",
                "true true true",
            ),
            ("implicit-timezone() eq timezone-from-dateTime(current-dateTime())", "true"),
            ("default-collation(), empty(static-base-uri())", "http://www.w3.org/2005/xpath-functions/collation/codepoint true"),
            (
                "declare default collation \"urn:turborel:collation:CASE_INSENSITIVE\"; default-collation(), compare(\"A\", \"a\")",
                "urn:turborel:collation:CASE_INSENSITIVE 0",
            ),
        ],
    );
    codes(&a, &[("declare function local:f() { position() }; local:f()", "FONC0001")]);
}

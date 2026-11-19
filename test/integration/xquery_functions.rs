//! The functions and operators of XQuery 1.0 [XQueryFO], as XMLQUERY
//! calls them [ISO/IEC 9075-14:2006 draft §6.18]: several of each section,
//! of a document passed as the context item, their errors SQLSTATE 10000
//! naming their codes [§6.18 GR3]; here §2 to §9, and the rest in
//! `xquery_functions_10_to_16.rs`, which shares these helpers.

use turborel::db::{Database, Session};
use turborel::error::{SqlError, SqlState};

use crate::common::rows;

/// Schema A: table T, row 1 an order of three items, row 2 a document
/// whose type declaration makes `id` an ID and `ref` an IDREF.
pub fn orders() -> Session {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, X XML)").unwrap();
    let a = db.session("A");
    a.run(
        "INSERT INTO T VALUES \
         (1, XMLPARSE(DOCUMENT '<order xml:base=\"http://e.org/shop/\" xmlns:p=\"urn:p\" xml:lang=\"en-US\">\
<p:item price=\"10\">pen</p:item><item price=\"25\">ink</item><item price=\"5\" xml:lang=\"de\">cap</item></order>' STRIP WHITESPACE)), \
         (2, XMLPARSE(DOCUMENT '<!DOCTYPE r [<!ATTLIST e id ID #IMPLIED ref IDREFS #IMPLIED>]><r><e id=\"a\"/><e id=\"b\" ref=\"a\"/><e ref=\"a b\"/></r>' STRIP WHITESPACE))",
    )
    .unwrap();
    a.commit().unwrap();
    a
}

/// The value of `query` of row `k`'s document, serialized: its items a
/// space apart.
pub fn value_of(a: &Session, k: i32, query: &str) -> String {
    let sql = format!(
        "SELECT XMLSERIALIZE(CONTENT XMLQUERY('{}' PASSING BY REF X EMPTY ON EMPTY) AS VARCHAR(2000)) FROM T WHERE K = {k}",
        query.replace('\'', "''")
    );
    let mut found = rows(a, &sql).unwrap_or_else(|_| panic!("{query}: {}", error_of(a, &sql)));
    assert_eq!(found.len(), 1, "{query}");
    found.remove(0)
}

/// The value of `query` of the order, row 1.
pub fn value(a: &Session, query: &str) -> String {
    value_of(a, 1, query)
}

/// The error `sql` fails with, as the statement is prepared or as its
/// row is fetched.
fn error_of(a: &Session, sql: &str) -> SqlError {
    match a.open_cursor(sql) {
        Err(e) => e,
        Ok(mut cursor) => cursor.fetch().map(|_| ()).expect_err(sql),
    }
}

/// The XQuery error code `query` of the order fails with, SQLSTATE 10000.
pub fn code(a: &Session, query: &str) -> String {
    let sql = format!(
        "SELECT XMLSERIALIZE(CONTENT XMLQUERY('{}' PASSING BY REF X EMPTY ON EMPTY) AS VARCHAR(2000)) FROM T WHERE K = 1",
        query.replace('\'', "''")
    );
    let error = error_of(a, &sql);
    assert_eq!(error.sqlstate(), SqlState::XQUERY_ERROR, "{query}: {error}");
    let message = error.to_string();
    let (_, after) = message.split_once("err:").unwrap_or_else(|| panic!("no code in {message}"));
    after.chars().take_while(char::is_ascii_alphanumeric).collect()
}

/// Each of `cases`, a query and its value, checked.
pub fn values(a: &Session, cases: &[(&str, &str)]) {
    for (query, expected) in cases {
        assert_eq!(value(a, query), *expected, "{query}");
    }
}

/// Each of `cases`, a query and its error code, checked.
pub fn codes(a: &Session, cases: &[(&str, &str)]) {
    for (query, expected) in cases {
        assert_eq!(code(a, query), *expected, "{query}");
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2] [XQueryFO §2.1 to
/// §2.6] the accessors of a query's nodes: a node's name as a QName, its
/// string value and typed value, an element's nilled property; the base
/// URI its `xml:base` gives, and no document URI.
#[test]
fn accessors_xqueryfo_2() {
    let a = orders();
    values(
        &a,
        &[
            ("local-name-from-QName(node-name(/order/*[1])), namespace-uri-from-QName(node-name(/order/*[1]))", "item urn:p"),
            ("node-name(text { \"t\" }), nilled(/order), nilled(text { \"t\" })", "false"),
            ("string(/order), string(), string(12.50), string(())", "peninkcap peninkcap 12.5 "),
            ("data(/order/item/@price), data(/order/item[1]) instance of xdt:untypedAtomic", "25 5 true"),
            ("base-uri(/order/item[1]), base-uri(())", "http://e.org/shop/"),
            ("empty(document-uri(/)), empty(document-uri(())), base-uri(text { \"t\" })", "true true"),
        ],
    );
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2, GR3] [XQueryFO §2.3,
/// §2.4, §2.5] the accessors' errors: no context item err:FONC0001 (the
/// query of no argument passed), an argument of more than one node
/// err:XPTY0004, an atomic value where a node is wanted err:XPTY0004.
#[test]
fn accessor_errors_xqueryfo_2() {
    let a = orders();
    codes(&a, &[("node-name((/order, /order))", "XPTY0004"), ("base-uri(1)", "XPTY0004"), ("nilled(\"x\")", "XPTY0004")]);
    let sql = "SELECT XMLQUERY('string()' EMPTY ON EMPTY) FROM T WHERE K = 1";
    let error = error_of(&a, sql);
    assert!(error.to_string().contains("err:FONC0001"), "{error}");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §3, §4]
/// fn:error raises the error it names, err:FOER0000 of none, SQLSTATE
/// 10000; fn:trace gives its value unchanged.
#[test]
fn error_and_trace_xqueryfo_3_4() {
    let a = orders();
    codes(
        &a,
        &[
            ("error()", "FOER0000"),
            ("error(QName(\"http://www.w3.org/2005/xqt-errors\", \"err:FOAR0001\"), \"mine\")", "FOAR0001"),
            ("error(\"x\")", "XPTY0004"),
        ],
    );
    let other = "SELECT XMLQUERY('error(QName(\"urn:app\", \"app:E1\"), \"no stock\")' EMPTY ON EMPTY) FROM T WHERE K = 1";
    let error = error_of(&a, other);
    assert!(error.to_string().contains("urn:app#E1: no stock"), "{error}");
    values(&a, &[("trace((1, 2), \"label\")", "1 2"), ("count(trace(//item, \"items\"))", "2")]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2, GR3] [XQueryFO §5.1,
/// §5.2] a constructor function is a cast, the empty sequence of the
/// empty sequence, a value its type refuses err:FORG0001; fn:dateTime of
/// a date and a time, of the time zone either has, two different
/// err:FORG0008.
#[test]
fn constructors_xqueryfo_5() {
    let a = orders();
    values(
        &a,
        &[
            ("xs:unsignedInt(\"12\") + 1, xs:unsignedInt(12) instance of xs:unsignedInt", "13 true"),
            ("xdt:dayTimeDuration(\"PT36H\"), empty(xs:date(())), xs:boolean(\"1\")", "P1DT12H true true"),
            ("dateTime(xs:date(\"1999-12-31\"), xs:time(\"12:00:00\"))", "1999-12-31T12:00:00"),
            ("dateTime(xs:date(\"1999-12-31Z\"), xs:time(\"24:00:00\"))", "1999-12-31T00:00:00Z"),
            ("dateTime(xs:date(\"1999-12-31+01:00\"), xs:time(\"12:00:00+01:00\"))", "1999-12-31T12:00:00+01:00"),
        ],
    );
    codes(
        &a,
        &[
            ("xs:unsignedInt(-1)", "FORG0001"),
            ("xs:integer((1, 2))", "XPTY0004"),
            ("dateTime(xs:date(\"1999-12-31Z\"), xs:time(\"12:00:00+01:00\"))", "FORG0008"),
        ],
    );
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2, GR3] [XQueryFO §6.2,
/// §6.3] the operators on numbers: each of the four types, promoted to
/// their common one; a quotient of decimals exact; integers and decimals
/// of many digits; division by zero err:FOAR0001, of floats an infinity;
/// an untyped operand a double.
#[test]
fn numeric_operators_xqueryfo_6_2() {
    let a = orders();
    values(
        &a,
        &[
            ("1 + 2, 7 - 10, 6 * 7, 7 div 2, 7 idiv 2, -7 mod 2", "3 -3 42 3.5 3 -1"),
            ("1.5 + 1, 1 div 3, 10.0 div 4, 0.1 + 0.2 eq 0.3", "2.5 0.333333333333333333 2.5 true"),
            ("123456789012345678901234567890 * 1000000000000000000000", "123456789012345678901234567890000000000000000000000"),
            ("1e0 div 0, -1 div 0e0, (xs:float(1) div 3) instance of xs:float, 1 + 1e0", "INF -INF true 2"),
            ("/order/item[2]/@price * 2, -(1), +(2.5)", "10 -1 2.5"),
            ("1 lt 1.5, 2 eq 2.0, xs:double(\"NaN\") eq xs:double(\"NaN\"), 3 ge 3e0", "true true false true"),
        ],
    );
    codes(&a, &[("1 div 0", "FOAR0001"), ("1.5 idiv 0", "FOAR0001"), ("xs:double(\"INF\") idiv 1", "FOAR0002"), ("\"a\" + 1", "XPTY0004")]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2] [XQueryFO §6.4.1 to
/// §6.4.5] fn:abs, fn:ceiling, fn:floor, fn:round and
/// fn:round-half-to-even, each of its argument's type, the empty sequence
/// of the empty sequence; the examples of the text.
#[test]
fn numeric_functions_xqueryfo_6_4() {
    let a = orders();
    values(
        &a,
        &[
            ("abs(-3), abs(-1.5), abs(xs:double(\"-INF\")), empty(abs(()))", "3 1.5 INF true"),
            ("ceiling(10.5), ceiling(-10.5), floor(10.5), floor(-10.5)", "11 -10 10 -11"),
            ("round(2.5), round(2.4999), round(-2.5)", "3 2 -2"),
            ("round-half-to-even(0.5), round-half-to-even(1.5), round-half-to-even(2.5)", "0 2 2"),
            ("round-half-to-even(3.567812e+3, 2), round-half-to-even(4.7564e-3, 2), round-half-to-even(35612.25, -2)", "3567.81 0 35600"),
            ("ceiling(xs:float(1.2)) instance of xs:float, round(xs:double(\"NaN\"))", "true NaN"),
        ],
    );
    assert_eq!(code(&a, "abs(\"1\")"), "XPTY0004");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2, GR3] [XQueryFO §7.2,
/// §7.3, §7.4] the functions on strings: code points, comparison by the
/// code point collation or one named, joining, substrings by position,
/// lengths and normalized space; a code point that is no character
/// err:FOCH0001; a collation not known err:FOCH0002.
#[test]
fn strings_xqueryfo_7() {
    let a = orders();
    values(
        &a,
        &[
            ("codepoints-to-string((2309, 2358, 2378, 2325)), string-to-codepoints(\"Thérèse\")", "अशॊक 84 104 233 114 232 115 101"),
            (
                "compare(\"abc\", \"abd\"), compare(\"b\", \"B\"), compare(\"b\", \"B\", \"urn:turborel:collation:CASE_INSENSITIVE\")",
                "-1 1 0",
            ),
            ("codepoint-equal(\"a\", \"a\"), empty(compare((), \"a\"))", "true true"),
            ("concat(\"un\", \"grate\", (), 1), string-join((\"a\", \"b\", \"c\"), \"-\"), string-join((), \"-\")", "ungrate1 a-b-c "),
            (
                "substring(\"motor car\", 6), substring(\"metadata\", 4, 3), substring(\"12345\", 1.5, 2.6), substring(\"12345\", 0, 3)",
                " car ada 234 12",
            ),
            (
                "string-length(\"Harp not on that string\"), string-length(), normalize-space(\" The  wealthy curled darlings \")",
                "23 9 The wealthy curled darlings",
            ),
        ],
    );
    codes(
        &a,
        &[("codepoints-to-string(0)", "FOCH0001"), ("compare(\"a\", \"b\", \"urn:nowhere\")", "FOCH0002"), ("concat(\"a\")", "XPST0017")],
    );
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2, GR3] [XQueryFO §7.4.6
/// to §7.4.12] normalization by Unicode 13.0, case mapping in full,
/// translation and the escapes of URIs; FULLY-NORMALIZED the NFC of a
/// string [XML 1.1 Appendix B], and of one beginning with a composing
/// character err:FOCH0003.
#[test]
fn strings_mapped_xqueryfo_7_4() {
    let a = orders();
    values(
        &a,
        &[
            ("string-to-codepoints(normalize-unicode(\"e\u{301}\")), string-length(normalize-unicode(\"\u{e9}\", \"NFD\"))", "233 2"),
            ("normalize-unicode(\"\u{fb01}\", \"NFKC\"), normalize-unicode(\"x\", \"\")", "fi x"),
            ("string-to-codepoints(normalize-unicode(\"e\u{301}x\", \"fully-normalized\"))", "233 120"),
            ("upper-case(\"abCd0\"), lower-case(\"ABc!D\"), upper-case(\"stra\u{df}e\")", "ABCD0 abc!d STRASSE"),
            (
                "translate(\"bar\", \"abc\", \"ABC\"), translate(\"--aaa--\", \"abc-\", \"ABC\"), translate(\"abcdabc\", \"abc\", \"AB\")",
                "BAr AAA ABdAB",
            ),
            ("encode-for-uri(\"100% organic\"), iri-to-uri(\"http://e.org/a b\")", "100%25%20organic http://e.org/a%20b"),
            ("escape-html-uri(\"http://e.org/\u{e9}?a=b c\")", "http://e.org/%C3%A9?a=b c"),
        ],
    );
    assert_eq!(code(&a, "normalize-unicode(\"\u{301}a\", \"FULLY-NORMALIZED\")"), "FOCH0003");
    assert_eq!(code(&a, "normalize-unicode(\"a\", \"NFX\")"), "FOCH0003");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2, GR3] [XQueryFO §7.5]
/// substring matching by the code point collation, by one that compares a
/// character at a time, and by `UNICODE` and a tailoring, whose units are
/// a character and its marks, or a contraction.
#[test]
fn substring_matching_xqueryfo_7_5() {
    let a = orders();
    values(
        &a,
        &[
            (
                "contains(\"tattoo\", \"t\"), contains(\"tattoo\", \"ttt\"), contains(\"\", ()), contains(\"abc\", \"\")",
                "true false true true",
            ),
            ("starts-with(\"tattoo\", \"tat\"), ends-with(\"tattoo\", \"too\"), ends-with((), \"x\")", "true true false"),
            ("substring-before(\"tattoo\", \"attoo\"), substring-after(\"tattoo\", \"tat\"), substring-after(\"tattoo\", \"x\")", "t too "),
            (
                "contains(\"Pen\", \"PEN\", \"urn:turborel:collation:CASE_INSENSITIVE\"), substring-before(\"aXbXc\", \"x\", \"urn:turborel:collation:CASE_INSENSITIVE\")",
                "true a",
            ),
            (
                "contains(\"re&#x301;sume&#x301;\", \"&#xE9;s\", \"urn:turborel:collation:UNICODE\"), contains(\"e&#x301;\", \"e\", \"urn:turborel:collation:UNICODE\")",
                "true false",
            ),
            (
                "declare default collation \"urn:turborel:collation:UNICODE_CS\"; contains(\"chata\", \"c\"), substring-after(\"ochota\", \"ch\")",
                "false ota",
            ),
        ],
    );
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2, GR3] [XQueryFO §7.6]
/// fn:matches, fn:replace and fn:tokenize, with their flags and groups;
/// a pattern that is none err:FORX0002, flags err:FORX0001, a pattern
/// that matches the zero-length string err:FORX0003, a replacement
/// err:FORX0004.
#[test]
fn regular_expressions_xqueryfo_7_6() {
    let a = orders();
    values(
        &a,
        &[
            ("matches(\"abracadabra\", \"bra\"), matches(\"abracadabra\", \"^a.*a$\"), matches(\"ABC\", \"b\", \"i\")", "true true true"),
            ("matches(\"a&#10;b\", \"^b$\", \"m\"), matches(\"a b\", \"a b\", \"x\")", "true false"),
            ("replace(\"abracadabra\", \"bra\", \"*\"), replace(\"abracadabra\", \"a(.)\", \"a$1$1\")", "a*cada* abbraccaddabbra"),
            ("replace(\"darted\", \"^(.*?)d(.*)$\", \"$1c$2\"), replace(\"AAAA\", \"A+?\", \"b\")", "carted bbbb"),
            ("tokenize(\"The cat sat on the mat\", \"\\s+\"), count(tokenize(\"1, 15, 24, 50\", \",\\s*\"))", "The cat sat on the mat 4"),
        ],
    );
    codes(
        &a,
        &[
            ("matches(\"a\", \"(\")", "FORX0002"),
            ("matches(\"a\", \"a\", \"q\")", "FORX0001"),
            ("replace(\"abc\", \"x*\", \"y\")", "FORX0003"),
            ("replace(\"abc\", \"b\", \"$\")", "FORX0004"),
        ],
    );
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2, GR3] [XQueryFO §8.1]
/// fn:resolve-uri against a base given or the static context's, which the
/// prolog declares; none err:FONS0005, a base of no scheme err:FORG0009.
#[test]
fn uris_xqueryfo_8() {
    let a = orders();
    values(
        &a,
        &[
            ("resolve-uri(\"g\", \"http://a/b/c/d;p?q\"), resolve-uri(\"../g\", \"http://a/b/c/d;p?q\")", "http://a/b/c/g http://a/b/g"),
            ("declare base-uri \"http://e.org/dir/f\"; resolve-uri(\"g\"), static-base-uri()", "http://e.org/dir/g http://e.org/dir/f"),
            ("resolve-uri(\"http://x.org/\", \"http://a/\"), empty(resolve-uri((), \"http://a/\"))", "http://x.org/ true"),
        ],
    );
    codes(&a, &[("resolve-uri(\"g\")", "FONS0005"), ("resolve-uri(\"g\", \"no/scheme\")", "FORG0009")]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2, GR3] [XQueryFO §9,
/// §15.1.1] the boolean functions and the effective boolean value: of a
/// node first, a string, a number, NaN; of two atomic values err:FORG0006;
/// booleans ordered false before true.
#[test]
fn booleans_xqueryfo_9() {
    let a = orders();
    values(
        &a,
        &[
            (
                "true(), false(), not(()), not(\"\"), boolean(//item), boolean(0), boolean(xs:double(\"NaN\"))",
                "true false true true true false false",
            ),
            ("false() lt true(), true() eq xs:boolean(\"1\"), boolean(\"false\")", "true true true"),
            ("if (/order/*[3]) then \"three\" else \"fewer\"", "three"),
        ],
    );
    codes(&a, &[("boolean((1, 2))", "FORG0006"), ("not(xs:date(\"2026-01-01\"))", "FORG0006")]);
}

//! XMLCAST [ISO/IEC 9075-14:2006 draft §6.6]: SQL values to XML types by
//! the mapping of §9.8, XML values to SQL types through XQuery's atomic
//! values, and XML values to XML types as CAST takes them.

use turborel::db::{Database, Session};
use turborel::error::{Code, SqlState};

use crate::common::{code, fetched, rows};

/// Schema A, with table T of an INTEGER K and an XML X.
fn database() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, X XML)").unwrap();
    db
}

/// The one value `expression` gives, as shown.
fn one(a: &Session, expression: &str) -> String {
    let sql = format!("VALUES ({expression})");
    let mut all = rows(a, &sql).unwrap_or_else(|c| panic!("{sql}: {c:?}"));
    assert_eq!(all.len(), 1, "{sql}");
    all.remove(0)
}

/// `value`, SQL, cast to XML as `xml` and serialized.
fn to_xml(a: &Session, value: &str, xml: &str) -> String {
    one(a, &format!("XMLSERIALIZE(XMLCAST({value} AS {xml}) AS VARCHAR(100))"))
}

/// `text`, parsed as CONTENT, cast to `sql`.
fn from_xml(a: &Session, text: &str, sql: &str) -> String {
    one(a, &format!("XMLCAST(XMLPARSE(CONTENT '{text}' PRESERVE WHITESPACE) AS {sql})"))
}

/// The SQLSTATE `expression` fails with.
fn failure(a: &Session, expression: &str) -> SqlState {
    let sql = format!("VALUES ({expression})");
    fetched(a, &sql).expect_err(&sql).sqlstate()
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.6 GR3a to GR3c, §9.8 GR9] an
/// SQL value to XML(CONTENT): a document node of the text its lexical
/// form parses to, characters `&`, `<`, `>` mapped to references; a
/// number as CAST writes it, a truth value in lower case, a datetime with
/// T between date and time, an interval as a duration, a binary string
/// in hex; the null value's null.
#[test]
fn sql_values_to_content_6_6() {
    let db = database();
    let a = db.session("A");
    let cases = [
        ("'a<b & c>'", "a&lt;b &amp; c&gt;"),
        ("CAST(17 AS INTEGER)", "17"),
        ("CAST(1.5 AS NUMERIC(5,2))", "1.50"),
        ("CAST(1.5 AS DOUBLE PRECISION)", "1.5E0"),
        ("TRUE", "true"),
        ("DATE '2020-01-02'", "2020-01-02"),
        ("TIMESTAMP '2020-01-02 03:04:05.5'", "2020-01-02T03:04:05.5"),
        ("TIME '03:04:05+05:30'", "03:04:05+05:30"),
        ("INTERVAL '1-2' YEAR TO MONTH", "P01Y02M"),
        ("INTERVAL '3 04:05:06.5' DAY TO SECOND", "P03DT04H05M06.500000S"),
        ("INTERVAL -'90' MINUTE", "-PT90M"),
        ("X'0FB7'", "0FB7"),
        ("CAST(NULL AS INTEGER)", "NULL"),
    ];
    for (value, expected) in cases {
        assert_eq!(to_xml(&a, value, "XML(CONTENT(ANY))"), expected, "{value}");
    }
    assert_eq!(to_xml(&a, "''", "XML(CONTENT(UNTYPED))"), "");
    assert_eq!(one(&a, "XMLCAST('t' AS XML(CONTENT(UNTYPED))) IS CONTENT"), "TRUE");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.6 GR3d, §9.5 GR8] an SQL value
/// to XML(SEQUENCE): one atomic value, of the XML Schema type its SQL type
/// maps to, in canonical form.
#[test]
fn sql_values_to_sequences_6_6() {
    let db = database();
    let a = db.session("A");
    let cases = [
        ("CAST(1.50 AS NUMERIC(5,2))", "1.5"),
        ("CAST(17 AS SMALLINT)", "17"),
        ("CAST(1.5 AS DOUBLE PRECISION)", "1.5"),
        ("CAST(1E7 AS REAL)", "1.0E7"),
        ("INTERVAL '1-2' YEAR TO MONTH", "P1Y2M"),
        ("INTERVAL '36' HOUR", "P1DT12H"),
        ("TIMESTAMP '2020-01-02 03:04:05.50'", "2020-01-02T03:04:05.5"),
        ("TIME '03:04:05+00:00'", "03:04:05Z"),
        ("X'0fb7'", "0FB7"),
        ("' a '", " a "),
    ];
    for (value, expected) in cases {
        assert_eq!(to_xml(&a, value, "XML(SEQUENCE)"), expected, "{value}");
    }
    assert_eq!(one(&a, "XMLCAST(XMLCAST(CAST(17 AS INTEGER) AS XML) AS INTEGER) + 1"), "18");
    assert_eq!(one(&a, "XMLCAST(1 AS XML) IS CONTENT"), "FALSE", "an atomic value is no document node");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.6 GR4b to GR4l] an XML value
/// to an SQL type: its one atomic value, its document node's children
/// atomized, cast as the XML Schema type the SQL type maps to, whitespace
/// collapsed, then CAST to the SQL type; none, the null value.
#[test]
fn xml_values_to_sql_6_6() {
    let db = database();
    let a = db.session("A");
    let cases = [
        ("<a> 42 </a>", "INTEGER", "42"),
        ("<a>3.7</a>", "NUMERIC(5,1)", "3.7"),
        ("<a>3.75</a>", "DOUBLE PRECISION", "3.75E0"),
        ("<a>abc</a>", "VARCHAR(5)", "abc"),
        ("<a>abc  </a>", "CHAR(5)", "abc  "),
        ("<a>x<b>y</b></a>", "CLOB", "xy"),
        ("<!--note-->", "VARCHAR(9)", "note"),
        ("<b>1</b>", "BOOLEAN", "TRUE"),
        ("<x>0fb7</x>", "VARBINARY(4)", "0FB7"),
        ("<d>2020-01-02+05:00</d>", "DATE", "2020-01-01"),
        ("<d>2020-01-02T03:04:05.125-01:00</d>", "TIMESTAMP(3)", "2020-01-02 04:04:05.125"),
        ("<d>2020-01-02T03:04:05+05:30</d>", "TIMESTAMP(0) WITH TIME ZONE", "2020-01-02 03:04:05+05:30"),
        ("<t>24:00:00</t>", "TIME", "00:00:00"),
        ("<d>P14M</d>", "INTERVAL MONTH", "14"),
        ("<d>-PT90M</d>", "INTERVAL MINUTE", "-90"),
        ("", "INTEGER", "NULL"),
    ];
    for (text, sql, expected) in cases {
        assert_eq!(from_xml(&a, text, sql), expected, "{text} AS {sql}");
    }
    a.run("CREATE DOMAIN D AS INTEGER").unwrap();
    assert_eq!(from_xml(&a, "<a>5</a>", "D"), "5", "a domain's data type");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.6 GR4j, GR4k, GR4l] XML
/// values that cast to no SQL value: not of the XML Schema type's lexical
/// forms, or of more than one atomic value, XQuery error 10000; INF to a
/// number, 22003; a year before 0001 in UTC, 22007; a value the facets
/// take that CAST to the SQL type does not, CAST's 22003.
#[test]
fn xml_values_that_cast_to_none_6_6() {
    let db = database();
    let a = db.session("A");
    let parsed = |text: &str| format!("XMLPARSE(CONTENT '{text}' PRESERVE WHITESPACE)");
    let cases = [
        (parsed("<a>3.7</a>"), "INTEGER", SqlState::XQUERY_ERROR),
        (parsed("<a>1</a><b>2</b>"), "INTEGER", SqlState::XQUERY_ERROR),
        (parsed("<a>yes</a>"), "BOOLEAN", SqlState::XQUERY_ERROR),
        (parsed("<a>2021-02-29</a>"), "DATE", SqlState::XQUERY_ERROR),
        (parsed("<a>INF</a>"), "DOUBLE PRECISION", SqlState::NUMERIC_VALUE_OUT_OF_RANGE),
        (parsed("<a>0001-01-01T00:00:00+01:00</a>"), "TIMESTAMP(0)", SqlState::INVALID_DATETIME_FORMAT),
        (parsed("<a>0001-01-01T00:00:00+01:00</a>"), "TIMESTAMP(0) WITH TIME ZONE", SqlState::INVALID_DATETIME_FORMAT),
        (parsed("<a>1234.5</a>"), "NUMERIC(5,2)", SqlState::NUMERIC_VALUE_OUT_OF_RANGE),
    ];
    for (value, sql, state) in cases {
        assert_eq!(failure(&a, &format!("XMLCAST({value} AS {sql})")), state, "{value} AS {sql}");
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.6 GR4e, GR4j, §9.5 GR8a to
/// GR8l] [XQueryFO §17.1.1, §17.4] an XML value is cast as the type §9.5
/// maps the SQL type to, restricted by its facets, so a value they refuse
/// is an XQuery error, SQLSTATE 10000: a CHARACTER's length, a CHARACTER
/// VARYING's and a binary string's maxLength, a NUMERIC's digits, an
/// INTEGER's bounds; a datetime's pattern, matched by its text, or of a
/// type without time zone by its value's canonical form in UTC, so a
/// TIMESTAMP(p) takes a value of p digits of fraction alone; an
/// interval's pattern, of its leading precision's digits and two of each
/// field after.
#[test]
fn xml_values_cast_by_the_facets_6_6() {
    let db = database();
    let a = db.session("A");
    let taken = [
        ("<a>1234.5</a>", "NUMERIC(5,1)", "1234.5"),
        ("<a>99999</a>", "SMALLINT", "99999"),
        ("<a>2026-11-17T09:30:00-05:00</a>", "TIMESTAMP(0)", "2026-11-17 14:30:00"),
        ("<a>2026-11-17T09:30:00.123456-05:00</a>", "TIMESTAMP", "2026-11-17 14:30:00.123456"),
        ("<a>12:00:00.500+05:30</a>", "TIME(3) WITH TIME ZONE", "12:00:00.500+05:30"),
        ("<a>P01Y02M</a>", "INTERVAL YEAR TO MONTH", "1-02"),
        ("<a>P03DT04H05M06.500000S</a>", "INTERVAL DAY TO SECOND", "3 04:05:06.500000"),
    ];
    for (text, sql, expected) in taken {
        assert_eq!(from_xml(&a, text, sql), expected, "{text} AS {sql}");
    }
    let parsed = |text: &str| format!("XMLPARSE(CONTENT '{text}' PRESERVE WHITESPACE)");
    let refused = [
        ("<a>3.75</a>", "NUMERIC(5,1)"),
        ("<a>123456</a>", "SMALLINT"),
        ("<a>abc</a>", "CHAR(5)"),
        ("<a>abcdef</a>", "VARCHAR(5)"),
        ("<a>0FB7AA</a>", "VARBINARY(2)"),
        ("<a>2026-11-17T09:30:00-05:00</a>", "TIMESTAMP"),
        ("<a>2026-11-17T09:30:00.5</a>", "TIMESTAMP(0)"),
        ("<a>12:00:00Z</a>", "TIME WITH TIME ZONE"),
        ("<a>2020-01-02T03:04:05</a>", "TIMESTAMP(0) WITH TIME ZONE"),
        ("<a>P1Y2M</a>", "INTERVAL YEAR TO MONTH"),
        ("<a>P123M</a>", "INTERVAL MONTH"),
    ];
    for (text, sql) in refused {
        assert_eq!(failure(&a, &format!("XMLCAST({} AS {sql})", parsed(text))), SqlState::XQUERY_ERROR, "{text} AS {sql}");
    }
}
/// Verifies: [ISO/IEC 9075-14:2006 draft §6.6 SR4, SR5, SR7, SR9, SR10,
/// SR11, §9.5 GR8, §9.8 GR9a ii] XMLCAST of or to an XML type alone; to no
/// DOCUMENT type of an SQL value; a passing mechanism of XML types alone;
/// no collection, row or structured type; NULL to an XML type; of an XML
/// value to an XML type, CAST's rules, BY REF implicit; a character no
/// XML character, 0N002.
#[test]
fn xmlcast_takes_what_it_maps_6_6() {
    let db = database();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, XMLPARSE(DOCUMENT '<a>1</a>' PRESERVE WHITESPACE))").unwrap();
    for refused in [
        "XMLCAST(1 AS INTEGER)",
        "XMLCAST(1 AS XML(DOCUMENT))",
        "XMLCAST(1 AS XML BY REF)",
        "XMLCAST(NULL AS INTEGER)",
        "XMLCAST(ARRAY[1] AS XML)",
        "XMLCAST(CAST(1 AS DECFLOAT) AS XML)",
        "XMLCAST(XMLPARSE(CONTENT '1' PRESERVE WHITESPACE) AS INTEGER ARRAY)",
        "XMLCAST(XMLPARSE(CONTENT '1' PRESERVE WHITESPACE) AS INTEGER BY VALUE)",
    ] {
        assert_eq!(code(&a, &format!("VALUES ({refused})")), Code::SyntaxRule, "{refused}");
    }
    assert_eq!(one(&a, "XMLSERIALIZE(XMLCAST(NULL AS XML) AS VARCHAR(5))"), "NULL");
    assert_eq!(rows(&a, "SELECT XMLSERIALIZE(XMLCAST(X AS XML(DOCUMENT) BY VALUE) AS VARCHAR(20)) FROM T").unwrap(), ["<a>1</a>"]);
    assert_eq!(code(&a, "SELECT XMLCAST(X AS XML(CONTENT(UNTYPED))) FROM T"), Code::SyntaxRule, "BY REF of SEQUENCE to UNTYPED");
    assert_eq!(rows(&a, "SELECT XMLCAST(X AS INTEGER) FROM T").unwrap(), ["1"]);
    assert_eq!(failure(&a, "XMLCAST('a\u{FFFE}' AS XML)"), SqlState::INVALID_XML_CHARACTER);
    assert_eq!(code(&a, "VALUES (XMLCAST(1 AS XML BY NAME))"), Code::Syntax);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.6 SR10b i, GR3c, GR3d ii,
/// §9.8 GR1a, GR9j] XMLCAST of a value of a distinct type whose source is
/// a collection type, which SR10b i does not refuse, to XML(CONTENT): its
/// source's mapping, an element of each element, a null one nilled; to
/// XML(SEQUENCE), SQLCODE -900, as §9.5 maps no collection to an atomic
/// type; of a collection type itself, SQLCODE -103.
#[test]
fn distinct_collections_to_content_6_6() {
    let db = database();
    let a = db.session("A");
    a.run("CREATE TYPE IDS AS INTEGER ARRAY[3] FINAL").unwrap();
    let nil = r#"<element xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:nil="true"/>"#;
    let cast = "XMLSERIALIZE(XMLCAST(CAST(ARRAY[1, CAST(NULL AS INTEGER)] AS IDS) AS XML(CONTENT)) AS VARCHAR(200))";
    assert_eq!(one(&a, cast), format!("<element>1</element>{nil}"));
    assert_eq!(code(&a, "VALUES (XMLCAST(CAST(ARRAY[1] AS IDS) AS XML))"), Code::Unsupported);
    assert_eq!(code(&a, "VALUES (XMLCAST(ARRAY[1] AS XML(CONTENT)))"), Code::SyntaxRule);
}

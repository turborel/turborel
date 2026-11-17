//! SQL/XML's element constructors [ISO/IEC 9075-14:2006 draft §6.14,
//! §6.15]: XMLELEMENT with XMLATTRIBUTES, XMLFOREST, and XMLNAMESPACES
//! [§11.3, §10.12].

use turborel::db::{Database, Session};
use turborel::error::{Code, SqlState};

use crate::common::{code, fetched, rows, sorted};

/// Schema A: table E of an INTEGER ID, a VARCHAR(20) NAME, a DECIMAL(7,2)
/// PAY, a DATE HIRED and a VARBINARY(4) PHOTO, of rows (1, 'Ann <A&B>',
/// 1200.50, 2020-01-02, X'0FB7') and (2, NULL, NULL, NULL, NULL).
fn employees() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A CREATE TABLE E (ID INTEGER, NAME VARCHAR(20), PAY DECIMAL(7,2), HIRED DATE, PHOTO VARBINARY(4))",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO E VALUES (1, 'Ann <A&B>', 1200.50, DATE '2020-01-02', X'0FB7')").unwrap();
    a.run("INSERT INTO E VALUES (2, NULL, NULL, NULL, NULL)").unwrap();
    a.commit().unwrap();
    db
}

/// Each row's ID and `xml` serialized, in order of ID.
fn serialized(a: &Session, xml: &str) -> Vec<String> {
    sorted(a, &format!("SELECT ID, XMLSERIALIZE(CONTENT {xml} AS VARCHAR(300)) FROM E"))
}

/// The SQLSTATE of the error of `sql`.
fn state(a: &Session, sql: &str) -> SqlState {
    fetched(a, sql).expect_err(sql).sqlstate()
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.14 GR3 to GR6, §10.13, §9.8
/// GR9] XMLELEMENT: an element of the name, its attributes those of
/// XMLATTRIBUTES whose values are not null, its content each value mapped
/// to XML, characters escaped as text, a number, a date and a binary
/// string in hex; a regular identifier's name in upper case.
#[test]
fn xmlelement_6_14() {
    let db = employees();
    let a = db.session("A");
    let made = serialized(&a, "XMLELEMENT(NAME \"emp\", XMLATTRIBUTES(ID AS \"id\", NAME), NAME, ' ', PAY, ' ', HIRED, ' ', PHOTO)");
    assert_eq!(
        made,
        [r#"1,<emp id="1" NAME="Ann &lt;A&amp;B&gt;">Ann &lt;A&amp;B&gt; 1200.50 2020-01-02 0FB7</emp>"#, r#"2,<emp id="2">   </emp>"#]
    );
    assert_eq!(serialized(&a, "XMLELEMENT(NAME emp)"), ["1,<EMP/>", "2,<EMP/>"]);
    let nested = "XMLELEMENT(NAME \"e\", XMLELEMENT(NAME \"n\", NAME), XMLCOMMENT('c'), XMLPARSE(CONTENT '<x/>y' PRESERVE WHITESPACE))";
    assert_eq!(serialized(&a, nested)[0], "1,<e><n>Ann &lt;A&amp;B&gt;</n><!--c--><x/>y</e>");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.14 SR8, §10.13 GR3, GR4] the
/// content options: EMPTY ON NULL, implicit, an empty element of content
/// all null; NULL ON NULL the null value; ABSENT ON NULL nothing; NIL ON
/// NULL and NIL ON NO CONTENT an element of xsi:nil="true".
#[test]
fn content_options_10_13() {
    let db = employees();
    let a = db.session("A");
    assert_eq!(serialized(&a, "XMLELEMENT(NAME \"n\", NAME)")[1], "2,<n/>");
    assert_eq!(serialized(&a, "XMLELEMENT(NAME \"n\", NAME OPTION EMPTY ON NULL)")[1], "2,<n/>");
    assert_eq!(serialized(&a, "XMLELEMENT(NAME \"n\", NAME OPTION NULL ON NULL)")[1], "2,NULL");
    assert_eq!(serialized(&a, "XMLELEMENT(NAME \"n\", NAME OPTION ABSENT ON NULL)")[1], "2,");
    let nil = r#"2,<n xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:nil="true"/>"#;
    assert_eq!(serialized(&a, "XMLELEMENT(NAME \"n\", NAME OPTION NIL ON NULL)")[1], nil);
    assert_eq!(serialized(&a, "XMLELEMENT(NAME \"n\", '' OPTION NIL ON NO CONTENT)")[1], nil);
    assert!(
        serialized(&a, "XMLELEMENT(NAME \"n\", XMLCOMMENT('c') OPTION NIL ON NO CONTENT)")[1].contains("xsi:nil"),
        "a comment is no content"
    );
    assert_eq!(serialized(&a, "XMLELEMENT(NAME \"n\", 'v' OPTION NIL ON NO CONTENT)")[1], "2,<n>v</n>");
    assert_eq!(serialized(&a, "XMLELEMENT(NAME \"n\", NAME, 'v' OPTION NULL ON NULL)")[1], "2,<n>v</n>", "not every value is null");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.15 SR3, GR3 to GR7] XMLFOREST:
/// an element of each value, named by AS or its column, NULL ON NULL
/// implicit, the elements concatenated; the null value if each is.
#[test]
fn xmlforest_6_15() {
    let db = employees();
    let a = db.session("A");
    assert_eq!(
        serialized(&a, "XMLFOREST(ID AS \"id\", NAME, PAY)"),
        ["1,<id>1</id><NAME>Ann &lt;A&amp;B&gt;</NAME><PAY>1200.50</PAY>", "2,<id>2</id>"]
    );
    assert_eq!(serialized(&a, "XMLFOREST(NAME, PAY)")[1], "2,NULL");
    assert_eq!(serialized(&a, "XMLFOREST(NAME, PAY OPTION EMPTY ON NULL)")[1], "2,<NAME/><PAY/>");
    assert_eq!(rows(&a, "SELECT ID FROM E WHERE XMLFOREST(ID) IS CONTENT").unwrap().len(), 2, "RETURNING CONTENT implicit");
    assert_eq!(rows(&a, "SELECT ID FROM E WHERE XMLFOREST(ID RETURNING SEQUENCE) IS CONTENT").unwrap().len(), 0);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §11.3, §10.12, §6.14 SR1, SR3g,
/// SR6] [XQuery §3.7.4] XMLNAMESPACES: a prefix's URI the innermost
/// declaration's in scope, an element within another's scope; the
/// default namespace that of an unprefixed name; NO DEFAULT none; xml,
/// xs, xsi and sqlxml predeclared; each name's binding declared where it
/// is used.
#[test]
fn namespaces_11_3() {
    let db = employees();
    let a = db.session("A");
    let p = "XMLELEMENT(NAME \"p:a\", XMLNAMESPACES('u' AS \"p\", DEFAULT 'd'), XMLATTRIBUTES(ID AS \"p:id\"), XMLELEMENT(NAME \"b\"), XMLELEMENT(NAME \"p:c\", XMLNAMESPACES('w' AS \"p\", NO DEFAULT), XMLELEMENT(NAME \"e\")))";
    assert_eq!(serialized(&a, p)[0], r#"1,<p:a xmlns:p="u" p:id="1"><b xmlns="d"/><p:c xmlns:p="w"><e/></p:c></p:a>"#);
    let predeclared = "XMLELEMENT(NAME \"xs:a\", XMLATTRIBUTES(ID AS \"xml:lang\", ID AS \"sqlxml:x\"))";
    assert_eq!(
        serialized(&a, predeclared)[0],
        r#"1,<xs:a xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:sqlxml="http://standards.iso.org/iso/9075/2003/sqlxml" xml:lang="1" sqlxml:x="1"/>"#
    );
    let forest = "XMLFOREST(XMLNAMESPACES(DEFAULT 'd'), ID AS \"i\")";
    assert_eq!(serialized(&a, forest)[0], r#"1,<i xmlns="d">1</i>"#);
    let sub = "XMLELEMENT(NAME \"p:a\", XMLNAMESPACES('u' AS \"p\"), (SELECT XMLELEMENT(NAME \"p:b\") FROM E WHERE ID = 1))";
    assert_eq!(serialized(&a, sub)[0], r#"1,<p:a xmlns:p="u"><p:b/></p:a>"#, "a subquery is in the scope");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.14 SR3 to SR9, §6.15 SR2,
/// §11.3 SR2 to SR7, §10.12 SR2a ii 2] the names and declarations
/// refused, SQLCODE -103: a name no QName or of a prefix no declaration
/// binds; an attribute or forest element of no column named by no AS; an
/// attribute xmlns, of the prefix xmlns, or of XML, or twice; xsi:nil of
/// an element of content; a declaration of two defaults, a prefix twice,
/// xml or xmlns, no NCName, a URI empty or reserved.
#[test]
fn names_and_declarations_refused() {
    let db = employees();
    let a = db.session("A");
    for refused in [
        "XMLELEMENT(NAME \"1a\")",
        "XMLELEMENT(NAME \"q:a\")",
        "XMLELEMENT(NAME \"a\", XMLATTRIBUTES(ID + 1))",
        "XMLFOREST(ID + 1)",
        "XMLELEMENT(NAME \"a\", XMLATTRIBUTES(ID AS \"xmlns\"))",
        "XMLELEMENT(NAME \"a\", XMLATTRIBUTES(ID AS \"xmlns:b\"))",
        "XMLELEMENT(NAME \"a\", XMLATTRIBUTES(XMLCOMMENT('c') AS \"b\"))",
        "XMLELEMENT(NAME \"a\", XMLATTRIBUTES(ID AS \"b\", NAME AS \"b\"))",
        "XMLELEMENT(NAME \"a\", XMLNAMESPACES('u' AS \"p\"), XMLATTRIBUTES(ID AS \"p:b\", NAME AS \"p:b\"))",
        "XMLELEMENT(NAME \"a\", XMLATTRIBUTES(ID AS \"xsi:nil\"), 'v')",
        "XMLELEMENT(NAME \"a\", XMLNAMESPACES(DEFAULT 'u', NO DEFAULT))",
        "XMLELEMENT(NAME \"a\", XMLNAMESPACES('u' AS \"p\", 'w' AS \"p\"))",
        "XMLELEMENT(NAME \"a\", XMLNAMESPACES('u' AS \"xml\"))",
        "XMLELEMENT(NAME \"a\", XMLNAMESPACES('u' AS \"xmlns\"))",
        "XMLELEMENT(NAME \"a\", XMLNAMESPACES('u' AS \"a:b\"))",
        "XMLELEMENT(NAME \"a\", XMLNAMESPACES('' AS \"p\"))",
        "XMLELEMENT(NAME \"a\", XMLNAMESPACES('http://www.w3.org/2000/xmlns/' AS \"p\"))",
        "XMLELEMENT(NAME \"a\", XMLNAMESPACES(DEFAULT 'http://www.w3.org/XML/1998/namespace'))",
    ] {
        assert_eq!(code(&a, &format!("SELECT {refused} FROM E")), Code::SyntaxRule, "{refused}");
    }
    assert!(fetched(&a, "SELECT XMLELEMENT(NAME \"a\", XMLATTRIBUTES(ID AS \"xsi:nil\")) FROM E").is_ok(), "of no content");
    assert_eq!(code(&a, "SELECT XMLELEMENT(NAME \"a\" OPTION NULL ON NULL) FROM E"), Code::Syntax, "an option of content alone");
    assert_eq!(code(&a, "SELECT XMLFOREST(XMLNAMESPACES(DEFAULT 'u')) FROM E"), Code::Syntax);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.13 GR4c i 1 B, §9.8 GR9a]
/// [XQuery §3.7.1.3] content mapped to XML is parsed as CONTENT, so a
/// character XML 1.0 text does not hold is 2200N; an attribute of an XML
/// value after other content is err:XQTY0024, SQLSTATE 10000.
#[test]
fn content_errors_10_13() {
    let db = employees();
    let a = db.session("A");
    assert_eq!(state(&a, "SELECT XMLELEMENT(NAME \"a\", '\u{FFFE}') FROM E"), SqlState::INVALID_XML_CHARACTER);
    assert_eq!(state(&a, "SELECT XMLELEMENT(NAME \"a\", '\u{1}') FROM E"), SqlState::INVALID_XML_CONTENT);
}

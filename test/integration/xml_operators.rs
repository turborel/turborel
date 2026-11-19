//! SQL/XML's operators beyond parsing and serializing [ISO/IEC
//! 9075-14:2006 draft]: the document and content predicates and
//! XMLDOCUMENT.

use turborel::db::{Database, Session};
use turborel::error::{Code, SqlState};

use crate::common::{code, fetched, rows, sorted};

/// Schema A: table T of an INTEGER K and an XML X, holding a document (1),
/// content of two elements (2), text (3), a comment beside an element (4),
/// and the null value (5).
fn values() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, X XML)").unwrap();
    let a = db.session("A");
    for (k, text) in [(1, "<a/>"), (2, "<a/><b/>"), (3, "t"), (4, "<!--c--><a/><?p d?>")] {
        a.run(&format!("INSERT INTO T VALUES ({k}, XMLPARSE(CONTENT '{text}' PRESERVE WHITESPACE))")).unwrap();
    }
    a.run("INSERT INTO T VALUES (5, NULL)").unwrap();
    a.commit().unwrap();
    db
}

/// The keys of the rows of T `condition` is true of, in order.
fn keys(a: &Session, condition: &str) -> Vec<String> {
    sorted(a, &format!("SELECT K FROM T WHERE {condition}"))
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §8.3 GR2] IS DOCUMENT is True of
/// one document node whose children are one element and comments and
/// processing instructions, Unknown of the null value, else False; IS NOT
/// DOCUMENT is its negation \[SR2\].
#[test]
fn the_document_predicate_8_3() {
    let db = values();
    let a = db.session("A");
    assert_eq!(keys(&a, "X IS DOCUMENT"), ["1", "4"]);
    assert_eq!(keys(&a, "X IS NOT DOCUMENT"), ["2", "3"]);
    assert_eq!(keys(&a, "(X IS DOCUMENT) IS UNKNOWN"), ["5"]);
    assert_eq!(keys(&a, "NOT X IS DOCUMENT"), ["2", "3"]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §8.2 GR2] IS CONTENT is True of
/// one document node, Unknown of the null value, else False, as of a
/// sequence of an element alone, not under a document node; IS NOT CONTENT
/// is its negation \[SR2\].
#[test]
fn the_content_predicate_8_2() {
    let db = values();
    let a = db.session("A");
    assert_eq!(keys(&a, "X IS CONTENT"), ["1", "2", "3", "4"]);
    assert_eq!(keys(&a, "X IS NOT CONTENT"), Vec::<String>::new());
    assert_eq!(keys(&a, "(X IS NOT CONTENT) IS UNKNOWN"), ["5"]);
    assert_eq!(keys(&a, "XMLDOCUMENT(X RETURNING SEQUENCE) IS CONTENT"), ["1", "2", "3", "4"]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §8.2 SR1, §8.3 SR1] the operand
/// of either predicate is one XML value expression, else SQLCODE -103; a
/// column named DOCUMENT or CONTENT is still a column.
#[test]
fn the_predicates_take_an_xml_value() {
    let db = values();
    let a = db.session("A");
    for refused in ["K IS DOCUMENT", "'<a/>' IS CONTENT", "(X, X) IS DOCUMENT", "K IS NOT CONTENT"] {
        assert_eq!(code(&a, &format!("SELECT K FROM T WHERE {refused}")), Code::SyntaxRule, "{refused}");
    }
    assert_eq!(code(&a, "SELECT K FROM T WHERE X IS DOCUMENTS"), Code::Syntax);
    a.run("CREATE TABLE N (DOCUMENT XML, CONTENT INTEGER)").unwrap();
    a.run("INSERT INTO N VALUES (XMLPARSE(DOCUMENT '<n/>' PRESERVE WHITESPACE), 1)").unwrap();
    assert_eq!(rows(&a, "SELECT CONTENT FROM N WHERE DOCUMENT IS DOCUMENT AND DOCUMENT IS NOT NULL").unwrap(), ["1"]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.4] [ISO/IEC 9075-2:2023 §6.12
/// SR2f ii] IS \[NOT\] DOCUMENT and IS \[NOT\] CONTENT as the `<when
/// operand>` of a simple CASE, each the predicate of the case operand.
#[test]
fn the_predicates_as_when_operands_6_4() {
    let db = values();
    let a = db.session("A");
    let cases = "SELECT K, CASE X WHEN IS DOCUMENT THEN 'd' WHEN IS NOT CONTENT THEN 'n' WHEN IS CONTENT THEN 'c' ELSE 'u' END FROM T";
    assert_eq!(sorted(&a, cases), ["1,d", "2,c", "3,c", "4,d", "5,u"]);
    assert_eq!(code(&a, "SELECT CASE K WHEN IS DOCUMENT THEN 1 END FROM T"), Code::SyntaxRule);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.13 SR1, SR3, GR2, GR8, GR9]
/// XMLDOCUMENT: a new document node of the value's nodes, a document
/// node's children in its place; the null value of the null value; of
/// XML(CONTENT(UNTYPED)) by RETURNING CONTENT, implicit, and of
/// XML(SEQUENCE) by RETURNING SEQUENCE.
#[test]
fn xmldocument_6_13() {
    let db = values();
    let a = db.session("A");
    let documents = "SELECT K, XMLSERIALIZE(XMLDOCUMENT(X) AS VARCHAR(40)) FROM T";
    assert_eq!(sorted(&a, documents), ["1,<a/>", "2,<a/><b/>", "3,t", "4,<!--c--><a/><?p d?>", "5,NULL"]);
    assert_eq!(keys(&a, "XMLDOCUMENT(X) IS DOCUMENT"), ["1", "4"]);
    a.run("CREATE TABLE U (U XML(CONTENT(UNTYPED)), S XML(SEQUENCE), D XML(DOCUMENT))").unwrap();
    a.run("INSERT INTO U (U) SELECT XMLDOCUMENT(X) FROM T").unwrap();
    a.run("INSERT INTO U (U) SELECT XMLDOCUMENT(X RETURNING CONTENT) FROM T").unwrap();
    let untyped = a.run("INSERT INTO U (U) SELECT XMLDOCUMENT(X RETURNING SEQUENCE) FROM T").unwrap_err();
    assert_eq!(untyped.code, Code::SyntaxRule, "XML(SEQUENCE) is no UNTYPED type: {untyped}");
    a.run("INSERT INTO U (S) SELECT XMLDOCUMENT(X RETURNING SEQUENCE) FROM T").unwrap();
    a.run("INSERT INTO U (D) SELECT XMLDOCUMENT(X) FROM T WHERE K = 1").unwrap();
    let error = a.run("INSERT INTO U (D) SELECT XMLDOCUMENT(X) FROM T WHERE K = 2").unwrap_err();
    assert_eq!(error.sqlstate(), SqlState::NOT_AN_XML_DOCUMENT, "{error}");
    let nested = "SELECT XMLSERIALIZE(XMLDOCUMENT(XMLDOCUMENT(X)) AS VARCHAR(20)) FROM T WHERE K = 2";
    assert_eq!(rows(&a, nested).unwrap(), ["<a/><b/>"]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.13, §6.9 SR1, §11.4]
/// XMLDOCUMENT takes an XML value alone, SQLCODE -103, and its returning
/// clause is CONTENT or SEQUENCE.
#[test]
fn xmldocument_takes_an_xml_value() {
    let db = values();
    let a = db.session("A");
    assert_eq!(code(&a, "SELECT XMLDOCUMENT(K) FROM T"), Code::SyntaxRule);
    assert_eq!(code(&a, "SELECT XMLDOCUMENT('<a/>') FROM T"), Code::SyntaxRule);
    assert_eq!(code(&a, "SELECT XMLDOCUMENT(X RETURNING DOCUMENT) FROM T"), Code::Syntax);
    assert_eq!(code(&a, "SELECT XMLDOCUMENT(X RETURNING) FROM T"), Code::Syntax);
    assert_eq!(fetched(&a, "SELECT XMLDOCUMENT(X) FROM T").unwrap(), 5);
}

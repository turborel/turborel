//! SQL/XML [ISO/IEC 9075-14:2006 draft]: the XML type, its values kept in
//! columns, XMLPARSE and XMLSERIALIZE, CAST, and the rules that keep XML
//! values from being compared. Storage and descriptors are in
//! `xml_storage.rs`.

use turborel::db::{Database, Session};
use turborel::error::{Code, SqlError, SqlState};

use crate::common::{code, fetched, rows, sorted};

/// Schema A: table T of an INTEGER K and an XML X, and table D of a
/// DOCUMENT, an UNTYPED CONTENT and a SEQUENCE column.
fn documents() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, X XML) \
         CREATE TABLE D (DOC XML(DOCUMENT), UN XML(CONTENT(UNTYPED)), SEQ XML(SEQUENCE))",
    )
    .unwrap();
    db
}

/// The error `sql`, a query, fails with.
fn failure(a: &Session, sql: &str) -> SqlError {
    fetched(a, sql).expect_err(sql)
}

/// `text` parsed as `syntax`, as an XMLPARSE literal.
fn parsed(syntax: &str, text: &str) -> String {
    format!("XMLPARSE({syntax} '{}' PRESERVE WHITESPACE)", text.replace('\'', "''"))
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.1, §6.8, §6.16, §10.15,
/// §10.16] an XML column keeps the values XMLPARSE makes, the null value
/// too, and XMLSERIALIZE writes them back as text.
#[test]
fn values_are_kept_and_serialized() {
    let db = documents();
    let a = db.session("A");
    a.run(&format!(
        "INSERT INTO T VALUES (1, {}), (2, {}), (3, NULL)",
        parsed("DOCUMENT", "<a x=\"1\">t</a>"),
        parsed("CONTENT", "one<b/>two")
    ))
    .unwrap();
    assert_eq!(sorted(&a, "SELECT K, XMLSERIALIZE(CONTENT X AS VARCHAR(100)) FROM T"), ["1,<a x=\"1\">t</a>", "2,one<b/>two", "3,NULL"]);
    assert_eq!(rows(&a, "SELECT X FROM T WHERE K = 1").unwrap(), ["<a x=\"1\">t</a>"], "a value is shown as its serialization");
    a.run(&format!("UPDATE T SET X = {} WHERE K = 3", parsed("CONTENT", "<c/>"))).unwrap();
    assert_eq!(rows(&a, "SELECT XMLSERIALIZE(X AS VARCHAR(10)) FROM T WHERE K = 3").unwrap(), ["<c/>"]);
    a.commit().unwrap();
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.1 SR1 to SR3, §4.2.2] [DESIGN]
/// RULES.md D86: the XML types, as written and as described: XML alone is
/// XML(SEQUENCE), and DOCUMENT or CONTENT alone is ANY; SEQUENCE takes no
/// secondary modifier; XMLSCHEMA is valid SQL not yet taken.
#[test]
fn the_xml_types_6_1() {
    let db = documents();
    let a = db.session("A");
    a.run("CREATE TABLE U (A XML, B XML(SEQUENCE), C XML(DOCUMENT), D XML(CONTENT(ANY)), E xml(document(untyped)))").unwrap();
    let described = "SELECT COLUMN_NAME, DATA_TYPE FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'U'";
    assert_eq!(sorted(&a, described), ["A,XML", "B,XML", "C,XML", "D,XML", "E,XML"]);
    assert_eq!(a.run("CREATE TABLE V (A XML(SEQUENCE(ANY)))").unwrap_err().code, Code::Syntax);
    assert_eq!(a.run("CREATE TABLE V (A XML(TREE))").unwrap_err().code, Code::Syntax);
    for schema in ["ID S.X", "URI 'u' LOCATION 'l' ELEMENT E", "NO NAMESPACE NAMESPACE 'n'", "ID X NO NAMESPACE ELEMENT E"] {
        let statement = format!("CREATE TABLE V (A XML(DOCUMENT(XMLSCHEMA {schema})))");
        assert_eq!(a.run(&statement).unwrap_err().code, Code::Unsupported, "{statement}");
    }
    assert_eq!(a.run("CREATE TABLE V (A XML(DOCUMENT(XMLSCHEMA URI)))").unwrap_err().code, Code::Syntax);
    assert_eq!(a.run("CREATE TABLE V (A XML(CONTENT(NONE)))").unwrap_err().code, Code::Syntax);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.2 SR1b, GR1a] a value stored
/// in an XML(DOCUMENT) column is a document, else not an XML document,
/// 2200L; an UNTYPED column takes values of an UNTYPED type alone; one of
/// another type takes any XML value; no other value is stored in one.
#[test]
fn store_assignment_10_2() {
    let db = documents();
    let a = db.session("A");
    a.run(&format!("INSERT INTO D VALUES ({}, {}, {})", parsed("CONTENT", "<a/>"), parsed("DOCUMENT", "<b/>"), parsed("CONTENT", "x")))
        .unwrap();
    let error = a.run(&format!("INSERT INTO D (DOC) VALUES ({})", parsed("CONTENT", "<a/><b/>"))).unwrap_err();
    assert_eq!(error.sqlstate(), SqlState::NOT_AN_XML_DOCUMENT, "{error}");
    a.run(&format!("INSERT INTO T VALUES (1, {})", parsed("CONTENT", "<a/>"))).unwrap();
    assert_eq!(a.run("INSERT INTO D (UN) SELECT X FROM T").unwrap_err().code, Code::SyntaxRule, "SEQUENCE into UNTYPED");
    a.run("INSERT INTO D (SEQ, DOC) SELECT X, X FROM T").unwrap();
    assert_eq!(a.run("INSERT INTO T (X) VALUES ('<a/>')").unwrap_err().code, Code::SyntaxRule, "no character string is stored as XML");
    assert_eq!(a.run("INSERT INTO T (K) SELECT X FROM T").unwrap_err().code, Code::SyntaxRule);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.16, §10.16 GR6, GR8] XMLPARSE
/// of a character or binary string, a large object's too: a document, or
/// content; invalid ones refused, 2200M and 2200N; white space stripped
/// as STRIP WHITESPACE says; the null value's null.
#[test]
fn xmlparse_6_16() {
    let db = documents();
    let a = db.session("A");
    let serialized = |inner: &str| format!("SELECT XMLSERIALIZE(CONTENT {inner} AS VARCHAR(100)) FROM (VALUES (1)) AS V (N)");
    assert_eq!(rows(&a, &serialized("XMLPARSE(DOCUMENT ' <a> <b> </b> </a> ' STRIP WHITESPACE)")).unwrap(), ["<a><b/></a>"]);
    assert_eq!(rows(&a, &serialized("XMLPARSE(CONTENT ' <a> </a> ' PRESERVE WHITESPACE)")).unwrap(), [" <a> </a> "]);
    assert_eq!(rows(&a, &serialized("XMLPARSE(CONTENT CAST('<c/>' AS CLOB) PRESERVE WHITESPACE)")).unwrap(), ["<c/>"]);
    assert_eq!(rows(&a, &serialized("XMLPARSE(DOCUMENT X'3C642F3E' PRESERVE WHITESPACE)")).unwrap(), ["<d/>"]);
    assert_eq!(rows(&a, &serialized("XMLPARSE(DOCUMENT CAST(NULL AS VARCHAR(5)) PRESERVE WHITESPACE)")).unwrap(), ["NULL"]);
    assert_eq!(failure(&a, &serialized("XMLPARSE(DOCUMENT 'a<b/>' PRESERVE WHITESPACE)")).sqlstate(), SqlState::INVALID_XML_DOCUMENT);
    assert_eq!(failure(&a, &serialized("XMLPARSE(CONTENT '<a>' PRESERVE WHITESPACE)")).sqlstate(), SqlState::INVALID_XML_CONTENT);
    assert_eq!(code(&a, &serialized("XMLPARSE(CONTENT 1 PRESERVE WHITESPACE)")), Code::SyntaxRule);
    assert_eq!(code(&a, &serialized("XMLPARSE('<a/>' PRESERVE WHITESPACE)")), Code::Syntax);
    assert_eq!(code(&a, &serialized("XMLPARSE(CONTENT '<a/>' KEEP WHITESPACE)")), Code::Syntax);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.8, §10.15] XMLSERIALIZE: as
/// CONTENT if neither is written; as DOCUMENT, of a document alone, else
/// 2200L; of version 1.0 or 1.1; an XML declaration as asked, and of a
/// 1.1 document unasked; to a character or a binary string, in UTF-8 or
/// UTF-16, stored as its type takes it, too long a string data, right
/// truncation; of an XML value alone.
#[test]
fn xmlserialize_6_8() {
    let db = documents();
    let a = db.session("A");
    a.run(&format!("INSERT INTO T VALUES (1, {}), (2, {})", parsed("DOCUMENT", "<a/>"), parsed("CONTENT", "<a/><b/>"))).unwrap();
    let one = |options: &str| rows(&a, &format!("SELECT XMLSERIALIZE({options}) FROM T WHERE K = 1")).unwrap();
    assert_eq!(one("DOCUMENT X AS VARCHAR(50)"), ["<a/>"]);
    assert_eq!(one("X AS VARCHAR(50) INCLUDING XMLDECLARATION"), ["<?xml version=\"1.0\" encoding=\"UTF-8\"?><a/>"]);
    assert_eq!(one("DOCUMENT X AS VARCHAR(50) VERSION '1.1'"), ["<?xml version=\"1.1\" encoding=\"UTF-8\"?><a/>"]);
    assert_eq!(one("DOCUMENT X AS VARCHAR(50) VERSION '1.1' EXCLUDING XMLDECLARATION"), ["<a/>"]);
    assert_eq!(one("CONTENT X AS CLOB"), ["<a/>"]);
    assert_eq!(one("X AS VARBINARY(20)"), ["3C612F3E"]);
    assert_eq!(one("X AS VARBINARY(20) ENCODING utf16"), ["FEFF003C0061002F003E"]);
    assert_eq!(one("X AS BLOB ENCODING UTF8"), ["3C612F3E"]);
    let not_document = failure(&a, "SELECT XMLSERIALIZE(DOCUMENT X AS VARCHAR(50)) FROM T WHERE K = 2");
    assert_eq!(not_document.sqlstate(), SqlState::NOT_AN_XML_DOCUMENT, "{not_document}");
    assert_eq!(failure(&a, "SELECT XMLSERIALIZE(X AS VARCHAR(2)) FROM T WHERE K = 1").sqlstate(), SqlState::STRING_DATA_RIGHT_TRUNCATION);
    for refused in [
        "SELECT XMLSERIALIZE(X AS VARCHAR(50) VERSION '1.2') FROM T",
        "SELECT XMLSERIALIZE(X AS INTEGER) FROM T",
        "SELECT XMLSERIALIZE(X AS VARCHAR(50) ENCODING UTF8) FROM T",
        "SELECT XMLSERIALIZE(X AS BLOB ENCODING LATIN1) FROM T",
        "SELECT XMLSERIALIZE(K AS VARCHAR(50)) FROM T",
    ] {
        assert_eq!(code(&a, refused), Code::SyntaxRule, "{refused}");
    }
    a.run("CREATE TABLE N (DOCUMENT XML, CONTENT INTEGER)").unwrap();
    a.run(&format!("INSERT INTO N VALUES ({}, 1)", parsed("CONTENT", "<n/>"))).unwrap();
    assert_eq!(rows(&a, "SELECT XMLSERIALIZE(DOCUMENT AS VARCHAR(9)) FROM N").unwrap(), ["<n/>"], "a column named DOCUMENT");
    assert_eq!(rows(&a, "SELECT XMLSERIALIZE(DOCUMENT DOCUMENT AS VARCHAR(9)) FROM N").unwrap(), ["<n/>"]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §4.2.4, §10.8 SR1, §10.9 SR1,
/// §10.11 SR1] XML values are not comparable: no equality, grouping or
/// ordering operation takes one, a key, a join's common column or a set
/// operation's but UNION ALL.
#[test]
fn values_are_not_compared() {
    let db = documents();
    let a = db.session("A");
    let refused = [
        "SELECT K FROM T WHERE X = X",
        "SELECT K FROM T WHERE X IN (X, X)",
        "SELECT K FROM T WHERE X = ANY (SELECT X FROM T)",
        "SELECT K FROM T WHERE (K, X) = (1, X)",
        "SELECT K FROM T WHERE X IS DISTINCT FROM X",
        "SELECT NULLIF(X, X) FROM T",
        "SELECT CASE X WHEN X THEN 1 END FROM T",
        "SELECT K FROM T WHERE X MATCH (SELECT X FROM T)",
        "SELECT X FROM T GROUP BY X",
        "SELECT X FROM T ORDER BY X",
        "SELECT DISTINCT X FROM T",
        "SELECT COUNT(DISTINCT X) FROM T",
        "SELECT MAX(X) FROM T",
        "SELECT X FROM T UNION SELECT X FROM T",
        "SELECT K FROM T WHERE UNIQUE (SELECT X FROM T)",
        "SELECT * FROM T NATURAL JOIN T AS U",
        "SELECT * FROM T JOIN T AS U USING (X)",
    ];
    for statement in refused {
        let error = failure(&a, statement);
        assert_eq!(error.code, Code::SyntaxRule, "{statement}: {error}");
        assert!(error.to_string().contains("ISO/IEC 9075-14:2006 draft §10."), "{statement}: {error}");
    }
    assert_eq!(rows(&a, "SELECT X FROM T UNION ALL SELECT X FROM T").unwrap(), Vec::<String>::new());
    for keyed in ["CREATE TABLE K1 (X XML PRIMARY KEY)", "CREATE TABLE K2 (X XML UNIQUE)"] {
        assert_eq!(a.run(keyed).unwrap_err().code, Code::SyntaxRule, "{keyed}");
    }
    a.commit().unwrap();
    assert_eq!(a.run("CREATE INDEX I ON T (X)").unwrap_err().code, Code::SyntaxRule);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.5 SR1 to SR4, GR1] [DESIGN]
/// RULES.md D86: CAST takes an XML value to an XML type alone, and no other
/// value to one; BY REF, implicit, to an UNTYPED type of an UNTYPED one;
/// BY VALUE, a copy; to a DOCUMENT type, a document, else 2200L; NULL to
/// any.
#[test]
fn cast_6_5() {
    let db = documents();
    let a = db.session("A");
    a.run(&format!("INSERT INTO T VALUES (1, {}), (2, {})", parsed("DOCUMENT", "<a/>"), parsed("CONTENT", "x"))).unwrap();
    let cast = |sql: &str| rows(&a, &format!("SELECT XMLSERIALIZE(CAST({sql}) AS VARCHAR(20)) FROM T WHERE K = 1"));
    assert_eq!(cast("X AS XML(DOCUMENT)").unwrap(), ["<a/>"]);
    assert_eq!(cast("X AS XML(CONTENT(UNTYPED)) BY VALUE").unwrap(), ["<a/>"]);
    assert_eq!(cast("X AS XML BY REF").unwrap(), ["<a/>"]);
    assert_eq!(cast("NULL AS XML").unwrap(), ["NULL"]);
    assert_eq!(cast("X AS XML(CONTENT(UNTYPED))").err(), Some(Code::SyntaxRule), "BY REF to UNTYPED of SEQUENCE");
    let not_document = failure(&a, "SELECT XMLSERIALIZE(CAST(X AS XML(DOCUMENT)) AS VARCHAR(9)) FROM T WHERE K = 2");
    assert_eq!(not_document.sqlstate(), SqlState::NOT_AN_XML_DOCUMENT, "{not_document}");
    for refused in [
        "SELECT CAST(X AS VARCHAR(10)) FROM T",
        "SELECT CAST('<a/>' AS XML) FROM T",
        "SELECT CAST(K AS XML BY REF) FROM T",
        "SELECT CAST(NULL AS XML BY VALUE) FROM T",
        "SELECT CAST(X AS VARCHAR(9) BY VALUE) FROM T",
    ] {
        assert_eq!(code(&a, refused), Code::SyntaxRule, "{refused}");
    }
    assert_eq!(code(&a, "SELECT CAST(X AS XML BY NAME) FROM T"), Code::Syntax);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.3 SR1] values brought
/// together: XML types with XML types alone, of the type §10.3 gives.
#[test]
fn types_brought_together_10_3() {
    let db = documents();
    let a = db.session("A");
    a.run(&format!("INSERT INTO D VALUES ({}, {}, {})", parsed("DOCUMENT", "<a/>"), parsed("CONTENT", "u"), parsed("CONTENT", "s")))
        .unwrap();
    let both = "SELECT XMLSERIALIZE(CASE WHEN 1 = 1 THEN UN ELSE {} END AS VARCHAR(9)) FROM D";
    assert_eq!(rows(&a, &both.replace("{}", "DOC")).unwrap(), ["u"]);
    assert_eq!(rows(&a, &both.replace("{}", "SEQ")).unwrap(), ["u"]);
    a.run("CREATE TABLE E (U XML(CONTENT(UNTYPED)))").unwrap();
    a.run("INSERT INTO E SELECT CASE WHEN 1 = 1 THEN UN ELSE UN END FROM D").unwrap();
    assert_eq!(a.run("INSERT INTO E SELECT CASE WHEN 1 = 1 THEN UN ELSE SEQ END FROM D").unwrap_err().code, Code::SyntaxRule);
    let any = a.run("INSERT INTO E SELECT CASE WHEN 1 = 1 THEN UN ELSE DOC END FROM D").unwrap_err();
    assert_eq!(any.code, Code::SyntaxRule, "UNTYPED with ANY is CONTENT(ANY): {any}");
    assert_eq!(code(&a, "SELECT CASE WHEN 1 = 1 THEN UN ELSE 'a' END FROM D"), Code::SyntaxRule);
    assert_eq!(
        rows(&a, "SELECT XMLSERIALIZE(X AS VARCHAR(9)) FROM (SELECT UN FROM D UNION ALL SELECT DOC FROM D) AS V (X)").unwrap().len(),
        2
    );
}

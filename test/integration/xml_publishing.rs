//! SQL/XML's publishing operators [ISO/IEC 9075-14:2006 draft §6]:
//! XMLCOMMENT, XMLCONCAT, XMLPI and XMLTEXT.

use turborel::db::{Database, Session};
use turborel::error::{Code, SqlState};

use crate::common::{code, fetched, rows, sorted};

/// Schema A: table T of an INTEGER K, a VARCHAR(20) S and an XML X, of one
/// row (1, 'a b', <a/>) and one (2, NULL, NULL).
fn values() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, S VARCHAR(20), X XML)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 'a b', XMLPARSE(CONTENT '<a/>' PRESERVE WHITESPACE))").unwrap();
    a.run("INSERT INTO T VALUES (2, NULL, NULL)").unwrap();
    a.commit().unwrap();
    db
}

/// Each row's K and `xml` serialized, in order of K.
fn serialized(a: &Session, xml: &str) -> Vec<String> {
    sorted(a, &format!("SELECT K, XMLSERIALIZE(CONTENT {xml} AS VARCHAR(100)) FROM T"))
}

/// The SQLSTATE of the error of `sql`.
fn state(a: &Session, sql: &str) -> SqlState {
    fetched(a, sql).expect_err(sql).sqlstate()
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.11 SR1, SR3, GR2 to GR5]
/// XMLCOMMENT: a comment node of the string; the null value of the null
/// value; one of two hyphens together or a hyphen last 2200S; of
/// XML(CONTENT(UNTYPED)), a document node, RETURNING CONTENT, implicit,
/// and of XML(SEQUENCE) RETURNING SEQUENCE.
#[test]
fn xmlcomment_6_11() {
    let db = values();
    let a = db.session("A");
    assert_eq!(serialized(&a, "XMLCOMMENT(S)"), ["1,<!--a b-->", "2,NULL"]);
    assert_eq!(serialized(&a, "XMLCOMMENT(S RETURNING SEQUENCE)"), ["1,<!--a b-->", "2,NULL"]);
    assert_eq!(rows(&a, "SELECT K FROM T WHERE XMLCOMMENT(S) IS CONTENT").unwrap(), ["1"]);
    assert_eq!(rows(&a, "SELECT K FROM T WHERE XMLCOMMENT(S RETURNING SEQUENCE) IS CONTENT").unwrap(), Vec::<String>::new());
    for bad in ["a--b", "a-"] {
        assert_eq!(state(&a, &format!("SELECT XMLCOMMENT('{bad}') FROM T")), SqlState::INVALID_COMMENT, "{bad}");
    }
    a.run("CREATE TABLE U (U XML(CONTENT(UNTYPED)), D XML(DOCUMENT))").unwrap();
    a.run("INSERT INTO U (U) SELECT XMLCOMMENT(S) FROM T").unwrap();
    assert_eq!(a.run("INSERT INTO U (U) SELECT XMLCOMMENT(S RETURNING SEQUENCE) FROM T").unwrap_err().code, Code::SyntaxRule);
    let error = a.run("INSERT INTO U (D) SELECT XMLCOMMENT(S) FROM T WHERE K = 1").unwrap_err();
    assert_eq!(error.sqlstate(), SqlState::NOT_AN_XML_DOCUMENT, "a comment alone is no document");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.19 SR1, SR3, GR2, GR5 to GR7]
/// XMLTEXT: a text node of the string, escaped as it is serialized; the
/// null value of the null value; one of a character XML does not hold,
/// 0N002 [§9.8 GR9a ii].
#[test]
fn xmltext_6_19() {
    let db = values();
    let a = db.session("A");
    assert_eq!(serialized(&a, "XMLTEXT(S || ' <&>')"), ["1,a b &lt;&amp;&gt;", "2,NULL"]);
    assert_eq!(serialized(&a, "XMLTEXT('' RETURNING SEQUENCE)"), ["1,", "2,"]);
    assert_eq!(rows(&a, "SELECT K FROM T WHERE XMLTEXT(S) IS CONTENT").unwrap(), ["1"]);
    assert_eq!(state(&a, "SELECT XMLTEXT('\u{FFFE}') FROM T"), SqlState::INVALID_XML_CHARACTER);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.17 SR1 to SR4, GR1 to GR10]
/// [XQuery §3.7.3.5] XMLPI: a processing instruction of the target and
/// the string, its leading white space removed; of '' if none is written;
/// the null value of the null value; a target of no NCName or of `xml` in
/// any case SQLCODE -103; content holding '?>' 2200T.
#[test]
fn xmlpi_6_17() {
    let db = values();
    let a = db.session("A");
    assert_eq!(serialized(&a, "XMLPI(NAME \"style\", '  ' || S)"), ["1,<?style a b?>", "2,NULL"]);
    assert_eq!(serialized(&a, "XMLPI(NAME p)"), ["1,<?P?>", "2,<?P?>"]);
    assert_eq!(
        serialized(&a, "XMLPI(NAME \"xml-stylesheet\", 'href=\"s.css\"' RETURNING SEQUENCE)")[0],
        "1,<?xml-stylesheet href=\"s.css\"?>"
    );
    for refused in ["NAME xml", "NAME \"XmL\"", "NAME \"a:b\"", "NAME \"1a\"", "NAME p, 1"] {
        assert_eq!(code(&a, &format!("SELECT XMLPI({refused}) FROM T")), Code::SyntaxRule, "{refused}");
    }
    assert_eq!(code(&a, "SELECT XMLPI(p) FROM T"), Code::Syntax);
    assert_eq!(state(&a, "SELECT XMLPI(NAME p, 'a?>') FROM T"), SqlState::INVALID_PROCESSING_INSTRUCTION);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.12 SR2, SR4, GR1 to GR4,
/// §10.14] XMLCONCAT: every item of each value that is not null, in
/// order; the null value if every one is; RETURNING CONTENT, implicit, a
/// document node of XML(CONTENT(UNTYPED)) of untyped values, else
/// XML(CONTENT(ANY)); RETURNING SEQUENCE, of XML(SEQUENCE); two values or
/// more, each XML.
#[test]
fn xmlconcat_6_12() {
    let db = values();
    let a = db.session("A");
    assert_eq!(serialized(&a, "XMLCONCAT(X, XMLCOMMENT(S), X)"), ["1,<a/><!--a b--><a/>", "2,NULL"]);
    assert_eq!(serialized(&a, "XMLCONCAT(X, XMLTEXT('t') RETURNING SEQUENCE)"), ["1,<a/>t", "2,t"]);
    assert_eq!(rows(&a, "SELECT K FROM T WHERE XMLCONCAT(X, X) IS CONTENT").unwrap(), ["1"]);
    a.run("CREATE TABLE U (U XML(CONTENT(UNTYPED)), C XML(CONTENT))").unwrap();
    a.run("INSERT INTO U (U) SELECT XMLCONCAT(XMLCOMMENT(S), XMLTEXT(S)) FROM T").unwrap();
    a.run("INSERT INTO U (C) SELECT XMLCONCAT(X, X) FROM T").unwrap();
    assert_eq!(a.run("INSERT INTO U (U) SELECT XMLCONCAT(X, X) FROM T").unwrap_err().code, Code::SyntaxRule, "X is of ANY");
    assert_eq!(code(&a, "SELECT XMLCONCAT(X) FROM T"), Code::Syntax);
    assert_eq!(code(&a, "SELECT XMLCONCAT(X, S) FROM T"), Code::SyntaxRule);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.11, §6.19] XMLCOMMENT and
/// XMLTEXT take a character string alone, SQLCODE -103.
#[test]
fn publishing_takes_character_strings() {
    let db = values();
    let a = db.session("A");
    for refused in ["XMLCOMMENT(K)", "XMLTEXT(K)", "XMLTEXT(X)", "XMLCOMMENT(X'00')"] {
        assert_eq!(code(&a, &format!("SELECT {refused} FROM T")), Code::SyntaxRule, "{refused}");
    }
}

/// Schema A: table P of a VARCHAR(10) DEPT, a VARCHAR(10) NAME and an
/// INTEGER RANK, three in 'a', one in 'b' of a null name.
fn people() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE P (DEPT VARCHAR(10), NAME VARCHAR(10), RANK INTEGER)").unwrap();
    let a = db.session("A");
    for row in ["'a', 'Cy', 2", "'a', 'Al', 1", "'a', 'Bo', 3", "'b', NULL, 1"] {
        a.run(&format!("INSERT INTO P VALUES ({row})")).unwrap();
    }
    a.commit().unwrap();
    db
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §11.2 GR2, §10.14] XMLAGG: the
/// XML values of each group concatenated in the order of the sort keys,
/// each in its direction; a null value left out; the null value of a group
/// of none not null, and of no rows.
#[test]
fn xmlagg_11_2() {
    let db = people();
    let a = db.session("A");
    let by = |order: &str| {
        sorted(
            &a,
            &format!(
                "SELECT DEPT, XMLSERIALIZE(XMLAGG(XMLELEMENT(NAME \"n\", NAME) {order}) AS VARCHAR(100)) FROM P WHERE NAME IS NOT NULL OR DEPT = 'b' GROUP BY DEPT"
            ),
        )
    };
    assert_eq!(by("ORDER BY NAME"), ["a,<n>Al</n><n>Bo</n><n>Cy</n>", "b,<n/>"]);
    assert_eq!(by("ORDER BY RANK DESC"), ["a,<n>Bo</n><n>Cy</n><n>Al</n>", "b,<n/>"]);
    let nulls = "SELECT DEPT, XMLSERIALIZE(XMLAGG(XMLELEMENT(NAME \"n\", NAME OPTION NULL ON NULL) ORDER BY NAME) AS VARCHAR(100)) FROM P GROUP BY DEPT";
    assert_eq!(sorted(&a, nulls), ["a,<n>Al</n><n>Bo</n><n>Cy</n>", "b,NULL"]);
    assert_eq!(rows(&a, "SELECT XMLSERIALIZE(XMLAGG(XMLTEXT(NAME)) AS VARCHAR(10)) FROM P WHERE RANK > 9").unwrap(), ["NULL"]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §11.2 SR1, SR2, SR5] RETURNING
/// CONTENT, implicit, a document node of XML(CONTENT(UNTYPED)) of untyped
/// values, else XML(CONTENT(ANY)); RETURNING SEQUENCE of XML(SEQUENCE);
/// an XML value alone aggregated, by keys an ordering operation takes,
/// else SQLCODE -103.
#[test]
fn xmlagg_returning_11_2() {
    let db = people();
    let a = db.session("A");
    assert_eq!(rows(&a, "SELECT DEPT FROM P GROUP BY DEPT HAVING XMLAGG(XMLTEXT(NAME)) IS CONTENT ORDER BY DEPT").unwrap(), ["a"]);
    let sequence = "SELECT DEPT FROM P GROUP BY DEPT HAVING XMLAGG(XMLTEXT(NAME) RETURNING SEQUENCE) IS CONTENT";
    assert_eq!(rows(&a, sequence).unwrap(), Vec::<String>::new());
    a.run("CREATE TABLE U (U XML(CONTENT(UNTYPED)), C XML(CONTENT))").unwrap();
    a.run("INSERT INTO U (U) SELECT XMLAGG(XMLCOMMENT(NAME)) FROM P").unwrap();
    a.run("INSERT INTO U (C) SELECT XMLAGG(XMLPARSE(CONTENT NAME PRESERVE WHITESPACE) RETURNING CONTENT) FROM P").unwrap();
    assert_eq!(a.run("INSERT INTO U (U) SELECT XMLAGG(XMLTEXT(NAME) RETURNING SEQUENCE) FROM P").unwrap_err().code, Code::SyntaxRule);
    for refused in ["XMLAGG(NAME)", "XMLAGG(XMLTEXT(NAME) ORDER BY XMLTEXT(NAME))", "XMLAGG(XMLTEXT(NAME) RETURNING DOCUMENT)"] {
        let code = code(&a, &format!("SELECT {refused} FROM P"));
        assert!(matches!(code, Code::SyntaxRule | Code::Syntax), "{refused}: {code:?}");
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §11.2] [ISO/IEC 9075-2:2023 §6.10]
/// XMLAGG as a window function's aggregate, over each partition, ordered.
#[test]
fn xmlagg_over_a_window() {
    let db = people();
    let a = db.session("A");
    let windowed = "SELECT NAME, XMLSERIALIZE(XMLAGG(XMLTEXT(NAME || ' ') ORDER BY RANK) OVER (PARTITION BY DEPT) AS VARCHAR(40)) FROM P WHERE DEPT = 'a'";
    assert_eq!(sorted(&a, windowed), ["Al,Al Cy Bo ", "Bo,Al Cy Bo ", "Cy,Al Cy Bo "]);
}

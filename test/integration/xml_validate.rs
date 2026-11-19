//! XMLVALIDATE [ISO/IEC 9075-14:2006 draft §6.20] and the XML types of
//! XMLSCHEMA [§4.2.2, §6.1, §11.6]: values validated against registered
//! XML Schemas, typed, assigned and cast by the rules of their types.

use turborel::db::{Database, Session};
use turborel::error::SqlError;

use crate::common::rows;

/// The schema document of target namespace urn:o, of global elements
/// `order`, holding a `qty` of `xs:integer`, and `note`, a string.
pub const ORDERS: &str = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:o" xmlns:o="urn:o" elementFormDefault="qualified"><xs:element name="order"><xs:complexType><xs:sequence><xs:element name="qty" type="xs:integer"/></xs:sequence></xs:complexType></xs:element><xs:element name="note" type="xs:string"/></xs:schema>"#;

/// A schema document of no target namespace, of global element `n`, an
/// `xs:integer`.
pub const PLAIN: &str = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"><xs:element name="n" type="xs:integer"/></xs:schema>"#;

/// A database of schema A, owned by A, holding registered XML Schemas
/// A.O, of ORDERS at location o.xsd, and A.P, of PLAIN.
pub fn registered() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION B").unwrap();
    let a = db.session("A");
    a.run(&format!("CREATE XML SCHEMA O LOCATION 'o.xsd' AS '{ORDERS}'")).unwrap();
    a.run(&format!("CREATE XML SCHEMA P AS '{PLAIN}'")).unwrap();
    a.commit().unwrap();
    db
}

/// The one value `expression` gives, as shown.
pub fn one(s: &Session, expression: &str) -> String {
    let sql = format!("VALUES ({expression})");
    let mut all = rows(s, &sql).unwrap_or_else(|c| panic!("{sql}: {c:?}"));
    assert_eq!(all.len(), 1, "{sql}");
    all.remove(0)
}

/// The SQLCODE and SQLSTATE `sql` fails with, rolled back.
pub fn refused(s: &Session, sql: &str) -> (i32, String) {
    let query = sql.starts_with("VALUES") || sql.starts_with("SELECT");
    let e: SqlError = if query { crate::common::fetched(s, sql).expect_err(sql) } else { s.run(sql).expect_err(sql) };
    s.rollback().unwrap();
    (e.sqlcode(), e.sqlstate().as_str().to_owned())
}

/// XMLVALIDATE `dcs` of the text `xml`, parsed as `syntax`, `according`
/// the clause, serialized.
pub fn validated(s: &Session, (dcs, syntax, xml): (&str, &str, &str), according: &str) -> String {
    one(s, &format!("XMLSERIALIZE(XMLVALIDATE({dcs} XMLPARSE({syntax} '{xml}' PRESERVE WHITESPACE) {according}) AS VARCHAR(200))"))
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.20 GR6a, GR8, §10.22]
/// XMLVALIDATE DOCUMENT of a document valid according to the registered
/// XML Schema indicated by ID, by URI and by URI and LOCATION gives it.
#[test]
fn a_valid_document_is_validated_6_20() {
    let db = registered();
    let a = db.session("A");
    let order = r#"<order xmlns="urn:o"><qty>2</qty></order>"#;
    for according in
        ["ACCORDING TO XMLSCHEMA ID A.O", "ACCORDING TO XMLSCHEMA URI 'urn:o'", "ACCORDING TO XMLSCHEMA URI 'urn:o' LOCATION 'o.xsd'"]
    {
        assert_eq!(validated(&a, ("DOCUMENT", "DOCUMENT", order), according), order, "{according}");
    }
    let element = "ACCORDING TO XMLSCHEMA ID A.O NAMESPACE 'urn:o' ELEMENT \"order\"";
    assert_eq!(validated(&a, ("DOCUMENT", "DOCUMENT", order), element), order);
    assert_eq!(validated(&a, ("DOCUMENT", "DOCUMENT", order), "ACCORDING TO XMLSCHEMA ID A.O NAMESPACE 'urn:o'"), order);
    assert_eq!(
        validated(&a, ("DOCUMENT", "DOCUMENT", "<n>7</n>"), "ACCORDING TO XMLSCHEMA NO NAMESPACE NO NAMESPACE ELEMENT \"n\""),
        "<n>7</n>"
    );
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.20 GR6a iii, GR6b, GR6c]
/// XMLVALIDATE CONTENT validates each element child of the document node
/// alone, its comments copied; SEQUENCE each element and document node,
/// a comment or processing instruction copied, and of the empty sequence
/// gives it [GR4].
#[test]
fn content_and_sequences_are_validated_6_20() {
    let db = registered();
    let a = db.session("A");
    let notes = r#"<note xmlns="urn:o">a</note><!--c--><note xmlns="urn:o">b</note>"#;
    assert_eq!(validated(&a, ("CONTENT", "CONTENT", notes), "ACCORDING TO XMLSCHEMA ID A.O ELEMENT \"note\""), notes);
    let sequence = "XMLCONCAT(XMLPARSE(DOCUMENT '<n>1</n>' PRESERVE WHITESPACE), XMLCOMMENT('c'), XMLQUERY('<n>2</n>' RETURNING SEQUENCE BY REF EMPTY ON EMPTY) RETURNING SEQUENCE)";
    let shown = one(&a, &format!("XMLSERIALIZE(CONTENT XMLVALIDATE(SEQUENCE {sequence} ACCORDING TO XMLSCHEMA ID A.P) AS VARCHAR(100))"));
    assert_eq!(shown, "<n>1</n><!--c--><n>2</n>");
    let none = "XMLQUERY('()' RETURNING SEQUENCE BY REF EMPTY ON EMPTY)";
    assert_eq!(one(&a, &format!("XMLSERIALIZE(CONTENT XMLVALIDATE(SEQUENCE {none}) AS VARCHAR(10))")), "");
    assert_eq!(one(&a, "XMLVALIDATE(DOCUMENT CAST(NULL AS XML(DOCUMENT(UNTYPED))))"), "NULL");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.20 GR6a iii 2 A II, GR6b i]
/// implementation-defined: of no `<XML valid according to clause>`, each
/// element is validated by the registered XML Schema of its namespace,
/// none the one of no target namespace, that the current user may use;
/// of a namespace of none, SQLSTATE 2201L, as of one another user's.
#[test]
fn data_driven_validation_6_20() {
    let db = registered();
    let a = db.session("A");
    let mixed = r#"<n>4</n><note xmlns="urn:o">x</note>"#;
    assert_eq!(validated(&a, ("CONTENT", "CONTENT", mixed), ""), mixed);
    assert_eq!(refused(&a, "VALUES (XMLVALIDATE(CONTENT XMLPARSE(CONTENT '<x xmlns=\"urn:x\"/>' PRESERVE WHITESPACE)))").1, "2201L");
    let b = db.session("B");
    assert_eq!(refused(&b, &format!("VALUES (XMLVALIDATE(CONTENT XMLPARSE(CONTENT '{mixed}' PRESERVE WHITESPACE)))")).1, "2201L");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.20 GR3, GR6] each exception of
/// XMLVALIDATE: DOCUMENT of no document, 2200M; CONTENT of no document
/// node, 2200N; SEQUENCE of an atomic value or a text node, 2201J; a
/// document node of a text child, 2201K; an element of another name than
/// ELEMENT's, 2201P, or of another namespace than NAMESPACE's, 2201Q; an
/// element not valid, 2201R.
#[test]
fn what_xmlvalidate_raises_6_20() {
    let db = registered();
    let a = db.session("A");
    let fails = |dcs: &str, value: &str, according: &str| refused(&a, &format!("VALUES (XMLVALIDATE({dcs} {value} {according}))")).1;
    let content = |text: &str| format!("XMLPARSE(CONTENT '{text}' PRESERVE WHITESPACE)");
    assert_eq!(fails("DOCUMENT", &content("<n>1</n><n>2</n>"), "ACCORDING TO XMLSCHEMA ID A.P"), "2200M");
    assert_eq!(fails("CONTENT", "XMLQUERY('<n>1</n>' RETURNING SEQUENCE BY REF EMPTY ON EMPTY)", ""), "2200N");
    assert_eq!(fails("SEQUENCE", "XMLQUERY('1' RETURNING SEQUENCE BY REF EMPTY ON EMPTY)", ""), "2201J");
    assert_eq!(fails("CONTENT", &content("<n>1</n>text"), "ACCORDING TO XMLSCHEMA ID A.P"), "2201K");
    let order = content(r#"<order xmlns="urn:o"><qty>2</qty></order>"#);
    assert_eq!(fails("CONTENT", &order, "ACCORDING TO XMLSCHEMA ID A.O ELEMENT \"note\""), "2201P");
    let unqualified = content("<note>x</note>");
    assert_eq!(fails("CONTENT", &unqualified, "ACCORDING TO XMLSCHEMA ID A.O ELEMENT \"note\""), "2201P", "of no namespace");
    assert_eq!(fails("CONTENT", &content("<n>1</n>"), "ACCORDING TO XMLSCHEMA ID A.P NAMESPACE 'urn:o'"), "2201M");
    assert_eq!(fails("CONTENT", &order, "ACCORDING TO XMLSCHEMA URI 'urn:o' NO NAMESPACE"), "2201M");
    let both = format!("XMLCONCAT({}, {order} RETURNING SEQUENCE)", content("<n>1</n>"));
    assert_eq!(fails("SEQUENCE", &both, "ACCORDING TO XMLSCHEMA ID A.O NAMESPACE 'urn:o'"), "2201Q");
    assert_eq!(fails("CONTENT", &content("<n>one</n>"), "ACCORDING TO XMLSCHEMA ID A.P"), "2201R");
    assert_eq!(fails("CONTENT", &content("<m>1</m>"), "ACCORDING TO XMLSCHEMA ID A.P"), "2201R");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.20 SR3, §4.2.3] XMLVALIDATE of
/// a DOCUMENT or CONTENT value is of a type of XMLSCHEMA of what its
/// clause indicates, so a column of it takes the value; of no clause, of
/// ANY, which it does not, SQLCODE -103; the elements validated are typed,
/// their values numbers that compare as numbers.
#[test]
fn what_xmlvalidate_makes_6_20() {
    let db = registered();
    let a = db.session("A");
    a.run("CREATE TABLE T (X XML(CONTENT(XMLSCHEMA ID A.P)))").unwrap();
    let ten = "XMLPARSE(CONTENT '<n>10</n>' PRESERVE WHITESPACE)";
    a.run(&format!("INSERT INTO T VALUES (XMLVALIDATE(CONTENT {ten} ACCORDING TO XMLSCHEMA ID A.P))")).unwrap();
    a.commit().unwrap();
    assert_eq!(refused(&a, &format!("INSERT INTO T VALUES (XMLVALIDATE(CONTENT {ten}))")).0, -103);
    let compared = "SELECT XMLCAST(XMLQUERY('/n > 9' PASSING BY REF X RETURNING SEQUENCE EMPTY ON EMPTY) AS BOOLEAN), XMLCAST(XMLQUERY('data(/n) eq 10' PASSING BY REF X RETURNING SEQUENCE EMPTY ON EMPTY) AS BOOLEAN) FROM T";
    assert_eq!(rows(&a, compared).unwrap(), ["TRUE,TRUE"]);
    let text = "SELECT XMLCAST(XMLQUERY('data(/n) eq ''10''' PASSING BY REF X RETURNING SEQUENCE EMPTY ON EMPTY) AS BOOLEAN) FROM T";
    assert_eq!(refused(&a, text).1, "10000", "an integer is no string");
    assert_eq!(rows(&a, "SELECT XMLCAST(X AS INTEGER) + 1 FROM T").unwrap(), ["11"]);
    let tests = "SELECT XMLSERIALIZE(CONTENT XMLQUERY('(/n instance of element(n, xs:integer), /n instance of element(*, xs:decimal), /n instance of element(*, xdt:untyped))' PASSING BY REF X RETURNING SEQUENCE EMPTY ON EMPTY) AS VARCHAR(40)) FROM T";
    assert_eq!(rows(&a, tests).unwrap(), ["true true false"], "[XQuery §2.5.4.3] its type annotation's");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.13 GR9a, §10.19, §11.2 SR2a,
/// §6.12 SR2a] XMLDOCUMENT RETURNING CONTENT, of XML(CONTENT(UNTYPED)),
/// unvalidates what it is given, its integer no `xs:integer`; XMLAGG and
/// XMLCONCAT RETURNING CONTENT of values of XMLSCHEMA keep their types, of
/// a type of XMLSCHEMA a column of it takes.
#[test]
fn what_publishing_keeps_of_validated_values_6_13() {
    let db = registered();
    let a = db.session("A");
    let valid = "XMLVALIDATE(CONTENT XMLPARSE(CONTENT '<n>3</n>' PRESERVE WHITESPACE) ACCORDING TO XMLSCHEMA ID A.P)";
    let typed = |e: &str| {
        one(
            &a,
            &format!(
                "XMLCAST(XMLQUERY('data(/n) instance of xs:integer' PASSING BY REF {e} RETURNING SEQUENCE EMPTY ON EMPTY) AS BOOLEAN)"
            ),
        )
    };
    assert_eq!(typed(valid), "TRUE");
    assert_eq!(typed(&format!("XMLDOCUMENT({valid})")), "FALSE");
    a.run("CREATE TABLE T (X XML(CONTENT(XMLSCHEMA ID A.P)), Y XML(CONTENT(XMLSCHEMA ID A.P)))").unwrap();
    a.run(&format!("INSERT INTO T VALUES ({valid}, {valid})")).unwrap();
    a.run("INSERT INTO T SELECT XMLAGG(X), XMLAGG(XMLCONCAT(X, Y)) FROM T").unwrap();
    let count = "SELECT XMLCAST(XMLQUERY('count(/n[data(.) instance of xs:integer])' PASSING BY REF X RETURNING SEQUENCE EMPTY ON EMPTY) AS INTEGER), XMLCAST(XMLQUERY('count(/n[data(.) instance of xs:integer])' PASSING BY REF Y RETURNING SEQUENCE EMPTY ON EMPTY) AS INTEGER) FROM T";
    assert_eq!(crate::common::sorted(&a, count), ["1,1", "1,2"]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §11.6 AR1, §4.2.6] the owner of a
/// registered XML Schema's schema may validate by it, another may not,
/// SQLCODE -104 (`catalog::xml_schemas::may_use`); a permanently registered
/// one everyone may use: INFORMATION_SCHEMA.XS, the Schema for Schemas,
/// validates a schema document, adding the attributes its declarations
/// default [Schema1 §3.4.5], and refuses an element it does not declare,
/// SQLSTATE 2201R.
#[test]
fn who_may_validate_by_a_schema_11_6() {
    let db = registered();
    let b = db.session("B");
    let value = "XMLPARSE(CONTENT '<n>1</n>' PRESERVE WHITESPACE)";
    assert_eq!(refused(&b, &format!("VALUES (XMLVALIDATE(CONTENT {value} ACCORDING TO XMLSCHEMA ID A.P))")).0, -104);
    assert_eq!(refused(&b, &format!("VALUES (XMLVALIDATE(CONTENT {value} ACCORDING TO XMLSCHEMA NO NAMESPACE))")).0, -103);
    let xs = "ACCORDING TO XMLSCHEMA ID INFORMATION_SCHEMA.XS";
    assert_eq!(refused(&b, &format!("VALUES (XMLVALIDATE(CONTENT {value} {xs}))")).1, "2201R");
    let schema = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"><xs:element name="n" type="xs:int"/></xs:schema>"#;
    let typed =
        one(&b, &format!("XMLSERIALIZE(XMLVALIDATE(DOCUMENT XMLPARSE(DOCUMENT '{schema}' PRESERVE WHITESPACE) {xs}) AS VARCHAR(1000))"));
    let defaulted = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" finalDefault="" blockDefault="" attributeFormDefault="unqualified" elementFormDefault="unqualified"><xs:element name="n" type="xs:int" nillable="false" abstract="false"/></xs:schema>"#;
    assert_eq!(typed, defaulted, "the defaults the Schema for Schemas declares added");
}

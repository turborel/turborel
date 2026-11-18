//! The XML types of XMLSCHEMA [ISO/IEC 9075-14:2006 draft §4.2.2, §6.1,
//! §11.6]: columns of them, what is assigned and cast to them [§4.2.4,
//! §6.5, §6.6, §10.2], the types values of them bring together make
//! [§10.3], what they keep through storage and recovery, and what they
//! keep from being dropped.

use std::sync::Arc;

use turborel::db::{Database, OpenOptions};
use turborel::disk::io::{FileSystem, OsFileSystem};
use turborel::value::DataType;
use turborel::value::xml::{Typing, XmlType};

use crate::common::{Scratch, after_crash, rows};
use crate::xml_validate::{ORDERS, PLAIN, one, refused, registered};

/// An order, as text.
const ORDER: &str = r#"<order xmlns="urn:o"><qty>12</qty></order>"#;

/// `ORDER` parsed, and validated by A.O.
fn valid_order() -> String {
    format!("XMLVALIDATE(DOCUMENT XMLPARSE(DOCUMENT '{ORDER}' PRESERVE WHITESPACE) ACCORDING TO XMLSCHEMA ID A.O)")
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.1 SR4, GR1, §11.6 SR1 to SR4]
/// a column of XML(DOCUMENT(XMLSCHEMA ...)) is of the registered XML
/// Schema indicated, by ID or by its target namespace, and of the
/// namespace and element its element clause indicates, the namespace the
/// target namespace's if an ELEMENT alone is written; as a prepared query
/// describes it.
#[test]
fn columns_of_xmlschema_types_6_1() {
    let db = registered();
    let a = db.session("A");
    a.run("CREATE TABLE T (D XML(DOCUMENT(XMLSCHEMA ID A.O)), E XML(DOCUMENT(XMLSCHEMA URI 'urn:o' ELEMENT \"order\")), C XML(CONTENT(XMLSCHEMA NO NAMESPACE NO NAMESPACE)))").unwrap();
    let prepared = a.prepare("SELECT D, E, C FROM T").unwrap();
    let types: Vec<String> = prepared.columns().iter().map(|c| c.data_type.to_string()).collect();
    assert_eq!(
        types,
        [
            "XML(DOCUMENT(XMLSCHEMA ID A.O))",
            "XML(DOCUMENT(XMLSCHEMA ID A.O NAMESPACE 'urn:o' ELEMENT \"order\"))",
            "XML(CONTENT(XMLSCHEMA ID A.P NO NAMESPACE))",
        ]
    );
    let DataType::Xml(XmlType::Document(Typing::Schema(named))) = &prepared.columns()[1].data_type else { panic!("of XMLSCHEMA") };
    assert_eq!((named.schema.as_str(), named.name.as_str()), ("A", "O"));
    assert_eq!((named.namespace.as_deref(), named.element.as_deref()), (Some("urn:o"), Some("order")));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §11.6 SR1, SR2b ii, AR1, GR1]
/// implementation-defined [SR4b, SR4d iii]: a name of no registered XML
/// Schema, or a URI of none, is SQLCODE -103; one another's, -104; a
/// namespace the schema has not is SQLSTATE 2201M, a global element it
/// does not declare 2201N, as the type is defined; a permanently
/// registered one, of an element it declares, is a type: a column of it
/// takes a schema document INFORMATION_SCHEMA.XS validates, and
/// XMLVALIDATE refuses another, SQLSTATE 2201R.
#[test]
fn an_xmlschema_type_indicates_what_is_there_11_6() {
    let db = registered();
    let (a, b) = (db.session("A"), db.session("B"));
    let table = |t: &str| format!("CREATE TABLE T (X XML(DOCUMENT(XMLSCHEMA {t})))");
    assert_eq!(refused(&a, &table("ID A.NONE")).0, -103);
    assert_eq!(refused(&a, &table("URI 'urn:none'")).0, -103);
    assert_eq!(refused(&a, &table("URI 'urn:o' LOCATION 'other.xsd'")).0, -103);
    assert_eq!(refused(&b, &table("ID A.O").replace("TABLE T", "TABLE B.T")).0, -104);
    assert_eq!(refused(&a, &table("ID A.O NAMESPACE 'urn:x'")), (-301, "2201M".to_owned()));
    assert_eq!(refused(&a, &table("ID A.O ELEMENT \"qty\"")), (-301, "2201N".to_owned()));
    assert_eq!(refused(&a, &table("ID A.O NO NAMESPACE ELEMENT \"order\"")), (-301, "2201M".to_owned()));
    a.run(&table("ID INFORMATION_SCHEMA.XS ELEMENT \"schema\"")).unwrap();
    let schema = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"><xs:element name="n" type="xs:int"/></xs:schema>"#;
    let valid = |text: &str| {
        format!(
            "INSERT INTO T VALUES (XMLVALIDATE(DOCUMENT XMLPARSE(DOCUMENT '{text}' PRESERVE WHITESPACE) ACCORDING TO XMLSCHEMA ID INFORMATION_SCHEMA.XS ELEMENT \"schema\"))"
        )
    };
    a.run(&valid(schema)).unwrap();
    a.commit().unwrap();
    let not = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"><xs:element/></xs:schema>"#;
    assert_eq!(refused(&a, &valid(not)).1, "2201R");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §4.2.4, §10.2 SR1b ii, GR1a] a
/// value XMLVALIDATE makes of the registered XML Schema of a column's type
/// is assigned to it; one of no XMLSCHEMA type, of another schema, or of
/// no element the column's names, is refused, SQLCODE -103; a value of
/// no document is SQLSTATE 2200L.
#[test]
fn what_a_column_of_xmlschema_takes_10_2() {
    let db = registered();
    let a = db.session("A");
    a.run("CREATE TABLE T (X XML(DOCUMENT(XMLSCHEMA ID A.O)), E XML(DOCUMENT(XMLSCHEMA ID A.O ELEMENT \"order\")))").unwrap();
    a.run(&format!("INSERT INTO T (X) VALUES ({})", valid_order())).unwrap();
    let element = valid_order().replace("ID A.O)", "ID A.O ELEMENT \"order\")");
    a.run(&format!("INSERT INTO T (X, E) VALUES ({element}, {element})")).unwrap();
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM T").unwrap(), ["2"]);
    a.commit().unwrap();
    let untyped = format!("XMLPARSE(DOCUMENT '{ORDER}' PRESERVE WHITESPACE)");
    assert_eq!(refused(&a, &format!("INSERT INTO T (X) VALUES ({untyped})")).0, -103);
    assert_eq!(refused(&a, &format!("INSERT INTO T (E) VALUES ({})", valid_order())).0, -103, "no element");
    let note = "XMLVALIDATE(DOCUMENT XMLPARSE(DOCUMENT '<note xmlns=\"urn:o\">n</note>' PRESERVE WHITESPACE) ACCORDING TO XMLSCHEMA ID A.O ELEMENT \"note\")";
    assert_eq!(refused(&a, &format!("INSERT INTO T (E) VALUES ({note})")).0, -103, "another element");
    let plain = "XMLVALIDATE(DOCUMENT XMLPARSE(DOCUMENT '<n>1</n>' PRESERVE WHITESPACE) ACCORDING TO XMLSCHEMA ID A.P)";
    assert_eq!(refused(&a, &format!("INSERT INTO T (X) VALUES ({plain})")).0, -103, "another schema");
    // [§10.2 SR1b ii 3] Of CONTENT, assignable; [GR1a i] of a document
    // alone.
    let notes = r#"<note xmlns="urn:o">a</note><note xmlns="urn:o">b</note>"#;
    let content = format!("XMLVALIDATE(CONTENT XMLPARSE(CONTENT '{notes}' PRESERVE WHITESPACE) ACCORDING TO XMLSCHEMA ID A.O)");
    assert_eq!(refused(&a, &format!("INSERT INTO T (X) VALUES ({content})")).1, "2200L", "of CONTENT");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.5 SR4b, GR1a, GR1b i] CAST BY
/// REF to a type of XMLSCHEMA takes a value of one of its registered XML
/// Schema alone, else SQLCODE -103; BY VALUE validates the value as
/// XMLVALIDATE does [GR1b i 1], one not valid SQLSTATE 2201R, one no
/// document 2200L; and to an UNTYPED type BY VALUE makes it unvalidated
/// [§10.19], so its integer is no `xs:integer` there.
#[test]
fn cast_to_and_from_xmlschema_types_6_5() {
    let db = registered();
    let a = db.session("A");
    let untyped = format!("XMLPARSE(DOCUMENT '{ORDER}' PRESERVE WHITESPACE)");
    let target = "XML(DOCUMENT(XMLSCHEMA ID A.O))";
    assert_eq!(refused(&a, &format!("VALUES (CAST({untyped} AS {target}))")).0, -103);
    let typed = |e: &str| {
        format!(
            "XMLCAST(XMLQUERY('declare namespace o=\"urn:o\"; data(/o:order/o:qty) instance of xs:integer' PASSING BY REF {e} RETURNING SEQUENCE EMPTY ON EMPTY) AS BOOLEAN)"
        )
    };
    assert_eq!(one(&a, &typed(&format!("CAST({untyped} AS {target} BY VALUE)"))), "TRUE");
    assert_eq!(one(&a, &typed(&format!("CAST({} AS {target})", valid_order()))), "TRUE");
    assert_eq!(one(&a, &typed(&format!("CAST({} AS XML(DOCUMENT(UNTYPED)) BY VALUE)", valid_order()))), "FALSE");
    assert_eq!(one(&a, &typed(&format!("CAST({} AS XML(DOCUMENT(ANY)) BY VALUE)", valid_order()))), "TRUE");
    let bad = format!("XMLPARSE(DOCUMENT '{}' PRESERVE WHITESPACE)", ORDER.replace("12", "twelve"));
    assert_eq!(refused(&a, &format!("VALUES (CAST({bad} AS {target} BY VALUE))")).1, "2201R");
    let two = "XMLPARSE(CONTENT '<a/><b/>' PRESERVE WHITESPACE)";
    assert_eq!(refused(&a, &format!("VALUES (CAST({two} AS {target} BY VALUE))")).1, "2200L");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.6 SR7, SR10b ii] XMLCAST of a
/// value of no XML type to a type of XMLSCHEMA is SQLCODE -103; of an XML
/// value, it is CAST, BY REF unless BY VALUE is written.
#[test]
fn xmlcast_to_xmlschema_types_6_6() {
    let db = registered();
    let a = db.session("A");
    assert_eq!(refused(&a, "VALUES (XMLCAST('<n>1</n>' AS XML(CONTENT(XMLSCHEMA ID A.P))))").0, -103);
    let untyped = "XMLPARSE(DOCUMENT '<n>1</n>' PRESERVE WHITESPACE)";
    assert_eq!(refused(&a, &format!("VALUES (XMLCAST({untyped} AS XML(DOCUMENT(XMLSCHEMA ID A.P))))")).0, -103);
    let shown = one(&a, &format!("XMLSERIALIZE(XMLCAST({untyped} AS XML(DOCUMENT(XMLSCHEMA ID A.P)) BY VALUE) AS VARCHAR(20))"));
    assert_eq!(shown, "<n>1</n>");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.3 SR1b ii 2 B III to IX] the
/// values of a UNION ALL of types of XMLSCHEMA of one registered XML
/// Schema are of a type of it, of the namespace and element all have,
/// DOCUMENT if all are: so one of the element of all is assigned to a
/// column of it, and one of two elements is not, SQLCODE -103; with an
/// UNTYPED type, XML(CONTENT(ANY)), no column of XMLSCHEMA's.
#[test]
fn xmlschema_types_brought_together_10_3() {
    let db = registered();
    let a = db.session("A");
    a.run("CREATE TABLE T (E XML(DOCUMENT(XMLSCHEMA ID A.O ELEMENT \"order\")), S XML(DOCUMENT(XMLSCHEMA ID A.O)))").unwrap();
    let order = valid_order().replace("ID A.O)", "ID A.O ELEMENT \"order\")");
    let note = "XMLVALIDATE(DOCUMENT XMLPARSE(DOCUMENT '<note xmlns=\"urn:o\">n</note>' PRESERVE WHITESPACE) ACCORDING TO XMLSCHEMA ID A.O ELEMENT \"note\")";
    a.run(&format!("INSERT INTO T (E) SELECT * FROM (VALUES ({order})) AS V UNION ALL SELECT * FROM (VALUES ({order})) AS W")).unwrap();
    a.run(&format!("INSERT INTO T (S) SELECT * FROM (VALUES ({order})) AS V UNION ALL SELECT * FROM (VALUES ({note})) AS W")).unwrap();
    assert_eq!(rows(&a, "SELECT COUNT(E), COUNT(S) FROM T").unwrap(), ["2,2"]);
    a.commit().unwrap();
    let two = format!("INSERT INTO T (E) SELECT * FROM (VALUES ({order})) AS V UNION ALL SELECT * FROM (VALUES ({note})) AS W");
    assert_eq!(refused(&a, &two).0, -103);
    let untyped =
        format!("INSERT INTO T (S) SELECT * FROM (VALUES ({order})) AS V UNION ALL VALUES (XMLPARSE(DOCUMENT '<x/>' PRESERVE WHITESPACE))");
    assert_eq!(refused(&a, &untyped).0, -103);
}

/// Verifies: [DESIGN] RULES.md D86: a value of a type of XMLSCHEMA is one
/// a validation made, so one sent as text, a parameter's, is not taken
/// for one, SQLSTATE 2201R.
#[test]
fn a_parameter_of_an_xmlschema_type_is_validated_first() {
    let db = registered();
    let a = db.session("A");
    a.run("CREATE TABLE T (X XML(DOCUMENT(XMLSCHEMA ID A.O)))").unwrap();
    let prepared = a.prepare("INSERT INTO T VALUES (?)").unwrap();
    let sent = turborel::value::Value::Xml(turborel::value::xml::XmlValue::serialized(ORDER));
    let e = a.execute_prepared(&prepared, &[sent]).unwrap_err();
    assert_eq!(e.sqlstate().as_str(), "2201R");
}

/// Verifies: [DESIGN] RULES.md D25, D86: a column of a type of XMLSCHEMA
/// and the type annotations of its values survive reopening, after a
/// checkpoint and a close and from the log alone after a failure: its
/// integer is still an `xs:integer`, and an untyped value still refused.
#[test]
fn typed_values_survive_reopening() {
    for checkpointed in [false, true] {
        let scratch = Scratch::new(if checkpointed { "xml-typed-checkpointed" } else { "xml-typed-failed" });
        let (fs, dir): (Arc<dyn FileSystem>, _) = (Arc::new(OsFileSystem), scratch.path("db"));
        let crashed = Database::new();
        crashed.open_files(&fs, &dir, &OpenOptions::default()).unwrap();
        crashed.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
        let a = crashed.session("A");
        a.run(&format!("CREATE XML SCHEMA O AS '{ORDERS}'")).unwrap();
        a.run(&format!("CREATE XML SCHEMA P AS '{PLAIN}'")).unwrap();
        a.run("CREATE TABLE T (X XML(DOCUMENT(XMLSCHEMA ID A.O ELEMENT \"order\")))").unwrap();
        a.run(&format!("INSERT INTO T VALUES ({})", valid_order().replace("ID A.O)", "ID A.O ELEMENT \"order\")"))).unwrap();
        a.commit().unwrap();
        if checkpointed {
            crashed.checkpoint().unwrap();
            crashed.close().unwrap();
        }
        let db = Database::new();
        db.open_files(&if checkpointed { Arc::clone(&fs) } else { after_crash() }, &dir, &OpenOptions::default()).unwrap();
        let s = db.session("A");
        let typed = "SELECT XMLCAST(XMLQUERY('declare namespace o=\"urn:o\"; data(/o:order/o:qty) instance of xs:integer' PASSING BY REF X RETURNING SEQUENCE EMPTY ON EMPTY) AS BOOLEAN) FROM T";
        assert_eq!(rows(&s, typed).unwrap(), ["TRUE"]);
        assert_eq!(refused(&s, &format!("INSERT INTO T VALUES (XMLPARSE(DOCUMENT '{ORDER}' PRESERVE WHITESPACE))")).0, -103);
        db.close().unwrap();
        drop((db, a, crashed));
    }
}

/// Verifies: [EXTENSION] RULES.md D86: DROP XML SCHEMA is refused, even
/// with CASCADE, while a column, a domain or a distinct type is of a type
/// of XMLSCHEMA of it, SQLCODE -103; RESTRICT is refused while a routine
/// has a parameter of one, and CASCADE drops the routine.
#[test]
fn what_is_of_an_xmlschema_type_keeps_its_schema() {
    let db = registered();
    let a = db.session("A");
    for (create, drop) in [
        ("CREATE TABLE T (X XML(DOCUMENT(XMLSCHEMA ID A.O)))", "DROP TABLE T RESTRICT"),
        ("CREATE DOMAIN D AS XML(CONTENT(XMLSCHEMA ID A.O))", "DROP DOMAIN D RESTRICT"),
        ("CREATE TYPE U AS XML(DOCUMENT(XMLSCHEMA ID A.O)) FINAL", "DROP TYPE U RESTRICT"),
    ] {
        a.run(create).unwrap_or_else(|e| panic!("{create}: {e:?}"));
        a.commit().unwrap();
        assert_eq!(refused(&a, "DROP XML SCHEMA O CASCADE").0, -103, "{create}");
        a.run(drop).unwrap();
        a.commit().unwrap();
    }
    a.run("CREATE FUNCTION F (X XML(DOCUMENT(XMLSCHEMA ID A.O))) RETURNS INTEGER CONTAINS SQL RETURN 1").unwrap();
    a.commit().unwrap();
    assert_eq!(refused(&a, "DROP XML SCHEMA O RESTRICT").0, -103);
    a.run("DROP XML SCHEMA O CASCADE").unwrap();
    a.commit().unwrap();
    a.run("CREATE FUNCTION F (X XML) RETURNS INTEGER CONTAINS SQL RETURN 1").unwrap();
    a.commit().unwrap();
}

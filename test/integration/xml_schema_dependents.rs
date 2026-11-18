//! The definitions that name a registered XML Schema [ISO/IEC 9075-14:2006
//! draft §4.2.6]: bound to the one they named as they were created, and
//! its dependents, which DROP XML SCHEMA RESTRICT keeps and CASCADE drops.

use std::sync::Arc;

use turborel::db::{Database, OpenOptions, Session};
use turborel::disk::io::{FileSystem, OsFileSystem};

use crate::common::{Scratch, after_crash, rows};
use crate::xml_validate::{ORDERS, one, refused, registered};

/// A schema document of target namespace urn:o whose `order` holds a `qty`
/// of any string, unlike ORDERS's, whose `qty` is an integer.
const LOOSE: &str = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:o" elementFormDefault="qualified"><xs:element name="order"><xs:complexType><xs:sequence><xs:element name="qty" type="xs:string"/></xs:sequence></xs:complexType></xs:element></xs:schema>"#;

/// An order of a `qty` no integer, valid by LOOSE alone.
const SOME: &str = r#"<order xmlns="urn:o"><qty>some</qty></order>"#;

/// `text` parsed as a document.
fn parsed(text: &str) -> String {
    format!("XMLPARSE(DOCUMENT '{text}' PRESERVE WHITESPACE)")
}

/// Whether `s` may drop registered XML Schema A.O with RESTRICT: the
/// SQLCODE of the refusal, if refused, rolled back.
fn restrict(s: &Session) -> Option<i32> {
    match s.run("DROP XML SCHEMA O RESTRICT") {
        Ok(_) => {
            s.rollback().unwrap();
            None
        }
        Err(e) => {
            s.rollback().unwrap();
            Some(e.sqlcode())
        }
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §4.2.6] [EXTENSION] RULES.md
/// D86: a view, a CHECK, a domain's constraint, an assertion, a trigger
/// and a routine that name registered XML Schema A.O through XMLVALIDATE,
/// IS VALID or a type of XMLSCHEMA each keep DROP XML SCHEMA RESTRICT
/// from dropping it, SQLCODE -103; CASCADE drops each, and the schema, so
/// one registered again of its name has none.
#[test]
fn definitions_depend_on_what_they_name_4_2_6() {
    let order = parsed(r#"<order xmlns="urn:o"><qty>1</qty></order>"#);
    for definition in [
        format!("CREATE VIEW V AS SELECT XMLVALIDATE(DOCUMENT {order} ACCORDING TO XMLSCHEMA URI 'urn:o') AS X FROM (VALUES (1)) AS T (N)"),
        "CREATE TABLE T (X XML(DOCUMENT(UNTYPED)) CHECK (X IS VALID DOCUMENT ACCORDING TO XMLSCHEMA ID O))".to_owned(),
        "CREATE DOMAIN D AS XML(DOCUMENT(UNTYPED)) CHECK (VALUE IS VALID DOCUMENT ACCORDING TO XMLSCHEMA ID O)".to_owned(),
        "CREATE ASSERTION S CHECK ((SELECT COUNT(*) FROM U WHERE X IS NOT VALID DOCUMENT ACCORDING TO XMLSCHEMA ID O) = 0)".to_owned(),
        "CREATE TRIGGER G AFTER INSERT ON U REFERENCING NEW ROW AS N FOR EACH ROW INSERT INTO W VALUES (CAST(N.X AS XML(DOCUMENT(XMLSCHEMA ID O)) BY VALUE))".to_owned(),
        format!("CREATE FUNCTION F () RETURNS BOOLEAN CONTAINS SQL RETURN {order} IS VALID DOCUMENT ACCORDING TO XMLSCHEMA URI 'urn:o'"),
    ] {
        let db = registered();
        let a = db.session("A");
        a.run("CREATE TABLE U (X XML(DOCUMENT(UNTYPED)))").unwrap();
        a.run("CREATE TABLE W (X XML(DOCUMENT(ANY)))").unwrap();
        a.run(&definition).unwrap_or_else(|e| panic!("{definition}: {e:?}"));
        a.commit().unwrap();
        assert_eq!(restrict(&a), Some(-103), "{definition}");
        a.run("DROP XML SCHEMA O CASCADE").unwrap_or_else(|e| panic!("{definition}: {e:?}"));
        a.commit().unwrap();
        a.run(&format!("CREATE XML SCHEMA O AS '{ORDERS}'")).unwrap();
        a.commit().unwrap();
        assert_eq!(restrict(&a), None, "what named it was dropped: {definition}");
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §4.2.6, §11.6 SR2b ii] a view's
/// URI indicates the registered XML Schema it indicated as the view was
/// created, though one of a lesser name of the same target namespace is
/// registered after, which a statement's URI then indicates; so the
/// view's order of no integer is still not valid, SQLSTATE 2201R, and a
/// statement's is; so too of a CAST BY VALUE to a type of XMLSCHEMA of a
/// URI, and of an array of one, in a view; and so after the database is
/// opened again, until CASCADE drops them all.
#[test]
fn a_view_keeps_the_schema_it_named_4_2_6() {
    let scratch = Scratch::new("xml-schema-pinned");
    let (fs, dir): (Arc<dyn FileSystem>, _) = (Arc::new(OsFileSystem), scratch.path("db"));
    let crashed = Database::new();
    crashed.open_files(&fs, &dir, &OpenOptions::default()).unwrap();
    crashed.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let a = crashed.session("A");
    a.run(&format!("CREATE XML SCHEMA O AS '{ORDERS}'")).unwrap();
    let validated = format!("XMLVALIDATE(DOCUMENT {} ACCORDING TO XMLSCHEMA URI 'urn:o')", parsed(SOME));
    a.run(&format!("CREATE VIEW V AS SELECT XMLSERIALIZE(CONTENT {validated} AS VARCHAR(60)) AS X FROM (VALUES (1)) AS T (N)")).unwrap();
    let cast = format!("CAST({} AS XML(DOCUMENT(XMLSCHEMA URI 'urn:o')) BY VALUE)", parsed(SOME));
    a.run(&format!("CREATE VIEW C AS SELECT XMLSERIALIZE(CONTENT {cast} AS VARCHAR(60)) AS X FROM (VALUES (1)) AS T (N)")).unwrap();
    let one_order = parsed(r#"<order xmlns="urn:o"><qty>1</qty></order>"#);
    let array = format!(
        "CAST(ARRAY[XMLVALIDATE(DOCUMENT {one_order} ACCORDING TO XMLSCHEMA ID O)] AS XML(DOCUMENT(XMLSCHEMA URI 'urn:o')) ARRAY[2])"
    );
    a.run(&format!("CREATE VIEW R AS SELECT CARDINALITY({array}) AS X FROM (VALUES (1)) AS T (N)")).unwrap();
    a.commit().unwrap();
    assert_eq!(refused(&a, "SELECT X FROM V").1, "2201R");
    assert_eq!(refused(&a, "SELECT X FROM C").1, "2201R");
    assert_eq!(rows(&a, "SELECT X FROM R").unwrap(), ["1"]);
    a.run(&format!("CREATE XML SCHEMA ANY_ORDER AS '{LOOSE}'")).unwrap();
    a.commit().unwrap();
    assert_eq!(one(&a, &format!("XMLSERIALIZE(CONTENT {validated} AS VARCHAR(60))")), SOME, "the least name now");
    assert_eq!(refused(&a, "SELECT X FROM V").1, "2201R", "the one it named");
    assert_eq!(refused(&a, "SELECT X FROM C").1, "2201R", "the one its type named");
    assert_eq!(rows(&a, "SELECT X FROM R").unwrap(), ["1"], "the one its element type named");
    assert_eq!(one(&a, &format!("XMLSERIALIZE(CONTENT {cast} AS VARCHAR(60))")), SOME);
    let db = Database::new();
    db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
    let s = db.session("A");
    assert_eq!(refused(&s, "SELECT X FROM V").1, "2201R", "reopened");
    assert_eq!(restrict(&s), Some(-103));
    s.run("DROP XML SCHEMA ANY_ORDER RESTRICT").unwrap();
    s.commit().unwrap();
    let views = "SELECT COUNT(*) FROM INFORMATION_SCHEMA.VIEWS WHERE TABLE_NAME IN ('V', 'C', 'R')";
    assert_eq!(rows(&s, views).unwrap(), ["3"]);
    s.run("DROP XML SCHEMA O CASCADE").unwrap();
    assert_eq!(rows(&s, views).unwrap(), ["0"]);
    s.commit().unwrap();
    db.close().unwrap();
    drop((db, a, crashed));
}

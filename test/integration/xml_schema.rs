//! Registered XML Schemas [ISO/IEC 9075-14:2006 draft §4.2.6; RULES.md
//! D106]: CREATE and DROP XML SCHEMA, the documents a schema reads among
//! those registered, what depends on one, the permanently registered ones,
//! and their recovery.

use std::sync::Arc;

use turborel::db::{Completion, Database, OpenOptions, Session};
use turborel::disk::io::{FileSystem, OsFileSystem};
use turborel::error::SqlError;
use turborel::value::{Binary, Value};

use crate::common::{Scratch, after_crash};

/// The XML Schema namespace, as a schema document declares it.
const XS: &str = r#"xmlns:xs="http://www.w3.org/2001/XMLSchema""#;

/// The schema document of target namespace `tns`, none if empty, of
/// `body`.
fn doc(tns: &str, body: &str) -> String {
    let target = if tns.is_empty() { String::new() } else { format!(r#" targetNamespace="{tns}" xmlns:t="{tns}""#) };
    format!("<xs:schema {XS}{target}>{body}</xs:schema>")
}

/// A schema document of target namespace `tns` importing namespace
/// `imported` and declaring element `e` of its complex type `ct`.
fn importing(tns: &str, imported: &str) -> String {
    doc(tns, &format!(r#"<xs:import namespace="{imported}"/><xs:element name="e" xmlns:i="{imported}" type="i:ct"/>"#))
}

/// `text` as a character string literal.
fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// The SQLCODE and SQLSTATE of a refusal.
fn refused(result: Result<Completion, SqlError>) -> (i32, String) {
    let e = result.expect_err("refused");
    (e.sqlcode(), e.sqlstate().as_str().to_owned())
}

/// `CREATE XML SCHEMA name LOCATION location AS document`.
fn create(name: &str, (location, document): (&str, &str)) -> String {
    format!("CREATE XML SCHEMA {name} LOCATION {} AS {}", quoted(location), quoted(document))
}

/// Registers `name` in `s`, of location `location` and document `document`,
/// and commits.
fn register(s: &Session, name: &str, (location, document): (&str, &str)) {
    let sql = create(name, (location, document));
    s.run(&sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    s.commit().unwrap();
}

/// What registering `name` of `document` in `s` refuses with, rolled back.
fn refusal(s: &Session, name: &str, document: &str) -> (i32, String) {
    let got = refused(s.run(&create(name, ("", document))));
    s.rollback().unwrap();
    got
}

/// A database of schemas A and B, owned by A and B.
fn two() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION B").unwrap();
    db
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §4.2.6] RULES.md D106:
/// a registered XML Schema of a name registered already is refused,
/// SQLCODE -103, and so is one in a schema not there (SQLSTATE 3F000), or
/// one another user's (SQLCODE -104); the first is kept, and its
/// components are there to be imported.
#[test]
fn a_name_registered_is_refused_4_2_6() {
    let db = two();
    let a = db.session("A");
    register(&a, "S", ("s.xsd", &doc("urn:s", r#"<xs:complexType name="ct"/>"#)));
    assert_eq!(refusal(&a, "S", &doc("urn:other", "")), (-103, "2A000".to_owned()));
    assert_eq!(refusal(&a, "NOWHERE.S", &doc("", "")), (-103, "3F000".to_owned()));
    assert_eq!(refusal(&a, "B.S", &doc("", "")).0, -104);
    register(&a, "T", ("t.xsd", &importing("urn:t", "urn:s")));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §4.2.6, §10.15 GR3,
/// §10.16] RULES.md D106: the document is a character string, a binary
/// string or an XML document, evaluated once, a subquery's or a dynamic
/// parameter's too, a `?` alone a character large object; text no
/// well-formed document is SQLSTATE 2200M, a document no schema 22000, the
/// null value and an XML value of no document 2200L, and a value of
/// another type SQLCODE -103.
#[test]
fn the_document_is_a_string_or_an_xml_document_4_2_6() {
    let db = two();
    let a = db.session("A");
    let text = doc("urn:t", r#"<xs:element name="e"/>"#);
    let octets = Value::Binary(Binary::new(text.clone().into_bytes())).to_literal();
    a.run("CREATE TABLE DOCS (D CLOB(1M))").unwrap();
    a.run(&format!("INSERT INTO DOCS VALUES ({})", quoted(&text))).unwrap();
    for (name, document) in [
        ("BIN", octets),
        ("PARSED", format!("XMLPARSE(DOCUMENT {} PRESERVE WHITESPACE)", quoted(&text))),
        ("QUERIED", "(SELECT D FROM DOCS)".to_owned()),
    ] {
        a.run(&format!("CREATE XML SCHEMA {name} AS {document}")).unwrap_or_else(|e| panic!("{name}: {e:?}"));
    }
    let prepared = a.prepare("CREATE XML SCHEMA PARAM AS ?").unwrap();
    a.execute_prepared(&prepared, &[Value::Char(text.clone())]).unwrap();
    a.commit().unwrap();
    for name in ["BIN", "PARSED", "QUERIED", "PARAM"] {
        assert_eq!(refusal(&a, name, &text).0, -103, "{name} is registered");
    }
    for (document, expected) in [
        ("'<xs:schema'", (-301, "2200M")),
        ("'<a/>'", (-301, "22000")),
        ("CAST(NULL AS VARCHAR(9))", (-301, "2200L")),
        ("XMLCONCAT(XMLPARSE(CONTENT '<a/>' PRESERVE WHITESPACE), XMLPARSE(CONTENT '<b/>' PRESERVE WHITESPACE))", (-301, "2200L")),
        ("42", (-103, "2A000")),
    ] {
        let got = refused(a.run(&format!("CREATE XML SCHEMA BAD AS {document}")));
        assert_eq!((got.0, got.1.as_str()), expected, "{document}");
        a.rollback().unwrap();
    }
}

/// Verifies: [EXTENSION] [Schema1 §4.2.1, §4.2.3] RULES.md D106: an
/// `<include>` finds a registered XML Schema by its LOCATION, an
/// `<import>` by its namespace, of those the owner of its schema may use
/// alone, and nothing else: a reference to a component of one not found
/// is refused (SQLSTATE 22000).
#[test]
fn includes_and_imports_resolve_among_registered_schemas() {
    let db = two();
    let (a, b) = (db.session("A"), db.session("B"));
    register(&a, "PART", ("part.xsd", &doc("urn:m", r#"<xs:complexType name="pt"/>"#)));
    register(&a, "OTHER", ("other.xsd", &doc("urn:o", r#"<xs:complexType name="ct"/>"#)));
    let main = doc(
        "urn:m",
        r#"<xs:include schemaLocation="part.xsd"/><xs:import namespace="urn:o"/><xs:element name="m" type="t:pt"/>
           <xs:element name="n" xmlns:o="urn:o" type="o:ct"/>"#,
    );
    let sql = "CREATE XML SCHEMA MAIN LOCATION 'main.xsd' AS CAST(? AS CLOB(1M))";
    a.execute_prepared(&a.prepare(sql).unwrap(), &[Value::Char(main)]).unwrap();
    a.commit().unwrap();
    let included = doc("urn:m", r#"<xs:include schemaLocation="nowhere.xsd"/><xs:element name="m" type="t:pt"/>"#);
    assert_eq!(refusal(&a, "LOST", &included), (-301, "22000".to_owned()), "no registered XML Schema is at nowhere.xsd");
    assert_eq!(refusal(&b, "IMPORTER", &importing("urn:u", "urn:o")), (-301, "22000".to_owned()), "B may not use A.OTHER");
    register(&b, "OWN", ("own.xsd", &doc("urn:o", r#"<xs:complexType name="ct"/>"#)));
    register(&b, "IMPORTER", ("", &importing("urn:u", "urn:o")));
}

/// Verifies: [EXTENSION] RULES.md D106, [ISO/IEC 9075-2:2023 §11.44 SR4]
/// as of a collation: DROP XML SCHEMA RESTRICT is refused (SQLCODE -103)
/// while a registered XML Schema uses it, directly or through another;
/// CASCADE drops those with it; one used by none drops with RESTRICT; one
/// not registered is refused; only the owner of its schema drops one
/// (SQLCODE -104).
#[test]
fn drop_restrict_is_refused_while_used_and_cascade_drops_users() {
    let db = two();
    let (a, b) = (db.session("A"), db.session("B"));
    register(&a, "BASE", ("base.xsd", &doc("urn:b", r#"<xs:complexType name="ct"/>"#)));
    let derived = r#"<xs:import namespace="urn:b"/><xs:complexType name="ct" xmlns:b="urn:b"><xs:complexContent><xs:extension base="b:ct"/></xs:complexContent></xs:complexType>"#;
    register(&a, "MID", ("mid.xsd", &doc("urn:m", derived)));
    register(&a, "TOP", ("top.xsd", &importing("urn:t", "urn:m")));
    register(&a, "ALONE", ("alone.xsd", &doc("urn:a", "")));
    assert_eq!(refused(b.run("DROP XML SCHEMA A.ALONE CASCADE")).0, -104);
    b.rollback().unwrap();
    let (code, state) = refused(a.run("DROP XML SCHEMA BASE RESTRICT"));
    assert_eq!((code, state.as_str()), (-103, "2A000"));
    a.rollback().unwrap();
    assert_eq!(refused(a.run("DROP XML SCHEMA MID RESTRICT")).0, -103);
    a.rollback().unwrap();
    a.run("DROP XML SCHEMA ALONE RESTRICT").unwrap();
    a.run("DROP XML SCHEMA BASE CASCADE").unwrap();
    a.commit().unwrap();
    for gone in ["BASE", "MID", "TOP", "ALONE"] {
        assert_eq!(refused(a.run(&format!("DROP XML SCHEMA {gone} CASCADE"))).0, -103, "{gone} is gone");
        a.rollback().unwrap();
    }
    register(&a, "MID", ("mid.xsd", &doc("urn:m", "")));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §4.2.6 Table 1]
/// RULES.md D106: the XML Schemas of the namespaces of xs, xsi and sqlxml
/// are registered always, as INFORMATION_SCHEMA.XS, XSI and SQLXML, and
/// none is dropped (SQLCODE -103).
#[test]
fn the_permanent_schemas_are_never_dropped_4_2_6() {
    let db = two();
    let a = db.session("A");
    for name in ["XS", "XSI", "SQLXML"] {
        let (code, _) = refused(a.run(&format!("DROP XML SCHEMA INFORMATION_SCHEMA.{name} CASCADE")));
        assert_eq!(code, -103, "{name}");
        a.rollback().unwrap();
    }
    let (code, _) = refused(a.run("DROP XML SCHEMA INFORMATION_SCHEMA.NONE CASCADE"));
    assert_eq!(code, -104, "INFORMATION_SCHEMA is no one's");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.2 SR3, GR5] RULES.md
/// D106: DROP SCHEMA RESTRICT is refused while the schema holds a registered
/// XML Schema; CASCADE drops it, and those that use it.
#[test]
fn drop_schema_drops_its_registered_schemas() {
    let db = two();
    let a = db.session("A");
    register(&a, "S", ("s.xsd", &doc("urn:s", r#"<xs:complexType name="ct"/>"#)));
    register(&a, "T", ("t.xsd", &importing("urn:t", "urn:s")));
    assert_eq!(refused(a.run("DROP SCHEMA A RESTRICT")).0, -103);
    a.rollback().unwrap();
    a.run("DROP SCHEMA A CASCADE").unwrap();
    a.commit().unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    register(&a, "S", ("s.xsd", &doc("urn:s", "")));
}

/// Verifies: [DESIGN] RULES.md D25, D106: registered XML Schemas survive
/// reopening, from the log alone after a failure and after a checkpoint
/// and a close, one that includes another, registered of a document of
/// line ends, which no character string literal holds, still reading
/// it: an import of its namespace finds its type from the one included,
/// it is registered still, and DROP RESTRICT of the one it includes is
/// still refused; one registered and not committed at the failure is not.
#[test]
fn registered_schemas_survive_reopening() {
    for checkpointed in [false, true] {
        let scratch = Scratch::new(if checkpointed { "xml-schema-checkpointed" } else { "xml-schema-failed" });
        let (fs, dir): (Arc<dyn FileSystem>, _) = (Arc::new(OsFileSystem), scratch.path("db"));
        let crashed = Database::new();
        crashed.open_files(&fs, &dir, &OpenOptions::default()).unwrap();
        crashed.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
        let a = crashed.session("A");
        register(&a, "PART", ("part.xsd", &doc("urn:m", r#"<xs:complexType name="ct"/>"#)));
        let main = doc("urn:m", "<xs:include schemaLocation=\"part.xsd\"/>\n<xs:element name=\"m\" type=\"t:ct\"/>");
        a.execute_prepared(&a.prepare("CREATE XML SCHEMA MAIN LOCATION 'main.xsd' AS ?").unwrap(), &[Value::Char(main)]).unwrap();
        a.commit().unwrap();
        if checkpointed {
            crashed.checkpoint().unwrap();
            crashed.close().unwrap();
        } else {
            a.run(&create("LATE", ("", &doc("", "")))).unwrap();
        }
        let db = Database::new();
        db.open_files(&if checkpointed { Arc::clone(&fs) } else { after_crash() }, &dir, &OpenOptions::default()).unwrap();
        let s = db.session("A");
        assert_eq!(refusal(&s, "MAIN", &doc("", "")).0, -103, "registered still");
        assert_eq!(refused(s.run("DROP XML SCHEMA PART RESTRICT")).0, -103);
        s.rollback().unwrap();
        register(&s, "USER_OF_MAIN", ("", &importing("urn:u", "urn:m")));
        register(&s, "LATE", ("", &doc("", "")));
        db.close().unwrap();
        drop((db, a, crashed));
    }
}

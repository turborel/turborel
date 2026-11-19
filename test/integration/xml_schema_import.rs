//! XQuery's Schema Import Feature [XQuery §4.10, §5.2.1] in XMLQUERY
//! [ISO/IEC 9075-14:2006 draft §6.18]: a registered XML Schema imported by
//! its target namespace, its types and declarations in scope of the
//! sequence types [XQuery §2.5.4.3 to §2.5.4.6].

use turborel::db::{Database, Session};
use turborel::error::SqlError;

use crate::xml_validate::{one, registered};

/// A schema document of target namespace `urn:s`: a type `base` and its
/// extension `more`, a simple type `small`, elements `item` and `big`, the
/// latter of `item`'s substitution group, a nillable `n`, and an attribute
/// `code` an element `tagged` refers to.
const SHAPES: &str = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:s="urn:s" targetNamespace="urn:s" elementFormDefault="qualified"><xs:complexType name="base"><xs:sequence><xs:element name="v" type="xs:int"/></xs:sequence></xs:complexType><xs:complexType name="more"><xs:complexContent><xs:extension base="s:base"><xs:sequence><xs:element name="w" type="xs:int"/></xs:sequence></xs:extension></xs:complexContent></xs:complexType><xs:simpleType name="small"><xs:restriction base="xs:int"><xs:maxInclusive value="9"/></xs:restriction></xs:simpleType><xs:element name="item" type="s:base"/><xs:element name="big" type="s:more" substitutionGroup="s:item"/><xs:element name="n" type="s:small" nillable="true"/><xs:attribute name="code" type="s:small"/><xs:element name="tagged"><xs:complexType><xs:attribute ref="s:code"/></xs:complexType></xs:element></xs:schema>"#;

/// The prolog that imports SHAPES, its prefix `s`.
const IMPORT: &str = "import schema namespace s = \"urn:s\"; ";

/// A database of `registered`'s, A also registering SHAPES as A.SHAPES.
fn database() -> Database {
    let db = registered();
    let a = db.session("A");
    a.run(&format!("CREATE XML SCHEMA SHAPES LOCATION 'shapes.xsd' AS '{SHAPES}'")).unwrap();
    a.commit().unwrap();
    db
}

/// XMLQUERY of `query`, its context item the document `xml`.
pub fn xmlquery(query: &str, xml: &str) -> String {
    let query = query.replace('\'', "''");
    format!("XMLQUERY('{query}' PASSING BY REF XMLPARSE(DOCUMENT '{xml}' PRESERVE WHITESPACE) RETURNING SEQUENCE EMPTY ON EMPTY)")
}

/// The value of `query` of the context item `xml`, serialized.
pub fn value(s: &Session, query: &str, xml: &str) -> String {
    one(s, &format!("XMLSERIALIZE(CONTENT {} AS VARCHAR(200))", xmlquery(query, xml)))
}

/// The XQuery error code `query` of the context item `xml` fails with;
/// rolled back.
pub fn code(s: &Session, query: &str, xml: &str) -> String {
    let sql = format!("VALUES ({})", xmlquery(query, xml));
    let e: SqlError = crate::common::fetched(s, &sql).expect_err(&sql);
    s.rollback().unwrap();
    assert_eq!(e.sqlstate().as_str(), "10000", "{sql}: {e:?}");
    e.msg.split_whitespace().find(|w| w.starts_with("err:")).map(|w| w.trim_end_matches(':').to_owned()).unwrap_or(e.msg)
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery §4.10, §2.5.4.3,
/// §2.5.4.5] an imported schema's type definitions are in-scope schema
/// types: an element validated by an extension of `s:base` matches
/// `element(*, s:base)`, not `element(*, s:small)`; an attribute of
/// `s:small` matches `attribute(*, xs:int)`, as `s:small` restricts it; an
/// element no validation typed matches neither; a type no imported schema
/// defines is err:XPST0008.
#[test]
fn imported_types_are_in_scope_4_10() {
    let db = database();
    let a = db.session("A");
    let big = r#"<big xmlns="urn:s"><v>1</v><w>2</w></big>"#;
    let test = |t: &str| format!("{IMPORT}(validate {{ . }})/s:big instance of {t}");
    assert_eq!(value(&a, &test("element(*, s:base)"), big), "true");
    assert_eq!(value(&a, &test("element(s:big, s:more)"), big), "true");
    assert_eq!(value(&a, &test("element(*, xs:anyType)"), big), "true");
    assert_eq!(value(&a, &test("element(*, s:small)"), big), "false");
    assert_eq!(value(&a, &format!("{IMPORT}/s:big instance of element(*, s:base)"), big), "false", "unvalidated");
    let tagged = r#"<tagged xmlns="urn:s" xmlns:s="urn:s" s:code="4"/>"#;
    let code_test = |t: &str| format!("{IMPORT}(validate {{ . }})/s:tagged/@s:code instance of {t}");
    assert_eq!(value(&a, &code_test("attribute(*, xs:int)"), tagged), "true");
    assert_eq!(value(&a, &code_test("attribute(s:code, s:small)"), tagged), "true");
    assert_eq!(value(&a, &code_test("attribute(*, xs:string)"), tagged), "false");
    assert_eq!(code(&a, &format!("{IMPORT}1 instance of element(*, s:nosuch)"), big), "err:XPST0008");
    assert_eq!(code(&a, "1 instance of element(*, s:base)", big), "err:XPST0081", "no prefix s without the import");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery §2.5.4.4,
/// §2.5.4.6] a schema element test matches an element of its declaration
/// or of the substitution group it heads, of its type, nilled only of a
/// nillable declaration; a schema attribute test an attribute of its
/// declaration; one of no imported declaration is err:XPST0008.
#[test]
fn schema_element_and_attribute_tests_2_5_4_4() {
    let db = database();
    let a = db.session("A");
    let validated = |path: &str, t: &str| format!("{IMPORT}(validate {{ . }}){path} instance of {t}");
    let big = r#"<big xmlns="urn:s"><v>1</v><w>2</w></big>"#;
    let item = r#"<item xmlns="urn:s"><v>1</v></item>"#;
    assert_eq!(value(&a, &validated("/s:big", "schema-element(s:item)"), big), "true", "of item's substitution group");
    assert_eq!(value(&a, &validated("/s:item", "schema-element(s:item)"), item), "true");
    assert_eq!(value(&a, &validated("/s:item", "schema-element(s:big)"), item), "false");
    assert_eq!(value(&a, &validated("", "document-node(schema-element(s:item))"), big), "true");
    let nil = r#"<n xmlns="urn:s" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:nil="true"/>"#;
    assert_eq!(value(&a, &validated("/s:n", "schema-element(s:n)"), nil), "true", "n is nillable");
    assert_eq!(value(&a, &validated("/s:n", "element(s:n, s:small)"), nil), "false", "nilled, of a test that is not");
    assert_eq!(value(&a, &validated("/s:n", "element(s:n, s:small?)"), nil), "true");
    let tagged = r#"<tagged xmlns="urn:s" xmlns:s="urn:s" s:code="4"/>"#;
    assert_eq!(value(&a, &validated("/s:tagged/@s:code", "schema-attribute(s:code)"), tagged), "true");
    assert_eq!(code(&a, &format!("{IMPORT}1 instance of schema-element(s:nosuch)"), item), "err:XPST0008");
    assert_eq!(code(&a, &format!("{IMPORT}1 instance of schema-attribute(s:v)"), item), "err:XPST0008");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQuery §4.10] the
/// static errors of schema imports: a namespace imported twice
/// err:XQST0058; one no registered XML Schema the user may use has
/// err:XQST0059, B's of A's; a prefix bound to no namespace err:XQST0057;
/// a namespace's names defined by two imported schemas from different
/// documents err:XQST0035, not from one; a complex type is no atomic item
/// type, err:XPST0051, while a simple type of the atomic variety is one.
#[test]
fn what_schema_imports_raise_4_10() {
    let db = database();
    let a = db.session("A");
    let doc = "<v/>";
    assert_eq!(code(&a, &format!("{IMPORT}import schema \"urn:s\"; 1"), doc), "err:XQST0058");
    assert_eq!(code(&a, "import schema \"urn:none\"; 1", doc), "err:XQST0059");
    assert_eq!(code(&db.session("B"), &format!("{IMPORT}1"), doc), "err:XQST0059");
    assert_eq!(code(&a, "import schema namespace e = \"\"; 1", doc), "err:XQST0057");
    assert_eq!(code(&a, &format!("{IMPORT}1 instance of s:base"), doc), "err:XPST0051");
    assert_eq!(value(&a, &format!("{IMPORT}1 instance of s:small"), doc), "false");
    let x = |t: &str| {
        format!(
            r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:x"><xs:complexType name="shared"/><xs:complexType name="{t}"/></xs:schema>"#
        )
    };
    a.run(&format!("CREATE XML SCHEMA X1 LOCATION 'x1.xsd' AS '{}'", x("t1"))).unwrap();
    a.run(&format!("CREATE XML SCHEMA X2 LOCATION 'x2.xsd' AS '{}'", x("t2"))).unwrap();
    let y = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:y"><xs:import namespace="urn:x" schemaLocation="x2.xsd"/></xs:schema>"#;
    a.run(&format!("CREATE XML SCHEMA Y AS '{y}'")).unwrap();
    a.commit().unwrap();
    let both = |hint: &str| format!("import schema namespace x = \"urn:x\" at \"{hint}\"; import schema \"urn:y\"; 1");
    assert_eq!(code(&a, &both("x1.xsd"), doc), "err:XQST0035");
    assert_eq!(value(&a, &both("x2.xsd"), doc), "1");
    let hinted = |hint: &str| format!("import schema namespace x = \"urn:x\"{hint}; 1 instance of element(*, x:t2)");
    assert_eq!(value(&a, &hinted(" at \"x2.xsd\""), doc), "false", "the location hint chooses X2");
    assert_eq!(code(&a, &hinted(""), doc), "err:XPST0008", "else X1, the least name, of no t2");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §4.2.6 Table 1] [XQuery §4.10] the
/// XML Schema namespace imports INFORMATION_SCHEMA.XS, the Schema for
/// Schemas, so a schema document validated is a schema element of
/// `xs:schema`; a view whose query imports a registered XML Schema
/// depends on it, DROP XML SCHEMA RESTRICT refused, SQLCODE -103.
#[test]
fn imports_of_the_schema_for_schemas_and_in_views_4_10() {
    let db = database();
    let a = db.session("A");
    let schema = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"><xs:element name="z"/></xs:schema>"#;
    let xs = "import schema \"http://www.w3.org/2001/XMLSchema\"; (validate { . })/xs:schema instance of schema-element(xs:schema)";
    assert_eq!(value(&a, xs, schema), "true");
    let query = xmlquery(&format!("{IMPORT}count(/s:item)"), r#"<item xmlns="urn:s"><v>1</v></item>"#);
    a.run(&format!("CREATE VIEW V AS SELECT XMLSERIALIZE(CONTENT {query} AS VARCHAR(10)) AS X FROM (VALUES (1)) AS T (N)")).unwrap();
    a.commit().unwrap();
    let e = a.run("DROP XML SCHEMA SHAPES RESTRICT").expect_err("V imports SHAPES");
    a.rollback().unwrap();
    assert_eq!(e.sqlcode(), -103);
    assert_eq!(one(&a, "(SELECT X FROM V)"), "1");
}

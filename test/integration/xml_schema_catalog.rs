//! Registered XML Schemas in the catalog [ISO/IEC 9075-14:2006 draft
//! §4.2.6, §4.7.1]: USAGE on them granted and revoked, as turborel does
//! it, and the Information Schema's rows of them [§21.4].

use turborel::db::{Database, Session};
use turborel::error::Code;

use crate::common::sorted;

/// A schema document of target namespace `urn:orders`: ORDER, of no
/// wildcard, and NOTE, whose content is any element, processed laxly, so
/// non-deterministic [§4.2.6].
const ORDERS_DOCUMENT: &str = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:orders"><xs:element name="order" type="xs:string"/><xs:element name="note"><xs:complexType><xs:sequence><xs:any processContents="lax"/></xs:sequence></xs:complexType></xs:element></xs:schema>"#;

/// A schema document of no target namespace, of one element, LINE.
const LINES_DOCUMENT: &str =
    r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"><xs:element name="line" type="xs:int"/></xs:schema>"#;

/// A schema document of target namespace `urn:more` that imports
/// `urn:orders` from `orders.xsd` and refers to its ORDER.
const MORE_DOCUMENT: &str = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:more" xmlns:o="urn:orders"><xs:import namespace="urn:orders" schemaLocation="orders.xsd"/><xs:element name="wrap"><xs:complexType><xs:sequence><xs:element ref="o:order"/></xs:sequence></xs:complexType></xs:element></xs:schema>"#;

/// A database of schemas A and B, owned by A and B, A holding the
/// registered XML Schemas A.ORDERS, at `orders.xsd`, and A.LINES, at
/// `lines.xsd`, registered by CREATE XML SCHEMA.
fn database() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION B").unwrap();
    let a = db.session("A");
    a.run(&format!("CREATE XML SCHEMA ORDERS LOCATION 'orders.xsd' AS '{ORDERS_DOCUMENT}'")).unwrap();
    a.run(&format!("CREATE XML SCHEMA LINES LOCATION 'lines.xsd' AS '{LINES_DOCUMENT}'")).unwrap();
    a.commit().unwrap();
    db
}

/// The SQLSTATE `sql` completed with in `s`: 00000, or a warning's.
fn completed(s: &Session, sql: &str) -> String {
    s.run(sql).unwrap_or_else(|e| panic!("{sql}: {e:?}")).sqlstate().as_str().to_owned()
}

/// The SQLCODE class and SQLSTATE of `sql`, refused in `s`.
fn refused(s: &Session, sql: &str) -> (Code, String) {
    let e = s.run(sql).expect_err(sql);
    (e.code, e.sqlstate().as_str().to_owned())
}

/// The registered XML Schemas of schema A that `s`'s user, PUBLIC or an
/// enabled role holds USAGE on, as USAGE_PRIVILEGES shows them.
fn usable(s: &Session) -> Vec<String> {
    let sql = "SELECT DISTINCT OBJECT_NAME FROM INFORMATION_SCHEMA.USAGE_PRIVILEGES WHERE OBJECT_TYPE = 'XML SCHEMA' \
               AND OBJECT_SCHEMA = 'A' AND ( GRANTEE IN ( 'PUBLIC', CURRENT_USER ) \
               OR GRANTEE IN ( SELECT ROLE_NAME FROM INFORMATION_SCHEMA.ENABLED_ROLES ) )";
    let found = sorted(s, sql);
    s.rollback().unwrap();
    found
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §4.7.1, §21.4]
/// [ISO/IEC 9075-2:2023 §12.2]: GRANT USAGE ON XML SCHEMA by its schema's
/// owner gives the grantee USAGE; USAGE_PRIVILEGES shows it, and the
/// owner's of the grantor _SYSTEM, grantable, OBJECT_TYPE XML SCHEMA; a
/// name of the session's default schema is found there; ROLLBACK undoes
/// a grant.
#[test]
fn usage_is_granted_by_the_owner() {
    let db = database();
    let (a, b) = (db.session("A"), db.session("B"));
    assert_eq!(usable(&b), Vec::<String>::new());
    a.run("GRANT USAGE ON XML SCHEMA A.ORDERS TO B").unwrap();
    a.rollback().unwrap();
    assert_eq!(usable(&b), Vec::<String>::new(), "rolled back");
    assert_eq!(completed(&a, "GRANT USAGE ON XML SCHEMA ORDERS TO B"), "00000");
    a.commit().unwrap();
    assert_eq!(usable(&b), ["ORDERS"]);
    let usage = "SELECT GRANTOR, GRANTEE, OBJECT_NAME, OBJECT_TYPE, PRIVILEGE_TYPE, IS_GRANTABLE FROM INFORMATION_SCHEMA.USAGE_PRIVILEGES \
                 WHERE OBJECT_SCHEMA = 'A'";
    assert_eq!(
        sorted(&a, usage),
        ["A,B,ORDERS,XML SCHEMA,USAGE,NO", "_SYSTEM,A,LINES,XML SCHEMA,USAGE,YES", "_SYSTEM,A,ORDERS,XML SCHEMA,USAGE,YES"]
    );
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §12.2 AR1, GR9, §12.3
/// SR4a, §12.2 SR2]: a grantee WITH GRANT OPTION grants in turn; one
/// without grants nothing, warning 01007; one holding no USAGE is refused,
/// SQLCODE -104; ALL PRIVILEGES is USAGE, to PUBLIC everyone's; another
/// privilege, or a name of no registered XML Schema, is SQLCODE -103; a
/// permanently registered XML Schema is everyone's, not grantably, so a
/// grant of it grants nothing.
#[test]
fn usage_is_granted_by_those_holding_it_grantably() {
    let db = database();
    let (a, b, c) = (db.session("A"), db.session("B"), db.session("C"));
    assert_eq!(refused(&b, "GRANT USAGE ON XML SCHEMA A.ORDERS TO C").0, Code::Privilege);
    b.rollback().unwrap();
    a.run("GRANT USAGE ON XML SCHEMA A.ORDERS TO B WITH GRANT OPTION").unwrap();
    a.run("GRANT USAGE ON XML SCHEMA A.LINES TO C").unwrap();
    a.commit().unwrap();
    assert_eq!(completed(&b, "GRANT USAGE ON XML SCHEMA A.ORDERS TO C"), "00000");
    b.commit().unwrap();
    assert_eq!(completed(&c, "GRANT USAGE ON XML SCHEMA A.LINES TO D"), "01007", "C holds it, not grantably");
    c.commit().unwrap();
    assert_eq!(usable(&c), ["LINES", "ORDERS"]);
    let d = db.session("D");
    assert_eq!(usable(&d), Vec::<String>::new());
    a.run("GRANT ALL PRIVILEGES ON XML SCHEMA A.LINES TO PUBLIC").unwrap();
    a.commit().unwrap();
    assert_eq!(usable(&d), ["LINES"]);
    assert_eq!(refused(&a, "GRANT SELECT ON XML SCHEMA A.LINES TO D").0, Code::SyntaxRule);
    assert_eq!(refused(&a, "GRANT USAGE ON XML SCHEMA A.NONE TO D").0, Code::SyntaxRule);
    assert_eq!(completed(&a, "GRANT USAGE ON XML SCHEMA INFORMATION_SCHEMA.XS TO D"), "01007", "everyone's, not grantably");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §12.7 GR9, GR12q, GR12ai]:
/// revoking USAGE whose grantee granted it on is refused with RESTRICT,
/// SQLSTATE 2B000, as the grant made by it would be abandoned; GRANT
/// OPTION FOR with CASCADE takes the option and the grant made by it,
/// leaving USAGE; then RESTRICT takes USAGE, abandoning nothing; revoking
/// what was not granted warns, 01006.
#[test]
fn revoke_takes_the_grants_it_abandons() {
    let db = database();
    let (a, b) = (db.session("A"), db.session("B"));
    a.run("GRANT USAGE ON XML SCHEMA A.ORDERS TO B WITH GRANT OPTION").unwrap();
    a.commit().unwrap();
    b.run("GRANT USAGE ON XML SCHEMA A.ORDERS TO C").unwrap();
    b.commit().unwrap();
    assert_eq!(refused(&a, "REVOKE USAGE ON XML SCHEMA A.ORDERS FROM B RESTRICT"), (Code::SyntaxRule, "2B000".to_owned()));
    a.rollback().unwrap();
    assert_eq!(completed(&a, "REVOKE GRANT OPTION FOR USAGE ON XML SCHEMA A.ORDERS FROM B CASCADE"), "00000");
    a.commit().unwrap();
    assert_eq!(usable(&b), ["ORDERS"], "B keeps USAGE");
    assert_eq!(usable(&db.session("C")), Vec::<String>::new(), "the grant by B abandoned");
    assert_eq!(completed(&b, "GRANT USAGE ON XML SCHEMA A.ORDERS TO C"), "01007");
    b.rollback().unwrap();
    assert_eq!(completed(&a, "REVOKE USAGE ON XML SCHEMA A.ORDERS FROM B RESTRICT"), "00000");
    a.commit().unwrap();
    assert_eq!(usable(&b), Vec::<String>::new());
    assert_eq!(completed(&a, "REVOKE USAGE ON XML SCHEMA A.ORDERS FROM B RESTRICT"), "01006", "none left to revoke");
    assert_eq!(completed(&a, "REVOKE USAGE ON XML SCHEMA INFORMATION_SCHEMA.XSI FROM B RESTRICT"), "01006");
    assert_eq!(refused(&a, "REVOKE USAGE ON XML SCHEMA A.NONE FROM B RESTRICT").0, Code::SyntaxRule);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.42.4, §12.2]: USAGE
/// granted to a role is held by a user once it sets the role; A, the
/// database's owner, may create the role.
#[test]
fn usage_granted_to_a_role_is_held_through_it() {
    let db = database();
    db.create_user("A", b"x").unwrap();
    let a = db.session("A");
    a.run("CREATE ROLE READER").unwrap();
    a.run("GRANT READER TO B").unwrap();
    a.run("GRANT USAGE ON XML SCHEMA A.LINES TO READER").unwrap();
    a.commit().unwrap();
    let b = db.session("B");
    assert_eq!(usable(&b), Vec::<String>::new());
    b.run("SET ROLE 'READER'").unwrap();
    assert_eq!(usable(&b), ["LINES"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §5.1]: XML, no key
/// word here, names a table where SCHEMA does not follow it, so ON XML
/// grants on the table.
#[test]
fn a_table_named_xml_is_granted_on_as_before() {
    let db = database();
    let a = db.session("A");
    a.run("CREATE TABLE XML (K INTEGER)").unwrap();
    assert_eq!(completed(&a, "GRANT SELECT ON XML TO B"), "00000");
    a.commit().unwrap();
    let b = db.session("B");
    assert_eq!(sorted(&b, "SELECT COUNT(*) FROM A.XML"), ["0"]);
}

/// The registered XML Schemas of schema B, as `schema.name`.
fn of_b(s: &Session) -> Vec<String> {
    let sql = "SELECT OBJECT_NAME FROM INFORMATION_SCHEMA.USAGE_PRIVILEGES WHERE OBJECT_TYPE = 'XML SCHEMA' AND OBJECT_SCHEMA = 'B'";
    let found = sorted(s, sql);
    s.rollback().unwrap();
    found
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §11.6 AR1a] [EXTENSION] [ISO/IEC
/// 9075-2:2023 §12.7 GR12a, GR12q]: CREATE XML SCHEMA in schema B imports
/// A.ORDERS only once B's owner holds USAGE on it, else the import finds
/// nothing and what it names is refused; once it does, revoking that USAGE
/// abandons B.MORE, so RESTRICT is refused, SQLSTATE 2B000, and CASCADE
/// drops it; revoking it from another, leaving B's USAGE, abandons
/// nothing.
#[test]
fn revoke_drops_the_registered_xml_schemas_it_abandons() {
    let db = database();
    let (a, b) = (db.session("A"), db.session("B"));
    let more = format!("CREATE XML SCHEMA MORE LOCATION 'more.xsd' AS '{MORE_DOCUMENT}'");
    assert!(b.run(&more).is_err(), "B may not use A.ORDERS, so its library has none");
    b.rollback().unwrap();
    a.run("GRANT USAGE ON XML SCHEMA A.ORDERS TO B, C").unwrap();
    a.commit().unwrap();
    b.run(&more).unwrap();
    b.commit().unwrap();
    assert_eq!(of_b(&b), ["MORE"]);
    assert_eq!(completed(&a, "REVOKE USAGE ON XML SCHEMA A.ORDERS FROM C RESTRICT"), "00000", "C's USAGE holds up nothing");
    assert_eq!(refused(&a, "REVOKE USAGE ON XML SCHEMA A.ORDERS FROM B RESTRICT"), (Code::SyntaxRule, "2B000".to_owned()));
    a.rollback().unwrap();
    assert_eq!(of_b(&b), ["MORE"], "refused, so nothing changed");
    a.run("REVOKE USAGE ON XML SCHEMA A.ORDERS FROM B CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(of_b(&b), Vec::<String>::new(), "B.MORE dropped with the USAGE it needed");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §4.7.1]: the USAGE
/// granted on a registered XML Schema goes with it when it is dropped, so
/// one registered again under its name is granted to no one.
#[test]
fn a_drop_takes_the_grants_with_it() {
    let db = database();
    let a = db.session("A");
    a.run("GRANT USAGE ON XML SCHEMA A.LINES TO B").unwrap();
    a.run("DROP XML SCHEMA A.LINES RESTRICT").unwrap();
    a.run(&format!("CREATE XML SCHEMA LINES LOCATION 'lines.xsd' AS '{LINES_DOCUMENT}'")).unwrap();
    a.commit().unwrap();
    assert_eq!(usable(&db.session("B")), Vec::<String>::new());
}

/// A view of B that validates by A.ORDERS.
const VALIDATING_VIEW: &str = "CREATE VIEW V AS SELECT XMLVALIDATE(DOCUMENT XMLPARSE(DOCUMENT '<o:order xmlns:o=\"urn:orders\">x</o:order>' PRESERVE WHITESPACE) \
                               ACCORDING TO XMLSCHEMA ID A.ORDERS) AS X FROM (VALUES (1)) AS T (N)";

/// Verifies: [ISO/IEC 9075-14:2006 draft §11.6 AR1a, §6.1 SR4, §6.20]
/// [EXTENSION] [ISO/IEC 9075-2:2023 §12.7 GR12a, GR12q]: a column of an
/// XML type of XMLSCHEMA, or a view validating by a registered XML
/// Schema, needs its schema's owner to hold USAGE on it, SQLCODE -104
/// without; once B holds it, revoking it is refused, SQLSTATE 2B000,
/// even with CASCADE while B's table has a column of the type, as nothing
/// drops a column for it; with the table dropped, RESTRICT is refused for
/// the view and CASCADE destroys it.
#[test]
fn revoke_abandons_what_names_a_registered_xml_schema() {
    let db = database();
    let (a, b) = (db.session("A"), db.session("B"));
    let table = "CREATE TABLE T (X XML(DOCUMENT(XMLSCHEMA ID A.ORDERS)))";
    assert_eq!(refused(&b, table).0, Code::Privilege, "B holds no USAGE on A.ORDERS");
    b.rollback().unwrap();
    assert_eq!(refused(&b, VALIDATING_VIEW).0, Code::Privilege);
    b.rollback().unwrap();
    a.run("GRANT USAGE ON XML SCHEMA A.ORDERS TO B").unwrap();
    a.commit().unwrap();
    b.run(table).unwrap();
    b.run(VALIDATING_VIEW).unwrap();
    b.commit().unwrap();
    let revoke = |behavior: &str| refused(&a, &format!("REVOKE USAGE ON XML SCHEMA A.ORDERS FROM B {behavior}"));
    assert_eq!(revoke("CASCADE"), (Code::SyntaxRule, "2B000".to_owned()), "its column is dropped by nothing");
    a.rollback().unwrap();
    b.run("DROP TABLE T CASCADE").unwrap();
    b.commit().unwrap();
    assert_eq!(revoke("RESTRICT"), (Code::SyntaxRule, "2B000".to_owned()), "the view validates by it");
    a.rollback().unwrap();
    a.run("REVOKE USAGE ON XML SCHEMA A.ORDERS FROM B CASCADE").unwrap();
    a.commit().unwrap();
    let views = "SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = 'B'";
    assert_eq!(sorted(&b, views), Vec::<String>::new(), "V destroyed with the USAGE it needed");
}

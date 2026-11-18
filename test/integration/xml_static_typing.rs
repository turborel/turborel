//! XQuery's Static Typing Feature [XQuery §5.2.3] in XMLQUERY, XMLEXISTS
//! and XMLTABLE [ISO/IEC 9075-14:2006 draft §6.18 SR6]: in effect by
//! XQUERY.STATIC_TYPING or a query's option, each type error raised as the
//! statement is prepared, the values passed of their formal types
//! [§10.21].

use std::sync::Arc;

use turborel::db::{Database, OpenOptions, Session};
use turborel::disk::io::FileSystem;
use turborel::error::{SqlError, SqlState};

use crate::common::{Scratch, after_crash, fetched, os, rows};

/// A schema document of target namespace `urn:s`: a type `base` and its
/// extension `more`, elements `item` of `base` and `big` of `more` in its
/// substitution group, and a nillable `n` of a restriction of `xs:int`.
const SHAPES: &str = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:s="urn:s" targetNamespace="urn:s" elementFormDefault="qualified"><xs:complexType name="base"><xs:sequence><xs:element name="v" type="xs:int"/></xs:sequence></xs:complexType><xs:complexType name="more"><xs:complexContent><xs:extension base="s:base"><xs:sequence><xs:element name="w" type="xs:int"/></xs:sequence></xs:extension></xs:complexContent></xs:complexType><xs:simpleType name="small"><xs:restriction base="xs:int"><xs:maxInclusive value="9"/></xs:restriction></xs:simpleType><xs:element name="item" type="s:base"/><xs:element name="big" type="s:more" substitutionGroup="s:item"/><xs:element name="n" type="s:small" nillable="true"/></xs:schema>"#;

/// Schema A's table T of an untyped XML column, empty, and S of a column
/// of SHAPES's documents.
fn database() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, X XML(CONTENT(UNTYPED)))").unwrap();
    let a = db.session("A");
    a.run(&format!("CREATE XML SCHEMA SHAPES AS '{SHAPES}'")).unwrap();
    a.run("CREATE TABLE S (X XML(DOCUMENT(XMLSCHEMA ID SHAPES)))").unwrap();
    a.commit().unwrap();
    db
}

/// The error `sql`, a query, fails with.
fn failure(a: &Session, sql: &str) -> SqlError {
    fetched(a, sql).expect_err(sql)
}

/// Whether `error` is the XQuery error `code` [§6.18 GR3].
fn is_xquery(error: &SqlError, code: &str) -> bool {
    error.sqlstate() == SqlState::XQUERY_ERROR && error.to_string().contains(&format!("err:{code}"))
}

/// The prolog of the option that puts static typing in effect.
const OPTION: &str = "declare namespace t = \"urn:turborel:option\"; declare option t:static-typing \"on\"; ";

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 SR6, GR3] [XQuery §5.2.3,
/// §2.2.3.1, §4.16] [EXTENSION] static typing is off by default; ON of
/// XQUERY.STATIC_TYPING, or the option static-typing "on", puts it in
/// effect, and a type error is raised as the statement is prepared, of a
/// table of no rows; the option "off" takes none out.
#[test]
fn static_typing_is_in_effect_as_asked_6_18() {
    let db = database();
    let a = db.session("A");
    let query = |prolog: &str| {
        format!("SELECT XMLQUERY('{prolog}$x/a + 1' PASSING BY REF X AS \"x\" RETURNING SEQUENCE BY REF EMPTY ON EMPTY) FROM T")
    };
    assert_eq!(rows(&a, &query("")).unwrap().len(), 0, "off by default, nothing is evaluated");
    assert!(is_xquery(&failure(&a, &query(OPTION)), "XPTY0004"), "the option puts it in effect");
    a.run("SET XQUERY.STATIC_TYPING = 'ON'").unwrap();
    assert!(is_xquery(&failure(&a, &query("")), "XPTY0004"));
    let off = "declare namespace t = \"urn:turborel:option\"; declare option t:static-typing \"off\"; ";
    assert!(is_xquery(&failure(&a, &query(off)), "XPTY0004"), "an option suppresses no static error");
    let one = "SELECT XMLQUERY('exactly-one($x/a) + 1' PASSING BY REF X AS \"x\" RETURNING SEQUENCE BY REF EMPTY ON EMPTY) FROM T";
    assert_eq!(fetched(&a, one).unwrap(), 0);
    let exists = "SELECT K FROM T WHERE XMLEXISTS('$x/a/@b/c' PASSING BY REF X AS \"x\")";
    assert!(is_xquery(&failure(&a, exists), "XPST0005"));
    let table = "SELECT * FROM T, XMLTABLE('$x/a' PASSING BY REF X AS \"x\" COLUMNS N INTEGER PATH 'b + 1') AS U";
    assert!(is_xquery(&failure(&a, table), "XPTY0004"), "a column's pattern of each item of the row pattern");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.21 SR3c, SR3d] [X3.135-1986
/// §5.25 SR10] [XQueryFS §7.1.4] a value passed whose expression is known
/// not null, a column reference to a NOT NULL column or a literal, is of
/// its formal type without `?`; another, a nullable column's or one
/// computed, with it, which a parameter of one value takes none of.
#[test]
fn known_not_null_values_are_passed_as_one_10_21() {
    let db = database();
    let a = db.session("A");
    a.run("CREATE TABLE N (P VARCHAR(5) NOT NULL, Q VARCHAR(5))").unwrap();
    a.run("SET XQUERY.STATIC_TYPING = 'ON'").unwrap();
    let joined = |passed: &str| {
        format!(
            "SELECT XMLQUERY('string-join((\"a\", \"b\"), $s)' PASSING BY VALUE {passed} AS \"s\" RETURNING SEQUENCE EMPTY ON EMPTY) FROM N"
        )
    };
    for known in ["P", "N.P", "'-'"] {
        assert_eq!(fetched(&a, &joined(known)).unwrap(), 0, "{known} is known not null");
    }
    for nullable in ["Q", "P || Q", "CAST(P AS VARCHAR(6))", "UPPER(P)"] {
        assert!(is_xquery(&failure(&a, &joined(nullable)), "XPTY0004"), "{nullable} may be null");
    }
}

/// An ill-typed query of an XML value `$x`: `$x/a`'s typed values are
/// `xs:untypedAtomic*`, which `+` takes one of at most [XQueryFS §4.5.2].
const ILL: &str = "XMLEXISTS('$x/a + 1' PASSING BY REF X AS \"x\")";

/// A definition of each kind of an ill-typed query, named `n`: a view, a
/// table CHECK, an assertion, a trigger, a function, a generation
/// expression, a domain constraint, and a schema's view.
fn definitions(n: &str) -> Vec<String> {
    let value = ILL.replace("BY REF X", "BY REF VALUE");
    let p = ILL.replace("BY REF X", "BY REF P");
    let new = ILL.replace("BY REF X", "BY REF N.X");
    vec![
        format!("CREATE VIEW V{n} (Y) AS SELECT {ILL} FROM T"),
        format!("ALTER TABLE T ADD CONSTRAINT C{n} CHECK (NOT {ILL})"),
        format!("CREATE ASSERTION A{n} CHECK (NOT EXISTS (SELECT * FROM T WHERE {ILL}))"),
        format!("CREATE TRIGGER G{n} AFTER INSERT ON T REFERENCING NEW ROW AS N FOR EACH ROW WHEN ({new}) DELETE FROM T WHERE K < 0"),
        format!("CREATE FUNCTION F{n} (P XML(CONTENT(UNTYPED))) RETURNS BOOLEAN CONTAINS SQL RETURN {p}"),
        format!("ALTER TABLE T ADD COLUMN E{n} BOOLEAN GENERATED ALWAYS AS ({ILL})"),
        format!("CREATE DOMAIN D{n} AS XML(CONTENT(UNTYPED)) CHECK (VALUE IS NULL OR {value})"),
        format!("CREATE SCHEMA S{n} CREATE VIEW W (Y) AS SELECT {ILL} FROM A.T"),
    ]
}

/// Makes the definition `sql` in `a`: a schema's as direct SQL [ISO/IEC
/// 9075:1992 §20.1].
fn make(a: &Session, sql: &str) -> Result<(), SqlError> {
    if sql.starts_with("CREATE SCHEMA") { a.direct(sql).map(drop) } else { a.run(sql).map(drop) }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 SR6, §4.2.6] [EXTENSION]
/// RULES.md D106: a definition of each kind is typed as it is made, as
/// XQUERY.STATIC_TYPING is for the session that makes it or as its
/// query's option asks; one made with static typing off is read as it was
/// made by a session with it on; a session's value is its own, over the
/// database's.
#[test]
fn definitions_are_typed_as_made() {
    let db = database();
    let a = db.session("A");
    for sql in definitions("0") {
        make(&a, &sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
    a.run("SET XQUERY.STATIC_TYPING = 'ON'").unwrap();
    assert_eq!(fetched(&a, "SELECT * FROM V0").unwrap(), 0, "read as it was made");
    assert_eq!(fetched(&a, "SELECT * FROM S0.W").unwrap(), 0, "read as it was made");
    a.run("INSERT INTO T (K, X) VALUES (1, XMLPARSE(CONTENT '<b/>' STRIP WHITESPACE))").unwrap();
    assert_eq!(fetched(&a, "SELECT F0(X) FROM T").unwrap(), 1, "read as it was made");
    a.rollback().unwrap();
    for sql in definitions("1") {
        assert!(is_xquery(&make(&a, &sql).unwrap_err(), "XPTY0004"), "of the session's value: {sql}");
    }
    a.run("SET XQUERY.STATIC_TYPING = 'OFF'").unwrap();
    let option = format!("CREATE VIEW O (Y) AS SELECT {} FROM T", ILL.replace("'$x", &format!("'{OPTION}$x")));
    assert!(is_xquery(&a.run(&option).unwrap_err(), "XPTY0004"), "of its own option");
    a.commit().unwrap();
    db.create_user("A", b"x").unwrap();
    db.set_owner("A").unwrap();
    a.run("SET DATABASE XQUERY.STATIC_TYPING = 'ON'").unwrap();
    let b = db.session("A");
    assert!(is_xquery(&b.run(&definitions("2")[0]).unwrap_err(), "XPTY0004"), "of the database's value");
    b.run("SET XQUERY.STATIC_TYPING = 'OFF'").unwrap();
    b.run(&definitions("2")[0]).unwrap();
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 SR6, §4.2.6] [EXTENSION]
/// RULES.md D106, D25: a definition keeps the static typing it was made
/// with: made again from the log after a crash, and read again after a
/// clean close, as it was made, though the database's value differs.
#[test]
fn definitions_keep_their_typing_when_recovered() {
    let scratch = Scratch::new("xml-static-typing");
    let dir = scratch.path("db");
    let open = |fs: &Arc<dyn FileSystem>| {
        let db = Database::new();
        db.open_files(fs, &dir, &OpenOptions::default()).unwrap();
        db
    };
    let db = open(&os());
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, X XML(CONTENT(UNTYPED)))").unwrap();
    db.create_user("A", b"x").unwrap();
    db.set_owner("A").unwrap();
    let a = db.session("A");
    a.run("SET DATABASE XQUERY.STATIC_TYPING = 'ON'").unwrap();
    a.run("SET XQUERY.STATIC_TYPING = 'OFF'").unwrap();
    for sql in definitions("0") {
        make(&a, &sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
    a.commit().unwrap();
    drop(a);
    db.close().unwrap();
    drop(db);
    let before = open(&os());
    let a = before.session("A");
    a.run("INSERT INTO T (K, X) VALUES (1, XMLPARSE(CONTENT '<b/>' STRIP WHITESPACE))").unwrap();
    assert_eq!(fetched(&a, "SELECT F0(X) FROM T").unwrap(), 1, "read again as it was made");
    assert_eq!(fetched(&a, "SELECT * FROM S0.W").unwrap(), 1, "read again as it was made");
    a.run("SET XQUERY.STATIC_TYPING = 'OFF'").unwrap();
    for sql in definitions("1") {
        make(&a, &sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
    a.commit().unwrap();
    // A crash: the database is left, and opened again as it is.
    let db = open(&after_crash());
    let b = db.session("A");
    b.run("INSERT INTO T (K, X) VALUES (2, XMLPARSE(CONTENT '<b/>' STRIP WHITESPACE))").unwrap();
    assert_eq!(fetched(&b, "SELECT F1(X) FROM T").unwrap(), 2, "made again as it was made");
    assert_eq!(fetched(&b, "SELECT * FROM S1.W").unwrap(), 2, "made again as it was made");
    drop((a, b));
    drop((db, before));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.21 SR4, SR5c] [XQueryFS
/// §8.1.9, §8.1.10, §8.2.2.1.3, §7.2.6] a value of a type of XMLSCHEMA is a
/// document of the global elements its registered XML Schema declares;
/// their children and typed values are their types', those of the types
/// derived from them too, and a child none declares is the empty type.
#[test]
fn schema_types_are_read_10_21() {
    let db = database();
    let a = db.session("A");
    a.run("SET XQUERY.STATIC_TYPING = 'ON'").unwrap();
    let query = |text: &str| {
        format!(
            "SELECT XMLQUERY('declare namespace s = \"urn:s\"; {text}' PASSING BY REF X RETURNING SEQUENCE BY REF EMPTY ON EMPTY) FROM S"
        )
    };
    assert_eq!(fetched(&a, &query("/s:item/s:v + 1")).unwrap(), 0);
    assert_eq!(rows(&a, &query("/s:item/s:w")).unwrap().len(), 0, "of the type derived by extension");
    assert_eq!(rows(&a, &query("/s:n * 2")).unwrap().len(), 0, "of the restriction of xs:int, nillable");
    assert!(is_xquery(&failure(&a, &query("/s:item/s:zz")), "XPST0005"));
    assert!(is_xquery(&failure(&a, &query("/s:item + 1")), "XPTY0004"), "element-only content has no typed value");
    assert!(is_xquery(&failure(&a, &query("upper-case(/s:item/s:v)")), "XPTY0004"), "an xs:int is no xs:string");
}

/// A schema document of target namespace `urn:r`: an element `doc` of a
/// list, a union, mixed elements, an empty element and a wildcard of other
/// namespaces, and of a required integer attribute, an optional string
/// one and a wildcard of no namespace and `urn:x`.
const RICH: &str = r###"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:r="urn:r" targetNamespace="urn:r" elementFormDefault="qualified"><xs:simpleType name="sizes"><xs:list itemType="xs:int"/></xs:simpleType><xs:simpleType name="either"><xs:union memberTypes="xs:int xs:date"/></xs:simpleType><xs:element name="doc"><xs:complexType><xs:sequence><xs:element name="s" type="r:sizes"/><xs:element name="e" type="r:either" minOccurs="0"/><xs:element name="m" minOccurs="0" maxOccurs="unbounded"><xs:complexType mixed="true"><xs:sequence><xs:element name="b" type="xs:string"/></xs:sequence></xs:complexType></xs:element><xs:element name="z"><xs:complexType/></xs:element><xs:any namespace="##other" processContents="lax" minOccurs="0"/></xs:sequence><xs:attribute name="code" type="xs:int" use="required"/><xs:attribute name="note" type="xs:string"/><xs:anyAttribute namespace="##local urn:x" processContents="skip"/></xs:complexType></xs:element></xs:schema>"###;

/// Verifies: [ISO/IEC 9075-14:2006 draft §10.21 SR5c] [XQueryFS §8.1.4,
/// §8.1.6 to §8.1.9, §7.2.6, Appendix C.1.4] [Schema1 §3.10.4, §3.4.4]
/// [EXTENSION] a schema's attributes, of a required use one and of another
/// optional, its wildcards and xsi's attributes; lists' typed values
/// their items', unions' their members', mixed content's untyped, and
/// empty content's empty; a wildcard admits the names of the namespaces
/// its constraint allows alone, and an attribute wildcard not those of the
/// attribute uses beside it.
#[test]
fn schema_content_is_read_10_21() {
    let db = database();
    let a = db.session("A");
    a.run(&format!("CREATE XML SCHEMA RICH AS '{RICH}'")).unwrap();
    a.run("CREATE TABLE R (X XML(DOCUMENT(XMLSCHEMA ID RICH ELEMENT \"doc\")))").unwrap();
    a.run("SET XQUERY.STATIC_TYPING = 'ON'").unwrap();
    let query = |text: &str| {
        format!(
            "SELECT XMLQUERY('declare namespace r = \"urn:r\"; {text}' PASSING BY REF X RETURNING SEQUENCE BY REF EMPTY ON EMPTY) FROM R"
        )
    };
    for q in [
        "./r:doc/@code + 1",
        "sum(./r:doc/r:s)",
        "./r:doc/r:m[1] = \"x\"",
        "./r:doc/r:m/r:b/string()",
        "./r:doc/@xsi:type",
        "./r:doc/*:other",
        "declare namespace q = \"urn:q\"; ./r:doc/q:w",
        "count(./r:doc/@*)",
        "./r:doc/@plain",
        "declare namespace x = \"urn:x\"; ./r:doc/@x:a",
        "./r:doc/r:e instance of xs:int?",
    ] {
        assert_eq!(fetched(&a, &query(q)).unwrap(), 0, "{q}");
    }
    for (q, code) in [
        ("./r:doc/@note + 1", "XPTY0004"),
        ("./r:doc/r:e + 1", "XPTY0004"),
        ("data(./r:doc/r:z)", "XPST0005"),
        ("./r:doc/r:other", "XPST0005"),
        ("./r:doc/plain", "XPST0005"),
        ("declare namespace q = \"urn:q\"; ./r:doc/@q:a", "XPST0005"),
    ] {
        assert!(is_xquery(&failure(&a, &query(q)), code), "{q}");
    }
}

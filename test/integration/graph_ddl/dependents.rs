//! What a property graph depends on (§4.1 R4.7): DROP of a table, view,
//! column, schema or routine it reads is refused under RESTRICT and drops
//! it under CASCADE, as of a view; a column it names is not renamed; and
//! another change of a table that breaks it marks it INVALID until ALTER
//! PROPERTY GRAPH ... COMPILE finds it holds again.

use super::{BANK, bank, refused_with, rows};

/// Whether each graph is valid, by name.
const VALID: &str = "SELECT PROPERTY_GRAPH_NAME, IS_VALID FROM INFORMATION_SCHEMA.PROPERTY_GRAPHS ORDER BY 1";

/// Verifies: [EXTENSION] §4.1 R4.7; [ISO/IEC 9075:1992 §11.18 SR4, §11.20
/// SR3, §11.2 SR3]: DROP TABLE and DROP VIEW of what a graph reads, a
/// table of a property's subquery too, and DROP SCHEMA of a graph's, are
/// refused under RESTRICT and drop the graph under CASCADE.
#[test]
fn drops_of_what_a_graph_reads_4_1() {
    let db = bank();
    let a = db.session("A");
    a.run(BANK).unwrap();
    a.run("CREATE VIEW RICH AS SELECT ACCT_ID, OWNER FROM ACCOUNT").unwrap();
    a.run(
        "CREATE PROPERTY GRAPH SMALL VERTEX TABLES (RICH KEY (ACCT_ID), \
         CITY PROPERTIES ((SELECT COUNT(*) FROM NOTES) AS NOTED)) OPTIONS (TRUSTED MODE)",
    )
    .unwrap();
    a.commit().unwrap();
    refused_with(&a, "DROP TABLE TRANSFER RESTRICT", "property graph A.BANK");
    refused_with(&a, "DROP VIEW RICH RESTRICT", "property graph A.SMALL");
    refused_with(&a, "DROP TABLE NOTES RESTRICT", "property graph A.SMALL");
    a.run("DROP TABLE NOTES CASCADE").unwrap();
    assert_eq!(rows(&a, VALID), ["BANK,YES"]);
    a.rollback().unwrap();
    a.run("DROP TABLE ACCOUNT CASCADE").unwrap();
    assert_eq!(rows(&a, VALID), Vec::<String>::new());
    a.rollback().unwrap();
    a.run("DROP VIEW RICH CASCADE").unwrap();
    assert_eq!(rows(&a, VALID), ["BANK,YES"]);
    a.rollback().unwrap();
    let e = db.session("A").run("DROP SCHEMA A RESTRICT").unwrap_err();
    assert_eq!(e.sqlcode(), -103, "{e:?}");
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE T (K INTEGER PRIMARY KEY) CREATE PROPERTY GRAPH G VERTEX TABLES (T)").unwrap();
    let b = db.session("B");
    b.run("DROP TABLE T CASCADE").unwrap();
    b.rollback().unwrap();
    b.run("DROP SCHEMA B CASCADE").unwrap();
    b.commit().unwrap();
    assert_eq!(rows(&b, "SELECT * FROM INFORMATION_SCHEMA.PROPERTY_GRAPHS"), Vec::<String>::new());
}

/// Verifies: [EXTENSION] §4.1 R4.7; [ISO/IEC 9075:1992 §11.15 SR3]: a
/// column a graph names as a key, an endpoint or a property is dropped
/// with CASCADE alone, which drops the graph; one it names not is dropped
/// with it kept; one it names is not renamed.
#[test]
fn columns_a_graph_names_4_1() {
    let db = bank();
    let a = db.session("A");
    a.run(BANK).unwrap();
    a.run("ALTER TABLE NOTES ADD COLUMN SPARE INTEGER").unwrap();
    a.commit().unwrap();
    refused_with(&a, "ALTER TABLE ACCOUNT DROP COLUMN OWNER RESTRICT", "property graph A.BANK");
    refused_with(&a, "ALTER TABLE TRANSFER DROP COLUMN AMOUNT RESTRICT", "CASCADE");
    refused_with(&a, "ALTER TABLE ACCOUNT RENAME COLUMN OWNER TO HOLDER", "not renamed");
    a.run("ALTER TABLE ACCOUNT ADD COLUMN OPENED DATE").unwrap();
    a.run("ALTER TABLE ACCOUNT DROP COLUMN OPENED RESTRICT").unwrap();
    a.run("ALTER TABLE NOTES RENAME COLUMN SPARE TO EXTRA").unwrap();
    refused_with(&a, "ALTER TABLE CITY RENAME COLUMN CITY_NAME TO TOWN", "not renamed");
    a.run("ALTER TABLE TRANSFER DROP COLUMN AMOUNT CASCADE").unwrap();
    assert_eq!(rows(&a, VALID), Vec::<String>::new());
}

/// Verifies: [EXTENSION] §4.1 R4.7, R4.3, §22 GRF-0005: a key's PRIMARY
/// KEY or an endpoint's FOREIGN KEY of an ENFORCED graph dropped
/// marks the graph INVALID, its definition kept; COMPILE refuses while it is
/// broken, and makes it valid once it holds again.
#[test]
fn a_broken_graph_is_invalid_until_compiled_4_1() {
    let db = bank();
    let a = db.session("A");
    a.run("CREATE TABLE PLAIN (K INTEGER NOT NULL, CONSTRAINT PK PRIMARY KEY (K))").unwrap();
    a.run("CREATE PROPERTY GRAPH G VERTEX TABLES (PLAIN)").unwrap();
    a.run(BANK).unwrap();
    a.commit().unwrap();
    a.run("ALTER TABLE PLAIN DROP CONSTRAINT PK CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(rows(&a, VALID), ["BANK,YES", "G,NO"]);
    refused_with(&a, "ALTER PROPERTY GRAPH G COMPILE", "ENFORCED MODE");
    a.run("ALTER TABLE PLAIN ADD CONSTRAINT PK PRIMARY KEY (K)").unwrap();
    assert_eq!(rows(&a, VALID), ["BANK,YES", "G,NO"]);
    a.run("ALTER PROPERTY GRAPH G COMPILE").unwrap();
    assert_eq!(rows(&a, VALID), ["BANK,YES", "G,YES"]);
    a.commit().unwrap();
    let fk = "SELECT CONSTRAINT_NAME FROM INFORMATION_SCHEMA.TABLE_CONSTRAINTS \
              WHERE TABLE_NAME = 'LOCATED_IN' AND CONSTRAINT_TYPE = 'FOREIGN KEY' ORDER BY 1";
    for name in rows(&a, fk) {
        a.run(&format!("ALTER TABLE LOCATED_IN DROP CONSTRAINT {name} CASCADE")).unwrap();
    }
    assert_eq!(rows(&a, VALID), ["BANK,NO", "G,YES"]);
    let definition =
        rows(&a, "SELECT PROPERTY_GRAPH_DEFINITION FROM INFORMATION_SCHEMA.PROPERTY_GRAPHS WHERE PROPERTY_GRAPH_NAME = 'BANK'");
    assert!(definition[0].contains("A.LOCATED_IN KEY (ACCT_ID)"), "{definition:?}");
    refused_with(&a, "ALTER PROPERTY GRAPH BANK COMPILE", "FOREIGN KEY");
}

/// Verifies: [EXTENSION] §4.1 R4.7; [ISO/IEC 9075-2:2023 §11.62 SR4c, GR1,
/// §4.35.2]: DROP FUNCTION of a routine a property invokes is refused
/// under RESTRICT and drops the graph under CASCADE; a routine defined
/// after the graph is none of its subject routines.
#[test]
fn routines_a_property_invokes_4_1() {
    let db = bank();
    let a = db.session("A");
    a.run("CREATE FUNCTION TRIPLED (X INTEGER) RETURNS INTEGER CONTAINS SQL RETURN X * 3").unwrap();
    a.run("CREATE PROPERTY GRAPH G VERTEX TABLES (ACCOUNT PROPERTIES (TRIPLED(ACCT_ID) AS T))").unwrap();
    a.commit().unwrap();
    let e = a.run("DROP FUNCTION TRIPLED RESTRICT").unwrap_err();
    assert_eq!(e.sqlcode(), -103, "{e:?}");
    assert!(e.msg.contains("property graph A.G"), "{e:?}");
    a.rollback().unwrap();
    a.run("DROP FUNCTION TRIPLED CASCADE").unwrap();
    assert_eq!(rows(&a, VALID), Vec::<String>::new());
}

//! Assertions [ISO/IEC 9075:1992 §11.34, §11.35]: constraints of a schema,
//! on no table, checked as each statement that changes a table they read
//! ends, or at commit if deferred; their rules, their drops, and what the
//! Information Schema shows of them.

use turborel::db::Database;
use turborel::error::Code;

use crate::common::{count, exec, rows};

/// Schema A: tables T and U, of a column K each; assertion FEW: T has
/// fewer than three rows.
fn asserted() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (K INTEGER)
           CREATE TABLE U (K INTEGER)
           CREATE ASSERTION FEW CHECK ((SELECT COUNT(*) FROM T) < 3)",
    )
    .unwrap();
    db
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.34 GR2; §10.6 GR3] an
/// assertion is checked as each statement that changes a table it reads
/// ends: not satisfied if and only if its condition is false, the
/// statement then undone; a statement on a table it does not read is not
/// held to it.
#[test]
fn an_assertion_is_checked_11_34() {
    let db = asserted();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1), (2)").unwrap();
    assert_eq!(exec(&a, "INSERT INTO T VALUES (3)").err(), Some(Code::Constraint), "FEW");
    assert_eq!(count(&a, "SELECT K FROM T"), 2, "the statement undone");
    a.run("INSERT INTO U VALUES (1), (2), (3)").unwrap();
    a.run("UPDATE T SET K = K + 1").unwrap();
    a.run("DELETE FROM T WHERE K = 2").unwrap();
    a.run("INSERT INTO T VALUES (9)").unwrap();
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.34 GR2; §10.6 GR3] an
/// assertion false as it is defined is refused, in a schema statement and
/// in a `<schema definition>`, which then has no effect; a condition
/// unknown satisfies it.
#[test]
fn an_assertion_false_is_not_defined_11_34() {
    let db = asserted();
    let a = db.session("A");
    assert_eq!(exec(&a, "CREATE ASSERTION OCCUPIED CHECK (EXISTS (SELECT * FROM T))").err(), Some(Code::Constraint), "T is empty");
    a.run("INSERT INTO T VALUES (1)").unwrap();
    a.run("CREATE ASSERTION OCCUPIED CHECK (EXISTS (SELECT * FROM T))").unwrap();
    assert_eq!(exec(&a, "DELETE FROM T").err(), Some(Code::Constraint), "OCCUPIED");
    a.run("CREATE ASSERTION MAYBE CHECK ((SELECT MAX(K) FROM U) > 0)").unwrap();
    a.commit().unwrap();
    let made =
        db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE V (K INTEGER) CREATE ASSERTION OCCUPIED CHECK (EXISTS (SELECT * FROM V))");
    assert_eq!(made.err().map(|e| e.code), Some(Code::Constraint));
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE V (K INTEGER) CREATE ASSERTION VACANT CHECK (NOT EXISTS (SELECT * FROM V))")
        .expect("no schema B was made");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §10.6 GR1, GR2; §14.2;
/// §14.3 GR4, GR5a] an assertion INITIALLY DEFERRED is checked at commit,
/// which rolls back if it is false; SET CONSTRAINTS makes it immediate,
/// and checks it then; one not DEFERRABLE is not set.
#[test]
fn a_deferred_assertion_14_2() {
    let db = asserted();
    let a = db.session("A");
    a.run("CREATE ASSERTION MATCHED CHECK ((SELECT COUNT(*) FROM T) = (SELECT COUNT(*) FROM U)) DEFERRABLE INITIALLY DEFERRED").unwrap();
    a.commit().unwrap();
    a.run("INSERT INTO T VALUES (1)").unwrap();
    a.run("INSERT INTO U VALUES (1)").unwrap();
    a.commit().unwrap();
    a.run("INSERT INTO T VALUES (2)").unwrap();
    assert_eq!(a.commit().err().map(|e| e.code), Some(Code::CommitFailed), "MATCHED");
    assert_eq!(count(&a, "SELECT K FROM T"), 1, "rolled back");
    a.run("INSERT INTO T VALUES (2)").unwrap();
    assert_eq!(exec(&a, "SET CONSTRAINTS MATCHED IMMEDIATE").err(), Some(Code::Constraint));
    a.run("INSERT INTO U VALUES (2)").unwrap();
    a.run("SET CONSTRAINTS MATCHED IMMEDIATE").unwrap();
    assert_eq!(exec(&a, "INSERT INTO T VALUES (3)").err(), Some(Code::Constraint), "immediate now");
    assert_eq!(exec(&a, "SET CONSTRAINTS FEW DEFERRED").err(), Some(Code::SyntaxRule), "§14.2 SR2");
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.35 SR2, AR1, GR1; §11.18
/// SR4; §11.15 SR4; §11.37 GR5; §11.2 SR3, GR4] DROP ASSERTION drops
/// it; a table or column it reads is dropped with
/// RESTRICT only once it is gone, and takes it with CASCADE, as its schema
/// does.
#[test]
fn drop_assertion_11_35() {
    let db = asserted();
    let a = db.session("A");
    assert_eq!(exec(&a, "DROP ASSERTION NOPE").err(), Some(Code::SyntaxRule), "§11.35 SR2");
    assert_eq!(exec(&a, "DROP TABLE T RESTRICT").err(), Some(Code::SyntaxRule), "FEW reads it");
    a.run("DROP ASSERTION FEW").unwrap();
    a.run("INSERT INTO T VALUES (1), (2), (3)").unwrap();
    a.run("CREATE ASSERTION SMALL CHECK (NOT EXISTS (SELECT * FROM U WHERE K > 9))").unwrap();
    a.run("DROP TABLE U CASCADE").unwrap();
    assert_eq!(exec(&a, "DROP ASSERTION SMALL").err(), Some(Code::SyntaxRule), "it went with U");
    a.run("ALTER TABLE T ADD COLUMN N INTEGER").unwrap();
    a.run("CREATE ASSERTION SIGNED CHECK (NOT EXISTS (SELECT * FROM T WHERE N < 0))").unwrap();
    a.run("CREATE ASSERTION KEYED CHECK (NOT EXISTS (SELECT * FROM T WHERE K < 0))").unwrap();
    assert_eq!(exec(&a, "ALTER TABLE T DROP COLUMN N RESTRICT").err(), Some(Code::SyntaxRule), "SIGNED refers to it");
    a.run("ALTER TABLE T DROP COLUMN N CASCADE").unwrap();
    assert_eq!(exec(&a, "INSERT INTO T VALUES (-1)").err(), Some(Code::Constraint), "KEYED kept");
    a.run("ALTER TABLE T ADD COLUMN N INTEGER").unwrap();
    a.run("INSERT INTO T VALUES (1, -1)").unwrap();
    a.commit().unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION B").unwrap();
    assert_eq!(exec(&db.session("B"), "DROP ASSERTION A.KEYED").err(), Some(Code::Privilege), "§11.35 AR1");
    assert_eq!(exec(&a, "DROP SCHEMA A RESTRICT").err(), Some(Code::SyntaxRule), "§11.2 SR3");
    a.run("DROP TABLE T CASCADE").unwrap();
    a.run("CREATE ASSERTION ALONE CHECK (1 = 1)").unwrap();
    a.commit().unwrap();
    a.run("DROP SCHEMA A CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(count(&a, "SELECT CONSTRAINT_NAME FROM INFORMATION_SCHEMA.ASSERTIONS"), 0, "its assertions went with it");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.34 SR1, SR2, SR4, SR5,
/// SR6, SR7, AR1, AR2] an assertion's name is no other constraint's of its
/// schema, and its schema's in a `<schema definition>`; its condition
/// reads no temporary table, names no parameter and no CURRENT_USER; it is
/// defined by its schema's owner, who holds REFERENCES on what it reads.
#[test]
fn the_rules_of_an_assertion_11_34() {
    let db = asserted();
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE V (K INTEGER)").unwrap();
    let a = db.session("A");
    a.run("ALTER TABLE U ADD CONSTRAINT TAKEN CHECK (K > 0)").unwrap();
    assert_eq!(exec(&a, "CREATE ASSERTION TAKEN CHECK (1 = 1)").err(), Some(Code::SyntaxRule), "SR2, SR7");
    assert_eq!(exec(&a, "CREATE ASSERTION FEW CHECK (1 = 1)").err(), Some(Code::SyntaxRule), "SR2, SR7: FEW");
    a.run("CREATE GLOBAL TEMPORARY TABLE G (K INTEGER)").unwrap();
    assert_eq!(exec(&a, "CREATE ASSERTION SCRATCH CHECK (NOT EXISTS (SELECT * FROM G))").err(), Some(Code::SyntaxRule), "SR5");
    assert_eq!(exec(&a, "CREATE ASSERTION WHO CHECK (CURRENT_USER = 'A')").err(), Some(Code::SyntaxRule), "SR6");
    assert_eq!(exec(&a, "CREATE ASSERTION OTHERS CHECK (NOT EXISTS (SELECT * FROM B.V))").err(), Some(Code::Privilege), "AR2");
    let b = db.session("B");
    b.run("GRANT REFERENCES (K) ON V TO A").unwrap();
    b.commit().unwrap();
    a.run("CREATE ASSERTION OTHERS CHECK (NOT EXISTS (SELECT * FROM B.V WHERE K < 0))").unwrap();
    a.commit().unwrap();
    assert_eq!(exec(&b, "INSERT INTO V VALUES (-1)").err(), Some(Code::Constraint), "A's assertion holds of B's table");
    assert_eq!(exec(&b, "CREATE ASSERTION A.MINE CHECK (1 = 1)").err(), Some(Code::Privilege), "AR1: in A's schema");
    assert_eq!(
        db.schema("CREATE SCHEMA AUTHORIZATION C CREATE ASSERTION A.ELSEWHERE CHECK (1 = 1)").err().map(|e| e.code),
        Some(Code::SyntaxRule),
        "SR1"
    );
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §21.2.15, §21.2.17, §21.2.23]
/// the Information Schema shows each assertion, whether it is deferrable,
/// its condition, and the tables whose columns it references.
#[test]
fn assertions_in_the_information_schema_21_2() {
    let db = asserted();
    let a = db.session("A");
    a.run("CREATE ASSERTION LATER CHECK (NOT EXISTS (SELECT * FROM U WHERE K < 0)) DEFERRABLE").unwrap();
    let squeeze = |sql: &str| rows(&a, sql).unwrap().iter().map(|r| r.split_whitespace().collect::<Vec<_>>().join(" ")).collect::<Vec<_>>();
    let listed = "SELECT CONSTRAINT_NAME, IS_DEFERRABLE, INITIALLY_DEFERRED FROM INFORMATION_SCHEMA.ASSERTIONS ORDER BY CONSTRAINT_NAME";
    assert_eq!(squeeze(listed), ["FEW,NO,NO", "LATER,YES,NO"]);
    let clause = "SELECT CHECK_CLAUSE FROM INFORMATION_SCHEMA.CHECK_CONSTRAINTS WHERE CONSTRAINT_NAME = 'FEW'";
    assert_eq!(squeeze(clause), ["(SELECT COUNT(*) FROM T) < 3"]);
    let read = "SELECT TABLE_NAME, CONSTRAINT_NAME FROM INFORMATION_SCHEMA.CONSTRAINT_TABLE_USAGE ORDER BY CONSTRAINT_NAME";
    assert_eq!(squeeze(read), ["U,LATER"], "of the columns it references: FEW references none");
}

/// Verifies: [DESIGN] RULES.md D25: assertions and their drops are logged
/// and kept by a checkpoint, as is what a view is drawn from, which one
/// reading it needs: as they were after a crash and after a clean close.
#[test]
fn assertions_are_kept() {
    let open = |dir: &std::path::Path| {
        let db = Database::new();
        db.open_files(&crate::common::os(), dir, &turborel::db::OpenOptions::default()).unwrap();
        db
    };
    let scratch = crate::common::Scratch::new("assertions");
    let dir = scratch.path("db");
    let db = open(&dir);
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER) CREATE ASSERTION FEW CHECK ((SELECT COUNT(*) FROM T) < 2)")
        .unwrap();
    let a = db.session("A");
    a.run("CREATE ASSERTION SIGNED CHECK (NOT EXISTS (SELECT * FROM T WHERE K < 0))").unwrap();
    a.run("CREATE ASSERTION GONE CHECK (NOT EXISTS (SELECT * FROM T WHERE K = 7))").unwrap();
    a.run("CREATE VIEW V AS SELECT K FROM T").unwrap();
    a.run("CREATE ASSERTION SEEN CHECK (NOT EXISTS (SELECT * FROM V WHERE K = 8))").unwrap();
    a.run("DROP ASSERTION GONE").unwrap();
    a.commit().unwrap();
    drop((a, db));
    let kept = |db: &Database| {
        let a = db.session("A");
        assert_eq!(exec(&a, "INSERT INTO T VALUES (-1)").err(), Some(Code::Constraint), "SIGNED");
        assert_eq!(exec(&a, "INSERT INTO T VALUES (8)").err(), Some(Code::Constraint), "SEEN, through V");
        a.run("INSERT INTO T VALUES (7)").unwrap();
        assert_eq!(exec(&a, "INSERT INTO T VALUES (1)").err(), Some(Code::Constraint), "FEW");
        a.rollback().unwrap();
    };
    let db = open(&dir);
    kept(&db);
    db.close().unwrap();
    drop(db);
    kept(&open(&dir));
}

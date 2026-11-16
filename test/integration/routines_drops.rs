//! What a dropped SQL-invoked routine takes with it, and the routines a
//! drop of what they read or are typed by takes [ISO/IEC 9075-2:2023
//! §11.62, §11.31, §11.33, §11.23, §11.59, §11.2], and routines kept
//! across reopening [§11.60 GR3].

use turborel::db::{Database, Session};
use turborel::error::Code;

use crate::common::Scratch;

/// Schema A: T (K, P) of rows (1, 10) and (2, 20); function F (X) = X + 1,
/// deterministic, of CONTAINS SQL.
fn defined() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, P INTEGER)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 10), (2, 20)").unwrap();
    a.run("CREATE FUNCTION F (X INTEGER) RETURNS INTEGER DETERMINISTIC CONTAINS SQL RETURN X + 1").unwrap();
    a.commit().unwrap();
    db
}

/// The rows of `query` in `s`, each its values as text joined by commas.
fn rows(s: &Session, query: &str) -> Vec<String> {
    let mut c = s.open_cursor(query).unwrap();
    let mut out = Vec::new();
    while let Some(row) = c.fetch().unwrap() {
        out.push(row.iter().map(|v| v.to_string().trim_end().to_owned()).collect::<Vec<_>>().join(","));
    }
    out
}

/// The SQLCODE class of `sql` run in `s`, which fails.
fn code(s: &Session, sql: &str) -> Code {
    s.run(sql).unwrap_err().code
}

/// The names of the routines of schema A, in order.
fn routines(s: &Session) -> Vec<String> {
    rows(s, "SELECT ROUTINE_NAME FROM INFORMATION_SCHEMA.ROUTINES WHERE ROUTINE_SCHEMA = 'A' ORDER BY ROUTINE_NAME")
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.62 SR1 to SR3, GR6] DROP ROUTINE,
/// FUNCTION and PROCEDURE name a routine by its name, with its parameters'
/// types or without, or by its specific name, of the `<routine type>`
/// named; a name that names none is refused.
#[test]
fn drop_names_a_routine_11_62() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE PROCEDURE P (IN X INTEGER) SPECIFIC PS MODIFIES SQL DATA INSERT INTO T VALUES (X, X)").unwrap();
    a.run("CREATE FUNCTION G (X SMALLINT) RETURNS INTEGER CONTAINS SQL RETURN X").unwrap();
    a.commit().unwrap();
    assert_eq!(code(&a, "DROP FUNCTION P RESTRICT"), Code::SyntaxRule, "SR3: not a function");
    assert_eq!(code(&a, "DROP FUNCTION G (INTEGER) RESTRICT"), Code::SyntaxRule, "SR2: not of those types");
    assert_eq!(code(&a, "DROP ROUTINE NOWHERE RESTRICT"), Code::SyntaxRule);
    a.run("DROP SPECIFIC PROCEDURE PS RESTRICT").unwrap();
    a.run("DROP FUNCTION G (SMALLINT) RESTRICT").unwrap();
    a.run("DROP ROUTINE F CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(routines(&a), Vec::<String>::new());
    assert_eq!(code(&db.session("B"), "DROP ROUTINE A.NOWHERE RESTRICT"), Code::SyntaxRule);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.62 AR1] only the owner of a
/// routine's schema drops it.
#[test]
fn only_the_owner_drops_11_62() {
    let db = defined();
    let b = db.session("B");
    assert_eq!(code(&b, "DROP FUNCTION A.F RESTRICT"), Code::Privilege);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.62 SR4c, GR1] a routine a view, a
/// CHECK, a generated column, a trigger or another routine invokes is
/// dropped only with CASCADE, which drops each of them, and what invokes
/// those, at any remove.
#[test]
fn what_invokes_a_routine_goes_with_it_11_62() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE VIEW V (K, F) AS SELECT K, F(K) FROM T").unwrap();
    a.run("CREATE TABLE C (K INTEGER CHECK (F(K) > 0), G INTEGER GENERATED ALWAYS AS (F(K)))").unwrap();
    a.run("CREATE TABLE L (K INTEGER)").unwrap();
    a.run("CREATE TRIGGER W AFTER INSERT ON T REFERENCING NEW AS NR FOR EACH ROW INSERT INTO L VALUES (F(NR.K))").unwrap();
    a.run("CREATE FUNCTION H (X INTEGER) RETURNS INTEGER DETERMINISTIC CONTAINS SQL RETURN F(X) * 2").unwrap();
    a.run("CREATE FUNCTION J (X INTEGER) RETURNS INTEGER DETERMINISTIC CONTAINS SQL RETURN H(X) + 1").unwrap();
    a.commit().unwrap();
    assert_eq!(rows(&a, "SELECT J(1) FROM T WHERE K = 1"), ["5"]);
    assert_eq!(code(&a, "DROP FUNCTION F RESTRICT"), Code::SyntaxRule);
    assert_eq!(code(&a, "DROP FUNCTION H RESTRICT"), Code::SyntaxRule, "J invokes it");
    a.run("DROP FUNCTION F CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(routines(&a), Vec::<String>::new(), "H and J, which invoke F, go too");
    assert_eq!(a.open_cursor("SELECT * FROM V").err().map(|e| e.code), Some(Code::SyntaxRule), "the view is gone");
    assert_eq!(rows(&a, "SELECT COLUMN_NAME FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'C'"), ["K"]);
    a.run("INSERT INTO C VALUES (-5)").unwrap();
    a.run("INSERT INTO T VALUES (3, 30)").unwrap();
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM L"), ["0"], "the trigger is gone");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.31 SR7e, GR4b iv, §11.33 SR5d,
/// GR3b iv] a table or view a routine's body reads is dropped only with
/// CASCADE, which drops the routine.
#[test]
fn a_table_a_body_reads_takes_it_11_31() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE VIEW U AS SELECT K FROM T").unwrap();
    a.run("CREATE FUNCTION TOTAL () RETURNS INTEGER READS SQL DATA RETURN (SELECT SUM(P) FROM T)").unwrap();
    a.run("CREATE PROCEDURE COUNTED (OUT N INTEGER) READS SQL DATA SELECT COUNT(*) INTO N FROM U").unwrap();
    a.commit().unwrap();
    assert_eq!(code(&a, "DROP VIEW U RESTRICT"), Code::SyntaxRule);
    a.run("DROP VIEW U CASCADE").unwrap();
    assert_eq!(routines(&a), ["F", "TOTAL"]);
    assert_eq!(code(&a, "DROP TABLE T RESTRICT"), Code::SyntaxRule);
    a.run("DROP TABLE T CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(routines(&a), ["F"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.23 SR7c, GR2] a column a body
/// references is dropped only with CASCADE, which drops the routine; a
/// column it does not leaves it seeing one fewer, and it runs on.
#[test]
fn a_column_a_body_references_takes_it_11_23() {
    let db = defined();
    let a = db.session("A");
    a.run("ALTER TABLE T ADD COLUMN Q INTEGER").unwrap();
    a.run("CREATE FUNCTION TOP_P () RETURNS INTEGER READS SQL DATA RETURN (SELECT MAX(P) FROM T)").unwrap();
    a.commit().unwrap();
    a.run("ALTER TABLE T DROP COLUMN Q RESTRICT").unwrap();
    assert_eq!(rows(&a, "SELECT TOP_P() FROM T WHERE K = 1"), ["20"]);
    assert_eq!(code(&a, "ALTER TABLE T DROP COLUMN P RESTRICT"), Code::SyntaxRule);
    a.run("ALTER TABLE T DROP COLUMN P CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(routines(&a), ["F"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.59 SR4g, GR1] a distinct type a
/// routine's parameter or result is of is dropped only with CASCADE,
/// which drops the routine.
#[test]
fn a_type_a_routine_is_typed_by_takes_it_11_59() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE TYPE MONEY AS DECIMAL(9, 2) FINAL").unwrap();
    a.run("CREATE FUNCTION CENTS (M MONEY) RETURNS INTEGER CONTAINS SQL RETURN CAST(CAST(M AS DECIMAL(9, 2)) * 100 AS INTEGER)").unwrap();
    a.commit().unwrap();
    assert_eq!(rows(&a, "SELECT CENTS(CAST(1.25 AS MONEY)) FROM T WHERE K = 1"), ["125"]);
    assert_eq!(code(&a, "DROP TYPE MONEY RESTRICT"), Code::SyntaxRule);
    a.run("DROP TYPE MONEY CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(routines(&a), ["F"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.2 SR4l, GR10] a schema holding a
/// routine is dropped only with CASCADE, which drops it, and what of
/// other schemas invokes it.
#[test]
fn a_schema_takes_its_routines_11_2() {
    let db = defined();
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE M (K INTEGER)").unwrap();
    let a = db.session("A");
    a.run("GRANT EXECUTE ON FUNCTION F TO B").unwrap();
    a.commit().unwrap();
    let b = db.session("B");
    b.run("CREATE VIEW W (K) AS SELECT A.F(K) FROM M").unwrap();
    b.commit().unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION E CREATE FUNCTION E.ONE () RETURNS INTEGER CONTAINS SQL RETURN 1").unwrap();
    let e = db.session("E");
    assert_eq!(code(&e, "DROP SCHEMA E RESTRICT"), Code::SyntaxRule);
    e.run("DROP SCHEMA E CASCADE").unwrap();
    e.commit().unwrap();
    a.run("DROP TABLE T CASCADE").unwrap();
    a.run("DROP SCHEMA A CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(b.open_cursor("SELECT * FROM W").err().map(|e| e.code), Some(Code::SyntaxRule), "the view of B that invoked A.F is gone");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.60 GR3, §4.35.2] a routine is kept
/// with the definitions: after stopping without a checkpoint, by its
/// logged definition, and after a checkpoint, by the snapshot, with its
/// grants; and its drop too.
#[test]
fn routines_are_kept_across_reopening() {
    let scratch = Scratch::new("routines-reopen");
    let dir = scratch.path("db");
    {
        let db = Database::open(&dir).unwrap();
        db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER)").unwrap();
        let a = db.session("A");
        a.run("INSERT INTO T VALUES (1)").unwrap();
        a.run("CREATE FUNCTION LOGGED (X INTEGER) RETURNS INTEGER CONTAINS SQL RETURN X + 10").unwrap();
        a.run("CREATE FUNCTION GONE () RETURNS INTEGER CONTAINS SQL RETURN 0").unwrap();
        a.run("GRANT EXECUTE ON FUNCTION LOGGED TO B").unwrap();
        a.commit().unwrap();
        a.run("DROP FUNCTION GONE RESTRICT").unwrap();
        a.commit().unwrap();
    }
    {
        let db = Database::open(&dir).unwrap();
        let a = db.session("A");
        assert_eq!(rows(&a, "SELECT LOGGED(K) FROM T"), ["11"]);
        a.run("CREATE PROCEDURE KEPT (IN X INTEGER) MODIFIES SQL DATA INSERT INTO T VALUES (X)").unwrap();
        a.commit().unwrap();
        db.checkpoint().unwrap();
        drop(a);
        db.close().unwrap();
    }
    let db = Database::open(&dir).unwrap();
    let a = db.session("A");
    a.run("CALL KEPT(2)").unwrap();
    assert_eq!(rows(&a, "SELECT LOGGED(K) FROM T ORDER BY K"), ["11", "12"]);
    assert_eq!(routines(&a), ["KEPT", "LOGGED"]);
    assert_eq!(rows(&db.session("B"), "SELECT A.LOGGED(1) FROM (VALUES (0)) AS X (Z)"), ["11"]);
    a.commit().unwrap();
    drop(a);
    db.close().unwrap();
}

//! SQL-server modules [ISO/IEC 9075-4:2006 draft §4.1, §9.21, §9.22,
//! §12.8]: CREATE MODULE with its routines, declared local temporary
//! tables and SQL-path, DROP MODULE with RESTRICT and CASCADE, what other
//! drops take of them, and modules kept across reopening.

use turborel::db::{Database, Session};
use turborel::error::Code;

use crate::common::Scratch;

/// Schemas A and LIB, both of A: A.T (K) of row 1; LIB.TWICE (X) = X * 2
/// and A.TWICE (X) = X * 3.
fn defined() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER)").unwrap();
    db.schema("CREATE SCHEMA LIB AUTHORIZATION A CREATE FUNCTION TWICE (X INTEGER) RETURNS INTEGER CONTAINS SQL RETURN X * 2").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1)").unwrap();
    a.run("CREATE FUNCTION TWICE (X INTEGER) RETURNS INTEGER CONTAINS SQL RETURN X * 3").unwrap();
    a.commit().unwrap();
    db
}

/// The values of `query` in `s`, as text.
fn rows(s: &Session, query: &str) -> Vec<String> {
    let mut c = s.open_cursor(query).unwrap();
    let mut out = Vec::new();
    while let Some(row) = c.fetch().unwrap() {
        out.push(row.iter().map(|v| v.to_string().trim().to_owned()).collect::<Vec<_>>().join(","));
    }
    out
}

/// The module SHOP of A: PLACE (X) inserts X into T, and COUNTED () counts
/// T's rows.
const SHOP: &str = "CREATE MODULE SHOP \
     PROCEDURE PLACE (IN X INTEGER) MODIFIES SQL DATA INSERT INTO T VALUES (X); \
     FUNCTION COUNTED () RETURNS INTEGER READS SQL DATA RETURN (SELECT COUNT(*) FROM T); \
     END MODULE";

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §9.21 GR1, GR3, §9.24 GR1; ISO/IEC 9075-11:2006 draft §6.40]
/// A module's routines are invoked by their names in its schema, a
/// procedure by CALL, a function in a query, and ROUTINES shows them of
/// the module, no schema-level routines; MODULES shows the module, its
/// default schema its own, its definition as written.
#[test]
fn a_module_defines_its_routines_9_21() {
    let db = defined();
    let a = db.session("A");
    a.run(SHOP).unwrap();
    a.run("CALL PLACE(2)").unwrap();
    a.run("CALL A.PLACE(3)").unwrap();
    assert_eq!(rows(&a, "SELECT COUNTED() FROM T WHERE K = 1"), ["3"]);
    let routines = "SELECT ROUTINE_NAME, ROUTINE_TYPE, MODULE_SCHEMA, MODULE_NAME, SCHEMA_LEVEL_ROUTINE \
                    FROM INFORMATION_SCHEMA.ROUTINES WHERE ROUTINE_SCHEMA = 'A' ORDER BY ROUTINE_NAME";
    assert_eq!(rows(&a, routines), ["COUNTED,FUNCTION,A,SHOP,NO", "PLACE,PROCEDURE,A,SHOP,NO", "TWICE,FUNCTION,NULL,NULL,YES"]);
    let modules = "SELECT MODULE_SCHEMA, MODULE_NAME, DEFAULT_CHARACTER_SET_NAME, DEFAULT_SCHEMA_NAME, SQL_PATH, \
                   MODULE_DEFINITION = '{SHOP}' FROM INFORMATION_SCHEMA.MODULES";
    assert_eq!(rows(&a, &modules.replace("{SHOP}", SHOP)), ["A,SHOP,SQL_TEXT,A,NULL,TRUE"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §9.21 SR2, SR5, SR8, §9.24 SR3; ISO/IEC 9075-2:2023 §10.3 SR1, §11.62 SR2]
/// A second module of a name is refused, as is a routine of it named in
/// another schema, a path naming a schema twice or one of another
/// catalog, and a character set but SQL_TEXT; DROP ROUTINE of a module's
/// routine is refused, as it is no schema-level routine; DROP MODULE names
/// a module there is.
#[test]
fn rules_of_a_module_definition_9_21() {
    let db = defined();
    let a = db.session("A");
    a.run(SHOP).unwrap();
    a.commit().unwrap();
    let refused = |sql: &str, cited: &str| {
        let e = a.run(sql).unwrap_err();
        assert_eq!((e.code, e.sqlcode()), (Code::SyntaxRule, -103), "{sql}: {e:?}");
        assert!(e.rule.as_deref().unwrap_or_default().contains(cited), "{sql}: {e:?}");
        a.rollback().unwrap();
    };
    refused(SHOP, "§9.21 SR2");
    refused("CREATE MODULE M PROCEDURE LIB.P () CONTAINS SQL SIGNAL SQLSTATE '75000'; END MODULE", "§9.24 SR3");
    refused("CREATE MODULE M PATH A, LIB, A PROCEDURE P () CONTAINS SQL SIGNAL SQLSTATE '75000'; END MODULE", "§10.3 SR1");
    refused("CREATE MODULE M PATH ELSEWHERE.A PROCEDURE P () CONTAINS SQL SIGNAL SQLSTATE '75000'; END MODULE", "§9.21 SR5");
    refused("DROP PROCEDURE PLACE RESTRICT", "§11.62 SR2");
    refused("DROP MODULE NONE RESTRICT", "§9.22 SR2");
    let e = a.run("CREATE MODULE M NAMES ARE LATIN1 PROCEDURE P () CONTAINS SQL SIGNAL SQLSTATE '75000'; END MODULE").unwrap_err();
    assert_eq!(e.sqlcode(), -900, "{e:?}");
    a.rollback().unwrap();
    a.run("CREATE MODULE M NAMES ARE SQL_TEXT PROCEDURE P () CONTAINS SQL SIGNAL SQLSTATE '75000'; END MODULE").unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §4.2.1, §12.8 GR1, GR3; ISO/IEC 9075:1992 §13.11 GR4, GR5]
/// A module's declared local temporary table is named MODULE.T within it
/// alone, an instance of it each session's: rows one session's routines
/// insert the other's do not see, ON COMMIT DELETE ROWS empties it at
/// commit, and no view of the Information Schema shows it.
#[test]
fn a_module_s_declared_tables_are_each_session_s_12_8() {
    let db = defined();
    let a = db.session("A");
    a.run(
        "CREATE MODULE LOG \
         DECLARE LOCAL TEMPORARY TABLE MODULE.SEEN (V INTEGER) ON COMMIT DELETE ROWS \
         DECLARE LOCAL TEMPORARY TABLE MODULE.KEPT (V INTEGER) ON COMMIT PRESERVE ROWS \
         PROCEDURE NOTE (IN X INTEGER) MODIFIES SQL DATA BEGIN INSERT INTO MODULE.SEEN VALUES (X); INSERT INTO MODULE.KEPT VALUES (X); END; \
         FUNCTION SEEN () RETURNS INTEGER READS SQL DATA RETURN (SELECT COUNT(*) FROM MODULE.SEEN); \
         FUNCTION KEPT () RETURNS INTEGER READS SQL DATA RETURN (SELECT COUNT(*) FROM MODULE.KEPT); \
         END MODULE",
    )
    .unwrap();
    a.commit().unwrap();
    a.run("CALL NOTE(1)").unwrap();
    a.run("CALL NOTE(2)").unwrap();
    assert_eq!(rows(&a, "SELECT SEEN(), KEPT() FROM T"), ["2,2"]);
    let b = db.session("A");
    assert_eq!(rows(&b, "SELECT SEEN(), KEPT() FROM T"), ["0,0"]);
    b.commit().unwrap();
    a.commit().unwrap();
    assert_eq!(rows(&a, "SELECT SEEN(), KEPT() FROM T"), ["0,2"]);
    let Err(e) = a.open_cursor("SELECT V FROM MODULE.KEPT") else { panic!("MODULE.KEPT read outside its module") };
    assert_eq!(e.sqlcode(), -103, "{e:?}");
    a.rollback().unwrap();
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_NAME IN ('SEEN', 'KEPT')"), ["0"]);
    let e = a.run(
        "CREATE MODULE TWO DECLARE LOCAL TEMPORARY TABLE MODULE.X (V INTEGER) DECLARE LOCAL TEMPORARY TABLE MODULE.X (V INTEGER) \
                   PROCEDURE P () CONTAINS SQL SIGNAL SQLSTATE '75000'; END MODULE",
    );
    assert_eq!(e.unwrap_err().sqlcode(), -103);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §9.21 SR4, SR6, SR7, §8.1 SR2, SR4]
/// A name of a routine a module's routine invokes without a schema is of
/// the first schema of the module's path that has one: LIB.TWICE before
/// A.TWICE under PATH LIB, A, and A's alone with no path; the module's
/// other names are of its default schema, its SCHEMA clause's.
#[test]
fn the_module_path_qualifies_routine_names_9_21() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE TABLE LIB.L (K INTEGER)").unwrap();
    a.run("INSERT INTO LIB.L VALUES (7)").unwrap();
    a.run(
        "CREATE MODULE PATHED SCHEMA LIB PATH LIB, A \
         FUNCTION BY_PATH (X INTEGER) RETURNS INTEGER CONTAINS SQL RETURN TWICE(X); \
         FUNCTION FROM_L () RETURNS INTEGER READS SQL DATA RETURN (SELECT K FROM L); \
         PROCEDURE PUT (IN X INTEGER) MODIFIES SQL DATA INSERT INTO L VALUES (TWICE(X)); \
         END MODULE",
    )
    .unwrap();
    a.run("CREATE MODULE PLAIN FUNCTION BY_SCHEMA (X INTEGER) RETURNS INTEGER CONTAINS SQL RETURN TWICE(X); END MODULE").unwrap();
    a.commit().unwrap();
    assert_eq!(rows(&a, "SELECT BY_PATH(5), BY_SCHEMA(5), FROM_L() FROM T"), ["10,15,7"]);
    a.run("CALL PUT(4)").unwrap();
    assert_eq!(rows(&a, "SELECT K FROM LIB.L ORDER BY K"), ["7", "8"]);
    let path = "SELECT MODULE_NAME, DEFAULT_SCHEMA_NAME, SQL_PATH FROM INFORMATION_SCHEMA.MODULES ORDER BY MODULE_NAME";
    assert_eq!(rows(&a, path), ["PATHED,LIB,PATH LIB, A", "PLAIN,A,NULL"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §9.22 SR3, GR1, GR2, §9.25 SR1]
/// DROP MODULE RESTRICT is refused while a view or a routine outside it
/// invokes a routine of it; with CASCADE they go with it, and its routines
/// and declared tables; and a drop of a routine a routine of a module
/// invokes takes the whole module, and a view invoking another of its
/// routines.
#[test]
fn drop_module_restrict_and_cascade_9_22() {
    let db = defined();
    let a = db.session("A");
    a.run(SHOP).unwrap();
    a.run("CREATE VIEW V AS SELECT COUNTED() AS N FROM T").unwrap();
    a.run("CREATE PROCEDURE OUTSIDE () MODIFIES SQL DATA CALL PLACE(9)").unwrap();
    a.commit().unwrap();
    let e = a.run("DROP MODULE SHOP RESTRICT").unwrap_err();
    assert_eq!(e.sqlcode(), -103, "{e:?}");
    assert!(e.rule.as_deref().unwrap_or_default().contains("§9.22 SR3"), "{e:?}");
    a.rollback().unwrap();
    a.run("DROP MODULE SHOP CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.VIEWS WHERE TABLE_NAME = 'V'"), ["0"]);
    assert_eq!(rows(&a, "SELECT ROUTINE_NAME FROM INFORMATION_SCHEMA.ROUTINES WHERE ROUTINE_SCHEMA = 'A'"), ["TWICE"]);
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.MODULES"), ["0"]);
    a.run(
        "CREATE MODULE USES FUNCTION ONE () RETURNS INTEGER CONTAINS SQL RETURN LIB.TWICE(1); \
           FUNCTION OTHER () RETURNS INTEGER CONTAINS SQL RETURN 0; END MODULE",
    )
    .unwrap();
    a.run("CREATE VIEW W AS SELECT OTHER() AS Z FROM T").unwrap();
    a.commit().unwrap();
    let e = a.run("DROP FUNCTION LIB.TWICE RESTRICT").unwrap_err();
    assert_eq!(e.sqlcode(), -103, "{e:?}");
    a.rollback().unwrap();
    a.run("DROP FUNCTION LIB.TWICE CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.ROUTINES WHERE MODULE_NAME = 'USES'"), ["0"]);
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.VIEWS WHERE TABLE_NAME = 'W'"), ["0"]);
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.MODULES"), ["0"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §9.2 SR1, GR1, GR3, §9.1]
/// A module is a schema element: defined in a `<schema definition>`; DROP
/// SCHEMA RESTRICT is refused while the schema holds one, and with CASCADE
/// drops it, and a module of another schema whose path names it.
#[test]
fn modules_of_a_schema_and_drop_schema_9_2() {
    let db = defined();
    db.schema(
        "CREATE SCHEMA S AUTHORIZATION A \
         CREATE TABLE U (K INTEGER) \
         CREATE MODULE KEEPER PROCEDURE ADD_U (IN X INTEGER) MODIFIES SQL DATA INSERT INTO U VALUES (X); END MODULE \
         GRANT EXECUTE ON MODULE KEEPER TO B",
    )
    .unwrap();
    let b = db.session("B");
    b.run("CALL S.ADD_U(1)").unwrap();
    b.commit().unwrap();
    let a = db.session("A");
    assert_eq!(rows(&a, "SELECT K FROM S.U"), ["1"]);
    a.run("CREATE MODULE NAMING PATH S, A FUNCTION Z () RETURNS INTEGER CONTAINS SQL RETURN 0; END MODULE").unwrap();
    a.commit().unwrap();
    let e = a.run("DROP SCHEMA S RESTRICT").unwrap_err();
    assert_eq!(e.sqlcode(), -103, "{e:?}");
    assert!(e.msg.contains("KEEPER"), "{e:?}");
    a.rollback().unwrap();
    a.run("DROP SCHEMA S CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.MODULES"), ["0"]);
    let e =
        db.schema("CREATE SCHEMA R AUTHORIZATION A CREATE MODULE A.WRONG FUNCTION Z () RETURNS INTEGER CONTAINS SQL RETURN 0; END MODULE");
    assert!(e.unwrap_err().rule.as_deref().unwrap_or_default().contains("§9.21 SR1"));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §9.21 GR3, §12.8] a module is kept with
/// the definitions, its routines, declared tables, path and privileges:
/// after stopping without a checkpoint, by its logged definition, and
/// after a checkpoint, by the snapshot; and its drop too.
#[test]
fn modules_are_kept_across_reopening() {
    let scratch = Scratch::new("modules-reopen");
    let dir = scratch.path("db");
    let module = "CREATE MODULE KEPT PATH LIB, A \
                  DECLARE LOCAL TEMPORARY TABLE MODULE.N (V INTEGER) ON COMMIT PRESERVE ROWS \
                  PROCEDURE NOTE (IN X INTEGER) MODIFIES SQL DATA INSERT INTO MODULE.N VALUES (TWICE(X)); \
                  FUNCTION NOTED () RETURNS INTEGER READS SQL DATA RETURN (SELECT SUM(V) FROM MODULE.N); END MODULE";
    {
        let db = Database::open(&dir).unwrap();
        db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER)").unwrap();
        db.schema("CREATE SCHEMA LIB AUTHORIZATION A CREATE FUNCTION TWICE (X INTEGER) RETURNS INTEGER CONTAINS SQL RETURN X * 2").unwrap();
        let a = db.session("A");
        a.run("INSERT INTO T VALUES (1)").unwrap();
        a.run(module).unwrap();
        a.run("CREATE MODULE GONE FUNCTION Z () RETURNS INTEGER CONTAINS SQL RETURN 0; END MODULE").unwrap();
        a.run("GRANT EXECUTE ON MODULE KEPT TO B").unwrap();
        a.commit().unwrap();
        a.run("DROP MODULE GONE RESTRICT").unwrap();
        a.commit().unwrap();
    }
    let check = |db: &Database| {
        let b = db.session("B");
        b.run("CALL A.NOTE(3)").unwrap();
        assert_eq!(rows(&b, "SELECT A.NOTED() FROM (VALUES (0)) AS X (Z)"), ["6"]);
        b.rollback().unwrap();
        let modules = "SELECT MODULE_NAME, SQL_PATH FROM INFORMATION_SCHEMA.MODULES";
        assert_eq!(rows(&db.session("A"), modules), ["KEPT,PATH LIB, A"]);
    };
    {
        let db = Database::open(&dir).unwrap();
        check(&db);
        db.checkpoint().unwrap();
        db.close().unwrap();
    }
    let db = Database::open(&dir).unwrap();
    check(&db);
    db.close().unwrap();
}

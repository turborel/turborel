//! What is dropped with what a trigger depends on [ISO/IEC 9075-2:2023
//! §11.2, §11.23, §11.26, §11.31, §11.33], and what a triggered action
//! sees of a table altered after it [§11.21 Note 561].

use turborel::db::{Database, Session};
use turborel::error::Code;

/// Schema A: T (K, N), the subject; L, a log; S, a table actions read.
fn defined() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (K INTEGER NOT NULL PRIMARY KEY, N INTEGER, M INTEGER)
           CREATE TABLE L (K INTEGER, N INTEGER)
           CREATE TABLE S (K INTEGER)",
    )
    .unwrap();
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

/// Asserts `sql` is refused in `a` with SQLCODE -103, citing `cited`.
fn refused(a: &Session, sql: &str, cited: &str) {
    let e = a.run(sql).unwrap_err();
    assert_eq!(e.code, Code::SyntaxRule, "{e}");
    assert!(e.to_string().contains(cited), "{e}");
}

/// Whether trigger `name` of schema A is defined: DROP TRIGGER of it
/// works.
fn defined_trigger(a: &Session, name: &str) -> bool {
    let dropped = a.run(&format!("DROP TRIGGER {name}")).is_ok();
    a.rollback().unwrap();
    dropped
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.31 GR4b v] DROP TABLE of a trigger's
/// subject table drops the trigger, with RESTRICT too.
#[test]
fn dropping_the_subject_drops_its_triggers() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE TRIGGER X AFTER INSERT ON S FOR EACH STATEMENT DELETE FROM L").unwrap();
    a.commit().unwrap();
    a.run("DROP TABLE S RESTRICT").unwrap();
    a.commit().unwrap();
    assert!(!defined_trigger(&a, "X"));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.31 SR7g, GR4b v, §11.33 SR5c] a
/// table or view a triggered action names is dropped only with CASCADE,
/// which drops the trigger.
#[test]
fn dropping_what_an_action_names_needs_cascade() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE VIEW V AS SELECT K FROM S").unwrap();
    a.run("CREATE TRIGGER X AFTER INSERT ON T FOR EACH STATEMENT INSERT INTO L SELECT COUNT(*), 0 FROM V").unwrap();
    a.run("CREATE TRIGGER Y AFTER DELETE ON T FOR EACH STATEMENT DELETE FROM S").unwrap();
    a.commit().unwrap();
    refused(&a, "DROP VIEW V RESTRICT", "§11.33 SR5c");
    a.run("DROP VIEW V CASCADE").unwrap();
    a.commit().unwrap();
    assert!(!defined_trigger(&a, "X"));
    refused(&a, "DROP TABLE S RESTRICT", "§11.31 SR7g");
    a.run("DROP TABLE S CASCADE").unwrap();
    a.commit().unwrap();
    assert!(!defined_trigger(&a, "Y"));
    a.run("INSERT INTO T VALUES (1, 1, 1)").unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.23 SR7e, GR1] a column of a trigger
/// column list or of a triggered action column set is dropped only with
/// CASCADE, which drops the trigger; another column leaves it.
#[test]
fn dropping_a_column_a_trigger_uses_needs_cascade() {
    let db = defined();
    let a = db.session("A");
    a.run("CREATE TRIGGER LISTED AFTER UPDATE OF M ON T FOR EACH STATEMENT DELETE FROM L").unwrap();
    a.run("CREATE TRIGGER USED AFTER INSERT ON T REFERENCING NEW AS NR FOR EACH ROW INSERT INTO L VALUES (NR.K, NR.N)").unwrap();
    a.commit().unwrap();
    refused(&a, "ALTER TABLE T DROP COLUMN M RESTRICT", "§11.23 SR7e");
    refused(&a, "ALTER TABLE T DROP COLUMN N RESTRICT", "§11.23 SR7e");
    a.run("ALTER TABLE T DROP COLUMN M CASCADE").unwrap();
    a.commit().unwrap();
    assert!(!defined_trigger(&a, "LISTED"));
    assert!(defined_trigger(&a, "USED"));
    a.run("INSERT INTO T VALUES (1, 10)").unwrap();
    assert_eq!(rows(&a, "SELECT K, N FROM L"), ["1,10"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.2 SR4j, GR9] a schema holding a
/// trigger is dropped only with CASCADE; a trigger of another schema on
/// one of its tables goes with the table.
#[test]
fn dropping_a_schema_drops_its_triggers() {
    let db = defined();
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE U (K INTEGER)").unwrap();
    let (a, b) = (db.session("A"), db.session("B"));
    b.run("GRANT TRIGGER ON U TO A").unwrap();
    b.commit().unwrap();
    a.run("CREATE TRIGGER ON_U AFTER INSERT ON B.U FOR EACH STATEMENT DELETE FROM L").unwrap();
    a.commit().unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION D").unwrap();
    b.run("GRANT TRIGGER, DELETE ON U TO D").unwrap();
    b.commit().unwrap();
    let d = db.session("D");
    d.run("CREATE TRIGGER D.X AFTER INSERT ON B.U FOR EACH STATEMENT DELETE FROM B.U").unwrap();
    d.commit().unwrap();
    refused(&d, "DROP SCHEMA D RESTRICT", "D.X");
    d.run("DROP SCHEMA D CASCADE").unwrap();
    d.commit().unwrap();
    b.run("INSERT INTO U VALUES (1)").unwrap();
    assert_eq!(rows(&b, "SELECT K FROM U"), ["1"]);
    b.run("DROP SCHEMA B CASCADE").unwrap();
    b.commit().unwrap();
    assert!(!defined_trigger(&a, "ON_U"));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.26 SR13f, GR5] a constraint a
/// triggered action's query depends on, by the functional dependency of
/// its key, is dropped only with CASCADE, which drops the trigger.
#[test]
fn dropping_a_constraint_an_action_depends_on_needs_cascade() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (K INTEGER NOT NULL, N INTEGER, CONSTRAINT KEYED PRIMARY KEY (K))
           CREATE TABLE L (K INTEGER, N INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    a.run("CREATE TRIGGER X AFTER DELETE ON L FOR EACH STATEMENT INSERT INTO L SELECT K, N FROM T GROUP BY K").unwrap();
    a.commit().unwrap();
    refused(&a, "ALTER TABLE T DROP CONSTRAINT KEYED RESTRICT", "§11.26 SR13f");
    a.run("ALTER TABLE T DROP CONSTRAINT KEYED CASCADE").unwrap();
    a.commit().unwrap();
    assert!(!defined_trigger(&a, "X"));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.21 GR2, Note 561] a column added to
/// a table after a trigger is defined changes nothing its action does:
/// not its SELECT *, its INSERT without a column list, nor its transition
/// tables.
#[test]
fn an_added_column_changes_no_action() {
    let db = defined();
    let a = db.session("A");
    a.run(
        "CREATE TRIGGER X AFTER INSERT ON T REFERENCING NEW TABLE AS ADDED FOR EACH STATEMENT
           INSERT INTO L SELECT K, N FROM (SELECT * FROM ADDED) AS D (K, N, M)",
    )
    .unwrap();
    a.run("CREATE TRIGGER Y AFTER DELETE ON T FOR EACH STATEMENT INSERT INTO L SELECT K, N FROM (SELECT * FROM S) AS D (K, N)")
        .unwrap_err();
    a.run("CREATE TRIGGER Y AFTER DELETE ON T FOR EACH STATEMENT INSERT INTO S VALUES (9)").unwrap();
    a.commit().unwrap();
    a.run("ALTER TABLE T ADD COLUMN Z INTEGER").unwrap();
    a.run("ALTER TABLE S ADD COLUMN Z INTEGER").unwrap();
    a.commit().unwrap();
    a.run("INSERT INTO T VALUES (1, 10, 100, 1000)").unwrap();
    a.run("DELETE FROM T").unwrap();
    assert_eq!(rows(&a, "SELECT K, N FROM L"), ["1,10"]);
    assert_eq!(rows(&a, "SELECT K, Z FROM S"), ["9,NULL"]);
}

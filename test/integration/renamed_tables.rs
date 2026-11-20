//! A table's RENAME TO \[EXTENSION\] (RULES.md D402): the table, its rows,
//! indexes, constraints, privileges and triggers known by the new name in
//! either engine; refused while a stored text names it, as a column's
//! RENAME is (D83); undone by ROLLBACK, kept by a reopen and recovered
//! after a crash.

use turborel::db::{Database, Opened};
use turborel::error::Code;

use crate::common::{Scratch, alter_failed, committed_each, query, reopened_after_crash, rows, schema_a};

/// The engines a table is made in.
const ENGINES: [&str; 2] = ["ROWSTORE", "COLUMNSTORE"];

/// Verifies: [EXTENSION] RULES.md D402. A table renamed keeps its rows,
/// its key and the index on it, its NOT NULL and CHECK constraints, a
/// foreign key of another table that references it, its identity's
/// counter, its privileges and its trigger, in either engine; the old
/// name names no table, and the Information Schema shows the new one.
#[test]
fn a_table_renamed_keeps_all_but_its_name() {
    for engine in ENGINES {
        let db = schema_a();
        db.schema("CREATE SCHEMA AUTHORIZATION B").unwrap();
        let s = db.session("A");
        committed_each(
            &s,
            &[
                &format!(
                    "CREATE TABLE T (ID INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY, V VARCHAR(5) NOT NULL CHECK (V <> 'bad')) USING {engine}"
                ),
                "CREATE TABLE C (K INTEGER REFERENCES T (ID))",
                "CREATE TABLE LOG (N INTEGER)",
                "CREATE TRIGGER TR AFTER INSERT ON T FOR EACH ROW INSERT INTO LOG VALUES (1)",
                "INSERT INTO T (V) VALUES ('one'), ('two')",
                "GRANT SELECT, INSERT ON T TO B",
            ],
        );
        committed_each(&s, &["CREATE INDEX T_V ON T (V)"]);
        committed_each(&s, &["ALTER TABLE T RENAME TO U"]);
        assert_eq!(rows(&s, "SELECT * FROM T"), Err(Code::SyntaxRule));
        s.rollback().unwrap();
        assert_eq!(query(&s, "SELECT ID, V FROM U WHERE V = 'two'"), ["2,two"]);
        committed_each(&s, &["INSERT INTO U (V) VALUES ('three')", "INSERT INTO C VALUES (3)"]);
        assert_eq!(query(&s, "SELECT ID FROM U WHERE V = 'three'"), ["3"], "{engine}: the identity goes on");
        assert_eq!(query(&s, "SELECT COUNT(*) FROM LOG"), ["3"], "{engine}: the trigger fires");
        for (sql, why) in [
            ("INSERT INTO U (V) VALUES ('bad')", "check"),
            ("INSERT INTO U (V) VALUES (NULL)", "not null"),
            ("INSERT INTO C VALUES (9)", "key"),
        ] {
            assert_eq!(s.run(sql).unwrap_err().code, Code::Constraint, "{engine}: {why}");
            s.rollback().unwrap();
        }
        assert_eq!(s.run("DELETE FROM U WHERE ID = 3").unwrap_err().code, Code::Constraint, "{engine}: referenced");
        s.rollback().unwrap();
        let b = db.session("B");
        committed_each(&b, &["INSERT INTO A.U (V) VALUES ('four')"]);
        assert_eq!(query(&b, "SELECT COUNT(*) FROM A.U"), ["4"]);
        let tables = "SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = 'A' ORDER BY 1";
        assert_eq!(query(&s, tables), ["C", "LOG", "U"]);
        let grants = "SELECT PRIVILEGE_TYPE FROM INFORMATION_SCHEMA.TABLE_PRIVILEGES WHERE GRANTEE = 'B' AND TABLE_NAME = 'U' ORDER BY 1";
        assert_eq!(query(&s, grants), ["INSERT", "SELECT"]);
        let triggers = "SELECT EVENT_OBJECT_TABLE FROM INFORMATION_SCHEMA.TRIGGERS WHERE TRIGGER_NAME = 'TR'";
        assert_eq!(query(&s, triggers), ["U"]);
        committed_each(&s, &["CREATE TABLE T (X INTEGER)", "INSERT INTO T VALUES (7)"]);
        assert_eq!(query(&s, "SELECT X FROM T"), ["7"], "{engine}: the old name is free");
    }
}

/// Verifies: [EXTENSION] RULES.md D402; [ISO/IEC 9075:1992 §11.3 SR2;
/// ISO/IEC 9075-2:2023 §11.10 AR1] RENAME TO is refused of a name a table
/// or view of the schema has, with SQLCODE -103; of another schema's
/// name; by a user who does not own the schema, with SQLCODE -104; and of
/// a typed, temporary or system-versioned table.
#[test]
fn a_rename_takes_a_free_name_of_its_schema() {
    let db = schema_a();
    db.schema("CREATE SCHEMA AUTHORIZATION B").unwrap();
    let s = db.session("A");
    committed_each(
        &s,
        &[
            "CREATE TABLE T (K INTEGER)",
            "CREATE TABLE U (K INTEGER)",
            "CREATE VIEW W AS SELECT 1 AS ONE FROM U",
            "CREATE GLOBAL TEMPORARY TABLE G (K INTEGER)",
            "CREATE TABLE S (K INTEGER, B TIMESTAMP(6) GENERATED ALWAYS AS ROW START, \
             E TIMESTAMP(6) GENERATED ALWAYS AS ROW END, PERIOD FOR SYSTEM_TIME (B, E)) WITH SYSTEM VERSIONING",
            "CREATE TYPE PT AS (X INTEGER) NOT FINAL",
            "CREATE TABLE TT OF PT (REF IS ID SYSTEM GENERATED)",
            "GRANT ALL PRIVILEGES ON T TO B",
        ],
    );
    assert_eq!(alter_failed(&s, "ALTER TABLE T RENAME TO U", "§11.3 SR2").code, Code::SyntaxRule);
    alter_failed(&s, "ALTER TABLE T RENAME TO W", "§11.3 SR2");
    alter_failed(&s, "ALTER TABLE T RENAME TO B.T", "its schema");
    alter_failed(&s, "ALTER TABLE G RENAME TO G2", "temporary");
    alter_failed(&s, "ALTER TABLE S RENAME TO S2", "system-versioned");
    alter_failed(&s, "ALTER TABLE TT RENAME TO TT2", "typed");
    let b = db.session("B");
    let e = b.run("ALTER TABLE A.T RENAME TO X").unwrap_err();
    b.rollback().unwrap();
    assert_eq!(e.code, Code::Privilege, "{e}");
    assert_eq!(alter_failed(&s, "ALTER TABLE T RENAME U2", "expected").code, Code::Syntax);
    let e = s.run("ALTER TABLE T RENAME TO U GENERATION 3").unwrap_err();
    s.rollback().unwrap();
    assert_eq!(e.code, Code::Syntax, "the generation is the log's alone: {e}");
}

/// Verifies: [EXTENSION] RULES.md D402, D83. Stored texts are not
/// rewritten, so the table is refused RENAME TO while a view, a CHECK
/// that reads it, an assertion, a trigger's action, a routine or an
/// index expression names it; once that is dropped, it is renamed.
#[test]
fn a_rename_waits_for_the_texts_that_name_the_table() {
    let db = schema_a();
    let s = db.session("A");
    committed_each(&s, &["CREATE TABLE T (K INTEGER)", "CREATE TABLE O (K INTEGER)", "CREATE TABLE LOG (N INTEGER)"]);
    let dependents = [
        ("CREATE VIEW V AS SELECT K FROM T", "DROP VIEW V CASCADE", "view A.V"),
        ("CREATE ASSERTION SMALL CHECK (NOT EXISTS (SELECT * FROM T WHERE K > 9))", "DROP ASSERTION SMALL", "constraint A.SMALL"),
        ("CREATE TRIGGER G AFTER INSERT ON O FOR EACH ROW INSERT INTO T VALUES (1)", "DROP TRIGGER G", "trigger A.G"),
        ("CREATE FUNCTION F () RETURNS INTEGER READS SQL DATA RETURN (SELECT MAX(K) FROM T)", "DROP FUNCTION F CASCADE", "routine"),
    ];
    for (make, drop, what) in dependents {
        committed_each(&s, &[make]);
        alter_failed(&s, "ALTER TABLE T RENAME TO U", what);
        committed_each(&s, &[drop]);
    }
    committed_each(&s, &["ALTER TABLE T RENAME TO U", "ALTER TABLE U RENAME TO T"]);
    assert_eq!(query(&s, "SELECT COUNT(*) FROM T"), ["0"]);
}

/// Verifies: [ISO/IEC 9075:1992 §4.28] RENAME TO within a transaction is
/// undone by ROLLBACK, with the rows written under the new name; a
/// statement prepared on the old name runs again once it is back.
#[test]
fn a_rename_is_undone_by_rollback() {
    for engine in ENGINES {
        let db = schema_a();
        let s = db.session("A");
        committed_each(&s, &[&format!("CREATE TABLE T (K INTEGER) USING {engine}"), "INSERT INTO T VALUES (1)"]);
        let p = s.prepare("INSERT INTO T VALUES (5)").unwrap();
        s.commit().unwrap();
        s.run("ALTER TABLE T RENAME TO U").unwrap();
        s.run("INSERT INTO U VALUES (2)").unwrap();
        assert_eq!(query(&s, "SELECT COUNT(*) FROM U"), ["2"]);
        assert!(s.execute_prepared(&p, &[]).is_err(), "{engine}: T is no table now");
        s.rollback().unwrap();
        assert_eq!(rows(&s, "SELECT * FROM U"), Err(Code::SyntaxRule));
        s.rollback().unwrap();
        assert_eq!(query(&s, "SELECT K FROM T"), ["1"], "{engine}");
        assert!(s.execute_prepared(&p, &[]).is_ok(), "{engine}");
        assert_eq!(query(&s, "SELECT K FROM T ORDER BY K"), ["1", "5"]);
        s.rollback().unwrap();
    }
}

/// The state the renames leave, read in `db`.
fn renamed_state(db: &Database) -> Vec<String> {
    let s = db.session("A");
    let mut state = query(&s, "SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = 'A' ORDER BY 1");
    state.extend(query(&s, "SELECT K, V FROM R2 ORDER BY K"));
    state.extend(query(&s, "SELECT K FROM R2 WHERE V = 'b'"));
    state.extend(query(&s, "SELECT K FROM T"));
    state
}

/// What `renamed_state` reads of the renames done.
const RENAMED: [&str; 6] = ["R2", "T", "1,a", "2,b", "2", "9"];

/// Renames T to R and R to R2 in schema A of `db`, its table made in
/// `engine`, and makes another T, so the old name's generations go on.
/// A table R2 dropped before a checkpoint gives the R2 renamed a later
/// generation, which the image does not hold but the rename's record
/// does.
fn rename_each(db: &Database, engine: &str) {
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let s = db.session("A");
    committed_each(&s, &["CREATE TABLE R2 (X INTEGER)", "INSERT INTO R2 VALUES (0)"]);
    committed_each(&s, &["DROP TABLE R2 CASCADE"]);
    db.checkpoint().unwrap();
    committed_each(&s, &[&format!("CREATE TABLE T (K INTEGER PRIMARY KEY, V VARCHAR(3)) USING {engine}")]);
    committed_each(&s, &["CREATE INDEX T_V ON T (V)"]);
    committed_each(&s, &["INSERT INTO T VALUES (1, 'a')", "ALTER TABLE T RENAME TO R", "INSERT INTO R VALUES (2, 'b')"]);
    committed_each(&s, &["ALTER TABLE R RENAME TO R2", "CREATE TABLE T (K INTEGER)", "INSERT INTO T VALUES (9)"]);
}

/// Verifies: [DESIGN] RULES.md D25, D33, D402: renames, logged with the
/// generation each gives the table, are kept by a reopen, from the log
/// and from a checkpoint's image, in either engine.
#[test]
fn renames_are_kept_by_a_reopen() {
    for engine in ENGINES {
        let scratch = Scratch::new("renamed-tables-reopen");
        let dir = scratch.path("db");
        {
            let db = Database::open(&dir).unwrap();
            rename_each(&db, engine);
            assert_eq!(renamed_state(&db), RENAMED);
        }
        let db = Database::open(&dir).unwrap();
        assert_eq!(renamed_state(&db), RENAMED, "{engine}: from the log");
        db.checkpoint().unwrap();
        drop(db);
        let db = Database::open(&dir).unwrap();
        assert_eq!(renamed_state(&db), RENAMED, "{engine}: from the image");
    }
}

/// Verifies: [DESIGN] RULES.md D25, D402: after a crash, the renames
/// committed are recovered with the rows written under each name; one
/// whose transaction was open left the table as it was.
#[test]
fn renames_are_recovered_after_a_crash() {
    for engine in ENGINES {
        let scratch = Scratch::new("renamed-tables-crash");
        let dir = scratch.path("db");
        let (crashed, _) = reopened_after_crash(&dir);
        rename_each(&crashed, engine);
        let open = crashed.session("A");
        open.run("ALTER TABLE R2 RENAME TO R3").unwrap();
        for _ in 0..2 {
            let (db, opened) = reopened_after_crash(&dir);
            assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
            assert_eq!(renamed_state(&db), RENAMED, "{engine}");
        }
        drop((open, crashed));
    }
}

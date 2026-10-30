//! \[EXTENSION\] Column defaults [ISO/IEC 9075:1992 §11.5]: given by a
//! `<default clause>`, taken by INSERT, DEFAULT VALUES and SET ... =
//! DEFAULT [§13.8, §13.9], by a column added [§11.11 GR2] and by the
//! referential action SET DEFAULT [§11.8 GR5, GR6]; and set or dropped by
//! ALTER COLUMN [§11.12-§11.14], in a transaction, kept across a reopen.

use turborel::db::{Database, OpenOptions, Opened};
use turborel::error::Code;

use crate::common::{Scratch, after_crash, exec, rows, sorted};

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.5 SR3, GR1, GR2; §6.2 SR3,
/// GR9-GR11]: what a `<default clause>` refuses: a string longer than its
/// column, a number of another type or losing a digit, a string for a
/// number and a number for a string, an authorization identifier for a
/// number or a column shorter than 128; and
/// the defaults it gives: a literal, padded; a number converted; NULL;
/// USER, CURRENT_USER, SESSION_USER and SYSTEM_USER, each the session's
/// authorization identifier.
#[test]
fn what_defaults_are_11_5() {
    let db = Database::new();
    let refused = [
        "CREATE TABLE T (A CHAR(2) DEFAULT 'ABC')",
        "CREATE TABLE T (A INTEGER DEFAULT 1.5)",
        "CREATE TABLE T (A INTEGER DEFAULT 1E0)",
        "CREATE TABLE T (A INTEGER DEFAULT 'A')",
        "CREATE TABLE T (A CHAR(3) DEFAULT 1)",
        "CREATE TABLE T (A CHAR(127) DEFAULT USER)",
        "CREATE TABLE T (A INTEGER DEFAULT CURRENT_USER)",
    ];
    for table in refused {
        let e = db.schema(&format!("CREATE SCHEMA AUTHORIZATION S {table}")).expect_err(table);
        assert_eq!(e.code, Code::SyntaxRule, "{table}");
    }
    db.schema(
        "CREATE SCHEMA AUTHORIZATION S
           CREATE TABLE T (K INTEGER, C CHAR(3) DEFAULT 'A', N DECIMAL(5,2) DEFAULT -2, F FLOAT DEFAULT 3, Z INTEGER DEFAULT NULL,
                           U CHAR(128) DEFAULT USER, V CHAR(128) DEFAULT SESSION_USER, W CHAR(128) DEFAULT SYSTEM_USER,
                           X CHAR(128) DEFAULT CURRENT_USER)",
    )
    .unwrap();
    let s = db.session("S");
    s.run("INSERT INTO T (K) VALUES (1)").unwrap();
    assert_eq!(rows(&s, "SELECT K, C, N, F, Z FROM T"), Ok(vec!["1,A  ,-2.00,3E0,NULL".into()]));
    let users = "SELECT K FROM T WHERE U = 'S' AND V = 'S' AND W = 'S' AND X = 'S' AND U = USER AND X = CURRENT_USER";
    assert_eq!(rows(&s, users), Ok(vec!["1".into()]), "each the authorization identifier");
    assert_eq!(rows(&s, "SELECT K FROM T WHERE SESSION_USER = 'S' AND SYSTEM_USER = 'S'"), Ok(vec!["1".into()]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §13.8 SR2, GR4b; §7.1; §13.9
/// GR5; §13.10]: a column not inserted into takes its default, as does one
/// given DEFAULT, every one of DEFAULT VALUES, one set to DEFAULT, and a
/// base table's column through a view; a NOT NULL column with no default
/// left out is refused.
#[test]
fn statements_take_defaults_13_8() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION S
           CREATE TABLE T (K INTEGER DEFAULT 7, C CHAR(2) DEFAULT 'X', R INTEGER DEFAULT 0 NOT NULL)
           CREATE TABLE M (K INTEGER NOT NULL, C INTEGER)
           CREATE VIEW V (K) AS SELECT K FROM T",
    )
    .unwrap();
    let s = db.session("S");
    s.run("INSERT INTO T (C) VALUES ('A')").unwrap();
    s.run("INSERT INTO T VALUES (1, DEFAULT, 5)").unwrap();
    s.run("INSERT INTO T DEFAULT VALUES").unwrap();
    s.run("INSERT INTO V VALUES (3)").unwrap();
    s.run("INSERT INTO M VALUES (1, 1)").unwrap();
    s.run("INSERT INTO T (K, C) SELECT K, 'Q' FROM M").unwrap();
    assert_eq!(sorted(&s, "SELECT K, C, R FROM T"), vec!["1,Q ,0", "1,X ,5", "3,X ,0", "7,A ,0", "7,X ,0"]);
    s.run("UPDATE T SET C = DEFAULT, R = DEFAULT WHERE K = 1").unwrap();
    assert_eq!(sorted(&s, "SELECT K, C, R FROM T WHERE K = 1"), vec!["1,X ,0", "1,X ,0"]);
    s.run("UPDATE V SET K = DEFAULT WHERE K = 3").unwrap();
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM T WHERE K = 7"), Ok(vec!["3".into()]));
    assert_eq!(exec(&s, "INSERT INTO M (C) VALUES (1)"), Err(Code::Constraint), "K has no default");
    assert_eq!(exec(&s, "INSERT INTO M DEFAULT VALUES"), Err(Code::Constraint));
    assert_eq!(exec(&s, "UPDATE T SET K = DEFAULT, K = 1"), Err(Code::SyntaxRule), "set twice");
    assert_eq!(exec(&s, "INSERT INTO T (K) DEFAULT VALUES"), Err(Code::Syntax), "no column list");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.11 GR2; §11.12-§11.14;
/// §11.8 GR5a iii, GR6a iii]: a column added has its default in every row,
/// refused NOT NULL with none; ALTER COLUMN sets and drops a default, the
/// rows unchanged, and refuses to drop one there is not, or set one of a
/// column the table has not; SET DEFAULT gives the referencing columns
/// their defaults, on update only the one that corresponds to a column
/// changed, and refuses a default no row is referenced by.
#[test]
fn defaults_are_altered_11_12() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION S
           CREATE TABLE T (K INTEGER)
           CREATE TABLE P (A INTEGER, B INTEGER, PRIMARY KEY (A, B))
           CREATE TABLE C (A INTEGER DEFAULT 0, B INTEGER DEFAULT 0,
                           FOREIGN KEY (A, B) REFERENCES P ON DELETE SET DEFAULT ON UPDATE SET DEFAULT)",
    )
    .unwrap();
    let s = db.session("S");
    s.run("INSERT INTO T VALUES (1)").unwrap();
    s.run("ALTER TABLE T ADD COLUMN D CHAR(2) DEFAULT 'Z'").unwrap();
    assert_eq!(rows(&s, "SELECT K, D FROM T"), Ok(vec!["1,Z ".into()]), "the default in the row there was");
    assert_eq!(exec(&s, "ALTER TABLE T ADD COLUMN E INTEGER NOT NULL"), Err(Code::Constraint));
    s.run("ALTER TABLE T ADD COLUMN E INTEGER DEFAULT 4 NOT NULL").unwrap();
    s.run("ALTER TABLE T ALTER COLUMN K SET DEFAULT 9").unwrap();
    s.run("ALTER TABLE T ALTER D DROP DEFAULT").unwrap();
    s.run("INSERT INTO T (E) VALUES (5)").unwrap();
    assert_eq!(sorted(&s, "SELECT K, D, E FROM T"), vec!["1,Z ,4", "9,NULL,5"]);
    assert_eq!(exec(&s, "ALTER TABLE T ALTER COLUMN D DROP DEFAULT"), Err(Code::SyntaxRule), "none to drop");
    assert_eq!(exec(&s, "ALTER TABLE T ALTER COLUMN Q SET DEFAULT 1"), Err(Code::SyntaxRule));
    assert_eq!(exec(&s, "ALTER TABLE T ALTER COLUMN D SET DEFAULT 1"), Err(Code::SyntaxRule), "a number for CHAR");
    s.rollback().unwrap();
    assert_eq!(sorted(&s, "SELECT * FROM T"), Vec::<String>::new(), "rolled back with the rest");

    for sql in ["INSERT INTO P VALUES (0, 0)", "INSERT INTO P VALUES (1, 1)", "INSERT INTO P VALUES (2, 2)", "INSERT INTO P VALUES (2, 0)"]
    {
        s.run(sql).unwrap();
    }
    s.run("INSERT INTO C VALUES (1, 1)").unwrap();
    s.run("INSERT INTO C VALUES (2, 2)").unwrap();
    s.run("DELETE FROM P WHERE A = 1").unwrap();
    assert_eq!(sorted(&s, "SELECT A, B FROM C"), vec!["0,0", "2,2"], "ON DELETE SET DEFAULT");
    s.run("ALTER TABLE C ALTER COLUMN A SET DEFAULT 5").unwrap();
    assert_eq!(exec(&s, "DELETE FROM P WHERE A = 2 AND B = 2"), Err(Code::Constraint), "no row is (5, 0)");
    s.run("UPDATE P SET B = 7 WHERE A = 2 AND B = 2").unwrap();
    assert_eq!(sorted(&s, "SELECT A, B FROM C"), vec!["0,0", "2,0"], "B alone took its default");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.5, §11.12], RULES.md D25:
/// defaults given in a schema, by a column added and by ALTER COLUMN are
/// kept across a clean close and a crash.
#[test]
fn defaults_are_kept() {
    let scratch = Scratch::new("defaults-kept");
    let dir = scratch.path("db");
    let open = || {
        let db = Database::new();
        let opened = db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
        (db, opened)
    };
    let (db, _) = open();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER DEFAULT 1, C CHAR(1))").unwrap();
    let s = db.session("S");
    s.run("ALTER TABLE T ALTER COLUMN C SET DEFAULT 'C'").unwrap();
    s.run("ALTER TABLE T ADD D INTEGER DEFAULT 4").unwrap();
    s.commit().unwrap();
    drop(s);
    db.close().unwrap();
    drop(db);
    let (db, opened) = open();
    assert_eq!(opened, Opened::Reopened);
    let s = db.session("S");
    s.run("INSERT INTO T DEFAULT VALUES").unwrap();
    s.run("ALTER TABLE T ALTER COLUMN K DROP DEFAULT").unwrap();
    s.commit().unwrap();
    drop(s);
    let crashed = db;
    let (db, opened) = open();
    drop(crashed);
    assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
    let s = db.session("S");
    s.run("INSERT INTO T DEFAULT VALUES").unwrap();
    assert_eq!(sorted(&s, "SELECT K, C, D FROM T"), vec!["1,C,4", "NULL,C,4"]);
}

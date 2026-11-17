//! \[EXTENSION\] ALTER TABLE … RENAME COLUMN … TO … (RULES.md D83): a
//! column given a new name in place, its values, keys, indexes and
//! privileges kept, the periods and table options that name it changed
//! with it; refused while a text that reads the table names it.

use turborel::db::{Database, Session};
use turborel::error::{Code, SqlError};

use crate::common::{Scratch, rows};

/// A database of schema A.
fn database() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    db
}

/// Runs each of `sql` in `s`, which must succeed, and commits.
fn ok(s: &Session, sql: &[&str]) {
    for one in sql {
        s.run(one).unwrap_or_else(|e| panic!("{one}: {e}"));
    }
    s.commit().unwrap();
}

/// The error `sql` fails with in `s`, rolled back.
fn refused(s: &Session, sql: &str) -> SqlError {
    let e = s.run(sql).err().unwrap_or_else(|| panic!("{sql} succeeded"));
    s.rollback().unwrap();
    e
}

/// Asserts `sql` fails in `s` by Syntax Rule, its message holding `cited`.
fn refused_by(s: &Session, sql: &str, cited: &str) {
    let e = refused(s, sql);
    assert_eq!(e.code, Code::SyntaxRule, "{sql}: {e}");
    assert!(e.to_string().contains(cited), "{sql}: {e} does not cite {cited}");
}

/// The rows of `sql` in `s`.
fn query(s: &Session, sql: &str) -> Vec<String> {
    rows(s, sql).unwrap_or_else(|c| panic!("{sql}: {c:?}"))
}

/// Verifies: [EXTENSION] RULES.md D83. A column renamed keeps its values,
/// place, key, index, NOT NULL, default and privileges; the Information
/// Schema shows its new name, and the old one names no column.
#[test]
fn a_column_renamed_keeps_all_but_its_name() {
    let db = database();
    let s = db.session("A");
    db.schema("CREATE SCHEMA AUTHORIZATION B").unwrap();
    ok(
        &s,
        &[
            "CREATE TABLE T (ID INTEGER PRIMARY KEY, NAME VARCHAR(10) DEFAULT 'none' NOT NULL, N INTEGER)",
            "INSERT INTO T VALUES (1, 'one', 10), (2, 'two', 20)",
            "GRANT SELECT, UPDATE (NAME), REFERENCES (NAME) ON T TO B",
        ],
    );
    ok(&s, &["CREATE INDEX T_N ON T (N)"]);
    ok(&s, &["ALTER TABLE T RENAME COLUMN NAME TO LABEL", "ALTER TABLE T RENAME ID TO \"key\""]);
    assert_eq!(query(&s, "SELECT \"key\", LABEL, N FROM T ORDER BY 1"), ["1,one,10", "2,two,20"]);
    assert_eq!(rows(&s, "SELECT NAME FROM T"), Err(Code::SyntaxRule));
    s.rollback().unwrap();
    let e = refused(&s, "INSERT INTO T VALUES (1, 'again', 0)");
    assert_eq!(e.code, Code::Constraint, "{e}");
    let e = refused(&s, "INSERT INTO T (\"key\", LABEL) VALUES (3, NULL)");
    assert_eq!(e.code, Code::Constraint, "{e}");
    ok(&s, &["INSERT INTO T (\"key\") VALUES (3)"]);
    assert_eq!(query(&s, "SELECT LABEL FROM T WHERE \"key\" = 3"), ["none"]);
    let columns = query(&s, "SELECT COLUMN_NAME FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'T' ORDER BY ORDINAL_POSITION");
    assert_eq!(columns, ["key", "LABEL", "N"]);
    let b = db.session("B");
    assert_eq!(query(&b, "SELECT LABEL FROM A.T WHERE N = 10"), ["one"]);
    let privileges = query(
        &s,
        "SELECT COLUMN_NAME, PRIVILEGE_TYPE FROM INFORMATION_SCHEMA.COLUMN_PRIVILEGES WHERE GRANTEE = 'B' AND PRIVILEGE_TYPE <> 'SELECT' ORDER BY 2",
    );
    assert_eq!(privileges, ["LABEL,REFERENCES", "LABEL,UPDATE"]);
    b.commit().unwrap();
    ok(&b, &["UPDATE A.T SET LABEL = 'uno' WHERE N = 10"]);
    refused(&b, "UPDATE A.T SET N = 0");
}

/// Verifies: [EXTENSION] RULES.md D83; [ISO/IEC 9075:1992 §11.12 SR3,
/// §11.4 SR1; ISO/IEC 9075-2:2023 §11.3 SR2d, §11.12 SR2] The column is
/// one of the table, the new name no other column's or period's; a typed
/// table's columns and a system-versioned table's are not renamed.
#[test]
fn a_rename_names_one_column_anew() {
    let db = database();
    let s = db.session("A");
    ok(
        &s,
        &[
            "CREATE TABLE T (A INTEGER, B INTEGER, F DATE NOT NULL, U DATE NOT NULL, PERIOD FOR P (F, U))",
            "CREATE TABLE V (ID INTEGER, S TIMESTAMP(6) GENERATED ALWAYS AS ROW START, \
         E TIMESTAMP(6) GENERATED ALWAYS AS ROW END, PERIOD FOR SYSTEM_TIME (S, E)) WITH SYSTEM VERSIONING",
            "CREATE TYPE PT AS (X INTEGER) NOT FINAL",
            "CREATE TABLE TT OF PT (REF IS ID SYSTEM GENERATED)",
        ],
    );
    refused_by(&s, "ALTER TABLE T RENAME COLUMN Z TO Y", "§11.12 SR3");
    refused_by(&s, "ALTER TABLE T RENAME COLUMN A TO B", "§11.4 SR1");
    refused_by(&s, "ALTER TABLE T RENAME COLUMN A TO P", "§11.3 SR2d");
    refused_by(&s, "ALTER TABLE V RENAME COLUMN ID TO K", "§11.12 SR2");
    refused_by(&s, "ALTER TABLE TT RENAME COLUMN X TO Y", "typed table");
    let e = refused(&s, "ALTER TABLE T RENAME COLUMN A B");
    assert_eq!(e.code, Code::Syntax, "{e}");
}

/// Verifies: [EXTENSION] RULES.md D83, D31; [ISO/IEC 9075-2:2023 §11.3
/// SR2e v 2 B] A period bounded by the column is bounded by it under its
/// new name, its period constraint checking it so, and a table option of
/// columns, COLUMNSTORE.ORDER, names it anew.
#[test]
fn periods_and_options_follow_the_column() {
    let db = database();
    let s = db.session("A");
    ok(
        &s,
        &[
            "CREATE TABLE T (ID INTEGER, F DATE NOT NULL, U DATE NOT NULL, PERIOD FOR P (F, U), \
         UNIQUE (ID, P WITHOUT OVERLAPS)) USING columnstore WITH (COLUMNSTORE.ORDER = 'ID, F')",
            "INSERT INTO T VALUES (1, DATE '2020-01-01', DATE '2021-01-01')",
            "ALTER TABLE T RENAME COLUMN F TO FROM_DAY",
        ],
    );
    let periods = query(&s, "SELECT START_COLUMN_NAME, END_COLUMN_NAME FROM INFORMATION_SCHEMA.PERIODS WHERE TABLE_NAME = 'T'");
    assert_eq!(periods, ["FROM_DAY,U"]);
    let e = refused(&s, "INSERT INTO T VALUES (2, DATE '2021-01-01', DATE '2020-01-01')");
    assert_eq!(e.code, Code::Constraint, "{e}");
    let e = refused(&s, "INSERT INTO T VALUES (1, DATE '2020-06-01', DATE '2020-07-01')");
    assert_eq!(e.code, Code::Constraint, "{e}");
    ok(&s, &["UPDATE T FOR PORTION OF P FROM DATE '2020-03-01' TO DATE '2020-04-01' SET ID = 1"]);
    assert_eq!(query(&s, "SELECT COUNT(*) FROM T WHERE P CONTAINS DATE '2020-03-15'"), ["1"]);
    let order =
        query(&s, "SELECT OPTION_VALUE FROM INFORMATION_SCHEMA.TABLE_OPTIONS WHERE TABLE_NAME = 'T' AND OPTION_NAME = 'COLUMNSTORE.ORDER'");
    assert_eq!(order, ["\"ID\", \"FROM_DAY\""]);
}

/// Verifies: [EXTENSION] RULES.md D83; [ISO/IEC 9075:1992 §11.15 SR4] A
/// view, CHECK, assertion, trigger, routine or generation expression that
/// reads the table and names the column, or its new name, or a view that
/// selects all of its columns, refuses the rename; one that reads it but
/// names neither does not.
#[test]
fn a_text_naming_the_column_refuses_its_rename() {
    let db = database();
    let s = db.session("A");
    ok(&s, &["CREATE TABLE T (A INTEGER, B INTEGER, C INTEGER)", "CREATE TABLE LOG (N INTEGER)", "CREATE TABLE O (D INTEGER, Z INTEGER)"]);
    let refusing = [
        ("CREATE VIEW V1 AS SELECT A FROM T", "DROP VIEW V1 CASCADE", "view A.V1"),
        ("CREATE VIEW V2 AS SELECT * FROM T", "DROP VIEW V2 CASCADE", "view A.V2"),
        ("CREATE VIEW V3 AS SELECT B FROM T WHERE A > 0", "DROP VIEW V3 CASCADE", "view A.V3"),
        ("CREATE VIEW V4 (Q) AS SELECT (SELECT Z FROM T) FROM (VALUES (1)) AS W (Z)", "DROP VIEW V4 CASCADE", "view A.V4"),
        ("ALTER TABLE T ADD CONSTRAINT K CHECK (A > 0)", "ALTER TABLE T DROP CONSTRAINT K CASCADE", "constraint A.K"),
        (
            "ALTER TABLE O ADD CONSTRAINT K2 CHECK (D NOT IN (SELECT A FROM T))",
            "ALTER TABLE O DROP CONSTRAINT K2 CASCADE",
            "constraint A.K2",
        ),
        ("CREATE ASSERTION Q CHECK (NOT EXISTS (SELECT * FROM T WHERE A < 0))", "DROP ASSERTION Q", "assertion A.Q"),
        ("CREATE TRIGGER G AFTER UPDATE OF A ON T FOR EACH STATEMENT INSERT INTO LOG VALUES (1)", "DROP TRIGGER G", "trigger A.G"),
        ("CREATE PROCEDURE R () MODIFIES SQL DATA INSERT INTO LOG SELECT A FROM T", "DROP PROCEDURE R RESTRICT", "routine"),
        ("ALTER TABLE T ADD COLUMN G INTEGER GENERATED ALWAYS AS (A + 1)", "ALTER TABLE T DROP COLUMN G CASCADE", "column G"),
    ];
    for (create, drop, what) in refusing {
        ok(&s, &[create]);
        refused_by(&s, "ALTER TABLE T RENAME COLUMN A TO Z", what);
        ok(&s, &[drop]);
    }
    // The new name, an outer reference now, which would name the column
    // once renamed.
    ok(&s, &["CREATE VIEW NEW_NAME AS SELECT D FROM O WHERE EXISTS (SELECT 1 FROM T WHERE T.B = Z)"]);
    refused_by(&s, "ALTER TABLE T RENAME COLUMN A TO Z", "view A.NEW_NAME");
    ok(&s, &["DROP VIEW NEW_NAME CASCADE"]);
    // Naming other columns alone, and COUNT(*), refuse nothing.
    ok(
        &s,
        &[
            "CREATE VIEW OTHER AS SELECT B, COUNT(*) AS N FROM T GROUP BY B",
            "ALTER TABLE T ADD CONSTRAINT POSITIVE CHECK (B > 0)",
            "CREATE TRIGGER H AFTER INSERT ON T FOR EACH ROW INSERT INTO LOG VALUES (1)",
            "ALTER TABLE T RENAME COLUMN A TO Z",
        ],
    );
    assert_eq!(query(&s, "SELECT COUNT(*) FROM OTHER"), ["0"]);
}

/// Verifies: [EXTENSION] RULES.md D83, D57. A rename is undone by its
/// transaction's rollback, and kept, from the log, as the database is
/// opened again.
#[test]
fn a_rename_is_transactional_and_durable() {
    let scratch = Scratch::new("renamed-columns");
    let dir = scratch.path("db");
    {
        let db = Database::open(&dir).unwrap();
        db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
        let s = db.session("A");
        ok(&s, &["CREATE TABLE T (A INTEGER, B INTEGER)", "INSERT INTO T VALUES (1, 2)"]);
        s.run("ALTER TABLE T RENAME COLUMN A TO X").unwrap();
        assert_eq!(query(&s, "SELECT X FROM T"), ["1"]);
        s.rollback().unwrap();
        assert_eq!(query(&s, "SELECT A FROM T"), ["1"]);
        ok(&s, &["ALTER TABLE T RENAME COLUMN B TO Y"]);
    }
    let db = Database::open(&dir).unwrap();
    let s = db.session("A");
    assert_eq!(query(&s, "SELECT A, Y FROM T"), ["1,2"]);
    ok(&s, &["INSERT INTO T (Y) VALUES (5)"]);
    assert_eq!(query(&s, "SELECT COUNT(*) FROM T WHERE Y = 5"), ["1"]);
}

/// Verifies: [EXTENSION] RULES.md D83, D45. A statement prepared before
/// the rename is prepared again as it runs after: of the old name, it is
/// refused; of the column's place alone, it reads the column.
#[test]
fn a_prepared_statement_sees_the_new_name() {
    let db = database();
    let s = db.session("A");
    ok(&s, &["CREATE TABLE T (A INTEGER, B INTEGER)", "INSERT INTO T VALUES (1, 2)"]);
    let by_name = s.prepare("SELECT A FROM T").unwrap();
    let by_star = s.prepare("SELECT * FROM T").unwrap();
    ok(&s, &["ALTER TABLE T RENAME COLUMN A TO X"]);
    let e = s.open_prepared(&by_name, &[]).err().map(|e| e.code);
    assert_eq!(e, Some(Code::SyntaxRule));
    s.rollback().unwrap();
    let mut c = s.open_prepared(&by_star, &[]).unwrap();
    let row = c.fetch().unwrap().map(|r| r.iter().map(ToString::to_string).collect::<Vec<_>>());
    assert_eq!(row, Some(vec!["1".to_owned(), "2".to_owned()]));
}

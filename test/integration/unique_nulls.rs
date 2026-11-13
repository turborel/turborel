//! The `<unique null treatment>` of a unique constraint and of the UNIQUE
//! predicate [ISO/IEC 9075-2:2023 §4.25.3.2, §8.11, §11.7, Feature F292]:
//! NULLS NOT DISTINCT refusing a second null of a key, NULLS DISTINCT and
//! no treatment written allowing it (ID106), its Syntax Rules, its keys
//! added by ALTER TABLE, written by two transactions at once, of
//! temporary tables, described, and found again after a crash.

use std::sync::Arc;

use turborel::db::{Database, OpenOptions, Session};
use turborel::disk::io::{FileSystem, OsFileSystem};
use turborel::error::{Code, SqlError};

use crate::common::{Scratch, after_crash};

fn query(s: &Session, sql: &str) -> Vec<String> {
    let mut cursor = s.open_cursor(sql).unwrap();
    let mut out = Vec::new();
    while let Some(row) = cursor.fetch().unwrap() {
        out.push(row.iter().map(|v| v.to_string().trim_end().to_owned()).collect::<Vec<_>>().join(","));
    }
    out
}

/// The SQLCODE class of a refusal.
fn refused<T>(result: Result<T, SqlError>) -> Option<Code> {
    result.err().map(|e| e.code)
}

/// Schema A: T (K INTEGER, A INTEGER, B INTEGER, C INTEGER, D INTEGER),
/// of a key of NULLS NOT DISTINCT on A, one on (B, C), one of NULLS
/// DISTINCT written on D, and one of no treatment written on K.
fn keyed() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (K INTEGER, A INTEGER, B INTEGER, C INTEGER, D INTEGER,
                           CONSTRAINT UA UNIQUE NULLS NOT DISTINCT (A), CONSTRAINT UBC UNIQUE NULLS NOT DISTINCT (B, C),
                           CONSTRAINT UD UNIQUE NULLS DISTINCT (D), CONSTRAINT UK UNIQUE (K))",
    )
    .unwrap();
    db
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.25.3.2, §11.7 GR3] a key
/// of NULLS NOT DISTINCT takes a null as a value: a second row of a null
/// there is refused (SQLCODE -201, 23000), by INSERT and by UPDATE; of two
/// columns, a row of a null in one is a duplicate only of a row of the
/// same value in the other, and two rows all null are duplicates (IA201).
/// NULLS DISTINCT, written or not (ID106), lets any number of rows have a
/// null. MC/DC of a key's check skipping a key with a null: skipped of
/// NULLS DISTINCT (D, K), not of NULLS NOT DISTINCT (A), a key of no null
/// checked either way (A 1 twice, K 1 twice).
#[test]
fn a_null_is_a_value_of_nulls_not_distinct_11_7() {
    let db = keyed();
    let a = db.session("A");
    let code = |sql: &str| refused(a.run(sql));
    a.run("INSERT INTO T (K, A, B, C, D) VALUES (1, NULL, 1, NULL, NULL), (NULL, 1, 2, NULL, NULL), (NULL, 2, NULL, NULL, 1)").unwrap();
    assert_eq!(code("INSERT INTO T (A, B) VALUES (NULL, 10)"), Some(Code::Constraint), "a second null of A");
    assert_eq!(code("INSERT INTO T (A, B) VALUES (1, 11)"), Some(Code::Constraint), "a second 1 of A");
    assert_eq!(code("INSERT INTO T (A, B) VALUES (3, 1)"), Some(Code::Constraint), "(1, NULL) again");
    assert_eq!(code("INSERT INTO T (A, B, C) VALUES (4, NULL, NULL)"), Some(Code::Constraint), "all null again");
    assert_eq!(code("INSERT INTO T (K, A, B) VALUES (1, 5, 5)"), Some(Code::Constraint), "a second 1 of K");
    a.run("INSERT INTO T (A, B) VALUES (6, 3)").unwrap();
    a.run("INSERT INTO T (A, C) VALUES (7, 1)").unwrap();
    assert_eq!(query(&a, "SELECT COUNT(*) FROM T WHERE K IS NULL AND D IS NULL"), ["3"], "nulls of K and D, distinct");
    assert_eq!(code("UPDATE T SET A = NULL WHERE A = 7"), Some(Code::Constraint), "a null set where one is");
    assert_eq!(code("UPDATE T SET B = NULL, C = NULL WHERE A = 6"), Some(Code::Constraint), "all null set");
    a.run("UPDATE T SET A = 8 WHERE A IS NULL").unwrap();
    a.run("UPDATE T SET A = NULL WHERE A = 7").unwrap();
    a.run("DELETE FROM T WHERE A IS NULL").unwrap();
    a.run("INSERT INTO T (A, B) VALUES (NULL, 12)").unwrap();
    assert_eq!(query(&a, "SELECT COUNT(*) FROM T WHERE A IS NULL"), ["1"], "the statements refused left no row");
    a.commit().unwrap();
    assert_eq!(query(&a, "SELECT A, B, C FROM T WHERE A IS NULL"), ["NULL,12,NULL"]);
    assert_eq!(query(&a, "SELECT A, B, C FROM T WHERE A = 8"), ["8,1,NULL"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.7 GR3, ISO/IEC 9075:1992
/// §10.6] a deferred key of NULLS NOT DISTINCT may hold two nulls until
/// it is checked: the commit refused if they are there (SQLCODE -913),
/// the transaction rolled back, not once one has gone.
#[test]
fn a_deferred_key_of_nulls_not_distinct_11_7() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (A INTEGER, N INTEGER,
           CONSTRAINT U UNIQUE NULLS NOT DISTINCT (A) DEFERRABLE INITIALLY DEFERRED)",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (NULL, 1), (NULL, 2)").unwrap();
    assert_eq!(refused(a.commit()), Some(Code::CommitFailed), "two nulls at commit");
    assert!(query(&a, "SELECT N FROM T").is_empty(), "rolled back");
    a.run("INSERT INTO T VALUES (NULL, 1), (NULL, 2)").unwrap();
    a.run("DELETE FROM T WHERE N = 2").unwrap();
    a.commit().unwrap();
    assert_eq!(query(&a, "SELECT N FROM T"), ["1"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.7 SR5, §11.4] a
/// `<unique null treatment>` is NULLS DISTINCT or NULLS NOT DISTINCT
/// (SQLCODE -102 otherwise), written by UNIQUE in a table constraint, not
/// by PRIMARY KEY (-103), nor in a column constraint (-103); and keys of
/// both treatments on the same columns are two unique constraints on them
/// (-103) [§11.7 SR6c]. MC/DC of PRIMARY KEY's refusal: PRIMARY KEY with a
/// treatment refused, without one taken; UNIQUE with one taken.
#[test]
fn null_treatment_rules_11_7() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE OK (K INTEGER)").unwrap();
    let a = db.session("A");
    let code = |sql: &str| refused(a.run(sql));
    assert_eq!(code("CREATE TABLE P (K INTEGER, PRIMARY KEY NULLS NOT DISTINCT (K))"), Some(Code::SyntaxRule), "PRIMARY KEY");
    assert_eq!(code("CREATE TABLE P (K INTEGER, PRIMARY KEY NULLS DISTINCT (K))"), Some(Code::SyntaxRule), "PRIMARY KEY");
    assert_eq!(code("CREATE TABLE P (K INTEGER UNIQUE NULLS NOT DISTINCT)"), Some(Code::SyntaxRule), "a column constraint");
    assert_eq!(code("CREATE TABLE P (K INTEGER, UNIQUE NULLS (K))"), Some(Code::Syntax), "no DISTINCT");
    assert_eq!(code("CREATE TABLE P (K INTEGER, UNIQUE NULLS NOT (K))"), Some(Code::Syntax), "no DISTINCT after NOT");
    assert_eq!(
        code("CREATE TABLE P (K INTEGER, UNIQUE NULLS DISTINCT (K), UNIQUE NULLS NOT DISTINCT (K))"),
        Some(Code::SyntaxRule),
        "two keys of K"
    );
    a.run("CREATE TABLE P (K INTEGER, PRIMARY KEY (K))").unwrap();
    a.run("CREATE TABLE Q (K INTEGER, NULLS INTEGER, UNIQUE NULLS NOT DISTINCT (NULLS))").unwrap();
    a.run("INSERT INTO Q VALUES (1, NULL)").unwrap();
    assert_eq!(code("INSERT INTO Q VALUES (2, NULL)"), Some(Code::Constraint), "NULLS, a <non-reserved word>, names a column");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.7, §11.18 GR1] ALTER
/// TABLE adds a key of NULLS NOT DISTINCT only if the rows hold no two
/// nulls there (SQLCODE -201), then holds it of every row written; ADD
/// COLUMN, which writes every row again, and DROP COLUMN of a column before
/// the key's, which numbers its columns again, keep it; DROP CONSTRAINT
/// takes it away.
#[test]
fn a_key_of_nulls_not_distinct_added_11_7() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (P INTEGER, A INTEGER, N INTEGER)").unwrap();
    let a = db.session("A");
    let insert = |values: &str| a.run(&format!("INSERT INTO T (A, N) VALUES {values}"));
    insert("(NULL, 1), (NULL, 2), (1, 3)").unwrap();
    a.commit().unwrap();
    assert_eq!(refused(a.run("ALTER TABLE T ADD CONSTRAINT U UNIQUE NULLS NOT DISTINCT (A)")), Some(Code::Constraint), "two nulls");
    a.rollback().unwrap();
    a.run("ALTER TABLE T ADD CONSTRAINT U UNIQUE (A)").unwrap();
    a.run("ALTER TABLE T DROP CONSTRAINT U RESTRICT").unwrap();
    a.run("DELETE FROM T WHERE N = 2").unwrap();
    a.run("ALTER TABLE T ADD CONSTRAINT U UNIQUE NULLS NOT DISTINCT (A)").unwrap();
    assert_eq!(refused(insert("(NULL, 4)")), Some(Code::Constraint), "in its transaction");
    a.commit().unwrap();
    assert_eq!(refused(insert("(NULL, 4)")), Some(Code::Constraint), "once committed");
    a.run("ALTER TABLE T ADD COLUMN X INTEGER").unwrap();
    assert_eq!(refused(insert("(NULL, 4)")), Some(Code::Constraint), "after ADD COLUMN");
    a.run("ALTER TABLE T DROP COLUMN P RESTRICT").unwrap();
    a.commit().unwrap();
    assert_eq!(refused(insert("(NULL, 4)")), Some(Code::Constraint), "after DROP COLUMN");
    a.run("ALTER TABLE T DROP CONSTRAINT U RESTRICT").unwrap();
    insert("(NULL, 4)").unwrap();
    a.commit().unwrap();
    assert_eq!(query(&a, "SELECT COUNT(*) FROM T WHERE A IS NULL"), ["2"]);
}

/// Verifies: [DESIGN] RULES.md D29, D66 two transactions each writing a null
/// to a key of NULLS NOT DISTINCT: the second is refused at once (SQLCODE
/// -911), as for any value, and writes it once the first rolls back.
#[test]
fn nulls_written_at_once_11_7() {
    let db = keyed();
    let (a, b) = (db.session("A"), db.session("A"));
    a.run("INSERT INTO T (A) VALUES (NULL)").unwrap();
    assert_eq!(refused(b.run("INSERT INTO T (A) VALUES (NULL)")), Some(Code::Conflict), "the other's null in doubt");
    b.rollback().unwrap();
    a.rollback().unwrap();
    b.run("INSERT INTO T (A) VALUES (NULL)").unwrap();
    b.commit().unwrap();
    assert_eq!(refused(a.run("INSERT INTO T (A) VALUES (NULL)")), Some(Code::Constraint), "the other's null committed");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.7, ISO/IEC 9075:1992
/// §4.9] a global temporary table's key of NULLS NOT DISTINCT, and a
/// declared local temporary table's, each of the session's rows alone:
/// a second null of the session refused (SQLCODE -201), another session's
/// not.
#[test]
fn temporary_keys_of_nulls_not_distinct_11_7() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE GLOBAL TEMPORARY TABLE G (A INTEGER, UNIQUE NULLS NOT DISTINCT (A))").unwrap();
    let (a, b) = (db.session("A"), db.session("A"));
    a.run("INSERT INTO G VALUES (NULL)").unwrap();
    assert_eq!(refused(a.run("INSERT INTO G VALUES (NULL)")), Some(Code::Constraint), "of G");
    b.run("INSERT INTO G VALUES (NULL)").unwrap();
    a.run("DECLARE LOCAL TEMPORARY TABLE MODULE.D (A INTEGER, UNIQUE NULLS NOT DISTINCT (A))").unwrap();
    a.run("INSERT INTO MODULE.D VALUES (NULL)").unwrap();
    assert_eq!(refused(a.run("INSERT INTO MODULE.D VALUES (NULL)")), Some(Code::Constraint), "of D");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.11 GR2] UNIQUE NULLS NOT
/// DISTINCT is false of rows equal but for a null being equal, two rows
/// all null among them (IA201); UNIQUE NULLS DISTINCT and UNIQUE alone
/// (ID106) true of them, false of rows equal and of no null.
#[test]
fn unique_predicate_null_treatment_8_11() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (G INTEGER, A INTEGER, B INTEGER) CREATE TABLE ONE (X INTEGER)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO ONE VALUES (0)").unwrap();
    a.run("INSERT INTO T VALUES (1, 1, NULL), (1, 1, NULL), (2, NULL, NULL), (2, NULL, NULL), (3, 1, 2), (3, 1, 2), (4, 1, NULL), (4, 2, NULL)")
        .unwrap();
    let truth = |treatment: &str, g: i32| {
        let sql = format!("SELECT COUNT(*) FROM ONE WHERE UNIQUE {treatment} (SELECT A, B FROM T WHERE G = {g})");
        query(&a, &sql) == ["1"]
    };
    for (g, distinct, not_distinct) in [(1, true, false), (2, true, false), (3, false, false), (4, true, true)] {
        assert_eq!(truth("", g), distinct, "UNIQUE of group {g}");
        assert_eq!(truth("NULLS DISTINCT", g), distinct, "NULLS DISTINCT of group {g}");
        assert_eq!(truth("NULLS NOT DISTINCT", g), not_distinct, "NULLS NOT DISTINCT of group {g}");
    }
    let correlated = "SELECT G FROM T AS O WHERE UNIQUE NULLS NOT DISTINCT (SELECT A, B FROM T WHERE G = O.G) GROUP BY G";
    assert_eq!(query(&a, correlated), ["4"], "correlated");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.7 GR2d] TABLE_CONSTRAINTS
/// gives NULLS_DISTINCT, as ISO/IEC 9075-11 names it: YES of a UNIQUE
/// constraint of NULLS DISTINCT, written or not, NO of NULLS NOT DISTINCT,
/// null of a PRIMARY KEY and of a constraint of another kind.
#[test]
fn null_treatment_described_11_7() {
    let db = keyed();
    let a = db.session("A");
    a.run("CREATE TABLE P (K INTEGER PRIMARY KEY, R INTEGER REFERENCES P, CONSTRAINT C CHECK (K > 0))").unwrap();
    a.commit().unwrap();
    let described = query(
        &a,
        "SELECT CONSTRAINT_NAME, CONSTRAINT_TYPE, NULLS_DISTINCT FROM INFORMATION_SCHEMA.TABLE_CONSTRAINTS
          WHERE CONSTRAINT_SCHEMA = 'A' AND CONSTRAINT_TYPE <> 'CHECK' OR CONSTRAINT_NAME = 'C' ORDER BY CONSTRAINT_NAME",
    );
    let unnamed: Vec<&String> = described.iter().filter(|r| r.contains("PRIMARY KEY") || r.contains("FOREIGN KEY")).collect();
    assert_eq!(unnamed.len(), 2, "{described:?}");
    assert!(unnamed.iter().all(|r| r.ends_with(",NULL")), "{described:?}");
    let named: Vec<&String> = described.iter().filter(|r| !r.contains("PRIMARY KEY") && !r.contains("FOREIGN KEY")).collect();
    assert_eq!(named, ["C,CHECK,NULL", "UA,UNIQUE,NO", "UBC,UNIQUE,NO", "UD,UNIQUE,YES", "UK,UNIQUE,YES"]);
}

fn opened(fs: &Arc<dyn FileSystem>, dir: &std::path::Path) -> Database {
    let db = Database::new();
    db.open_files(fs, dir, &OpenOptions::default()).unwrap();
    db
}

/// Verifies: [DESIGN] RULES.md D25, D66 a key of NULLS NOT DISTINCT is as
/// committed after a close, a crash, and a crash after a checkpoint taken
/// while a reader still sees a deleted row, and written after it. Its index
/// is made again holding its nulls, one entry a row, so a second is refused
/// (SQLCODE -201), and holding none for the deleted row.
#[test]
fn recovered() {
    for (case, checkpoint, closed) in [("closed", false, true), ("crashed", false, false), ("checkpointed", true, false)] {
        let scratch = Scratch::new(&format!("unique-nulls-{case}"));
        let (fs, dir): (Arc<dyn FileSystem>, _) = (Arc::new(OsFileSystem), scratch.path("db"));
        let before = opened(&fs, &dir);
        before.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (A INTEGER, N INTEGER, UNIQUE NULLS NOT DISTINCT (A, N))").unwrap();
        let a = before.session("A");
        a.run("INSERT INTO T VALUES (NULL, NULL), (NULL, 1), (1, NULL)").unwrap();
        a.commit().unwrap();
        let reader = before.session("A");
        reader.direct("SELECT N FROM T").unwrap();
        a.run("DELETE FROM T WHERE N = 1").unwrap();
        a.run("INSERT INTO T VALUES (NULL, 2)").unwrap();
        a.commit().unwrap();
        if checkpoint {
            before.checkpoint().unwrap();
        }
        reader.rollback().unwrap();
        a.run("INSERT INTO T VALUES (NULL, 3)").unwrap();
        a.commit().unwrap();
        if closed {
            before.close().unwrap();
        }
        let db = opened(&if closed { Arc::clone(&fs) } else { after_crash() }, &dir);
        assert_eq!(db.index_entries("A.T.UNIQUE1").unwrap(), 4, "{case}: one entry a row, the deleted one's gone");
        let a = db.session("A");
        for row in ["(NULL, NULL)", "(NULL, 2)", "(NULL, 3)", "(1, NULL)"] {
            assert_eq!(refused(a.run(&format!("INSERT INTO T VALUES {row}"))), Some(Code::Constraint), "{case}: {row}");
        }
        a.run("INSERT INTO T VALUES (NULL, 1)").unwrap();
        a.rollback().unwrap();
        db.close().unwrap();
        drop((db, before));
    }
}

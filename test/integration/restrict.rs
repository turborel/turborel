//! The referential action RESTRICT [ISO/IEC 9075-2:2023 §11.8, §15.18
//! GR9a iv, GR9b iv, GR10a iv, GR10b iv] (Feature T191): a referenced row
//! deleted, or given a distinct value in a referenced column, while a row
//! matched it before the statement, is a restrict violation (23001).

use turborel::db::{Database, Session};

use crate::common::rows;

/// Schema A and its tables, as `sql` defines them.
fn database(sql: &[&str]) -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    ok(&db.session("A"), sql);
    db
}

/// Runs each of `sql` in `s`, which must succeed, and commits.
fn ok(s: &Session, sql: &[&str]) {
    for one in sql {
        s.run(one).unwrap_or_else(|e| panic!("{one}: {e}"));
    }
    s.commit().unwrap();
}

/// The SQLSTATE of `sql` in `s`, rolled back if it fails.
fn state(s: &Session, sql: &str) -> String {
    match s.run(sql) {
        Ok(done) => done.sqlstate().as_str().to_owned(),
        Err(e) => {
            s.rollback().unwrap();
            e.sqlstate().as_str().to_owned()
        }
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §15.18 GR9a iv, GR10a iv] ON DELETE and
/// ON UPDATE RESTRICT refuse a referenced row deleted, or given a distinct
/// value in a referenced column, while a row matches it (23001); a row no
/// row matches is deleted, a column not referenced updated, and a
/// referenced value given again as it was.
#[test]
fn restrict_refuses_a_referenced_row_a_row_matches_15_18() {
    let db = database(&[
        "CREATE TABLE P (K INTEGER PRIMARY KEY, V INTEGER)",
        "CREATE TABLE C (K INTEGER REFERENCES P ON DELETE RESTRICT ON UPDATE RESTRICT)",
        "INSERT INTO P VALUES (1, 0), (2, 0)",
        "INSERT INTO C VALUES (1)",
    ]);
    let s = db.session("A");
    assert_eq!(state(&s, "DELETE FROM P WHERE K = 1"), "23001");
    assert_eq!(state(&s, "UPDATE P SET K = 3 WHERE K = 1"), "23001");
    for sql in ["UPDATE P SET V = 9 WHERE K = 1", "UPDATE P SET K = 1 WHERE K = 1", "DELETE FROM P WHERE K = 2"] {
        assert_eq!(state(&s, sql), "00000", "{sql}");
    }
    s.commit().unwrap();
    assert_eq!(rows(&s, "SELECT K, V FROM P").unwrap(), ["1,9"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §15.18 GR7, GR9a iv] the rows that match
/// a referenced row are those that did before the statement: a statement
/// that deletes a row and the rows that reference it, of a table that
/// references itself, is refused under RESTRICT (23001), and runs under
/// NO ACTION, which judges the rows as the statement ends.
#[test]
fn restrict_judges_the_rows_as_they_were_before_the_statement_15_18() {
    let db = database(&[
        "CREATE TABLE STAFF (ID INTEGER PRIMARY KEY, BOSS INTEGER, CONSTRAINT LED FOREIGN KEY (BOSS) REFERENCES STAFF ON DELETE RESTRICT)",
        "CREATE TABLE TEAM (ID INTEGER PRIMARY KEY, BOSS INTEGER, FOREIGN KEY (BOSS) REFERENCES TEAM)",
        "INSERT INTO STAFF VALUES (1, NULL), (2, 1)",
        "INSERT INTO TEAM VALUES (1, NULL), (2, 1)",
    ]);
    let s = db.session("A");
    assert_eq!(state(&s, "DELETE FROM STAFF"), "23001");
    assert_eq!(state(&s, "DELETE FROM TEAM"), "00000");
    s.commit().unwrap();
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM STAFF").unwrap(), ["2"]);
    // A row no row references is deleted.
    assert_eq!(state(&s, "DELETE FROM STAFF WHERE ID = 2"), "00000");
    s.commit().unwrap();
    assert_eq!(rows(&s, "SELECT ID FROM STAFF").unwrap(), ["1"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §15.18 GR9b iv, GR10b iv] with MATCH
/// PARTIAL, RESTRICT refuses a referenced row deleted while a row matches
/// it alone (23001): one that matches another row too is kept.
#[test]
fn restrict_of_match_partial_refuses_a_unique_matching_row_15_18() {
    let db = database(&[
        "CREATE TABLE P (A INTEGER, B INTEGER, UNIQUE (A, B))",
        "CREATE TABLE C (A INTEGER, B INTEGER, FOREIGN KEY (A, B) REFERENCES P (A, B) MATCH PARTIAL ON DELETE RESTRICT)",
        "INSERT INTO P VALUES (1, 1), (1, 2), (2, 1)",
        "INSERT INTO C VALUES (1, NULL), (2, NULL)",
    ]);
    let s = db.session("A");
    assert_eq!(state(&s, "DELETE FROM P WHERE A = 1 AND B = 1"), "00000", "(1, NULL) matches (1, 2) too");
    s.commit().unwrap();
    assert_eq!(state(&s, "DELETE FROM P WHERE A = 2"), "23001");
    assert_eq!(state(&s, "DELETE FROM P WHERE A = 1"), "23001");
}

/// Verifies: [ISO/IEC 9075-2:2023 §15.18 GR9a iv; ISO/IEC 9075:1992
/// §10.6] RESTRICT is no check deferred: refused as the statement ends,
/// of a foreign key INITIALLY DEFERRED too; and REFERENTIAL_CONSTRAINTS
/// shows its rules as RESTRICT.
#[test]
fn restrict_is_refused_at_once_and_shown_15_18() {
    let db = database(&[
        "CREATE TABLE P (K INTEGER PRIMARY KEY)",
        "CREATE TABLE C (K INTEGER, CONSTRAINT HELD FOREIGN KEY (K) REFERENCES P ON UPDATE RESTRICT ON DELETE RESTRICT \
         DEFERRABLE INITIALLY DEFERRED)",
        "INSERT INTO P VALUES (1)",
        "INSERT INTO C VALUES (1)",
    ]);
    let s = db.session("A");
    assert_eq!(state(&s, "DELETE FROM P"), "23001");
    let shown = "SELECT UPDATE_RULE, DELETE_RULE FROM INFORMATION_SCHEMA.REFERENTIAL_CONSTRAINTS WHERE CONSTRAINT_NAME = 'HELD'";
    assert_eq!(rows(&s, shown).unwrap(), ["RESTRICT,RESTRICT"]);
}

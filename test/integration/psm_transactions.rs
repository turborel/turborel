//! COMMIT and ROLLBACK in an SQL control statement NOT ATOMIC executed
//! directly [ISO/IEC 9075-4:2006 draft §4.10.4; ISO/IEC 9075-2:2023 §17.7,
//! §17.8]: the transaction ends within the statement, and the statements
//! after run in a new one.

use turborel::db::{Database, Session};
use turborel::error::Code;

/// Schema A: T (K), row 1, committed.
fn defined() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER)").unwrap();
    db.session("A").run("INSERT INTO T VALUES (1)").unwrap();
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

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §4.10.4; ISO/IEC 9075-2:2023 §17.7 GR6c, GR9, §17.8 GR3]
/// A compound statement run directly commits what came before its COMMIT
/// and undoes, at ROLLBACK, only what came after; a COMMIT in a loop
/// commits each turn; a COMMIT last leaves no transaction active, so SET
/// TRANSACTION may follow.
#[test]
fn commit_and_rollback_end_the_transaction_within_4_10_4() {
    let db = defined();
    let a = db.session("A");
    a.run("BEGIN INSERT INTO T VALUES (2); COMMIT; INSERT INTO T VALUES (3); ROLLBACK; INSERT INTO T VALUES (4); END").unwrap();
    assert_eq!(rows(&a, "SELECT K FROM T ORDER BY K"), ["1", "2", "4"]);
    a.rollback().unwrap();
    assert_eq!(rows(&db.session("A"), "SELECT K FROM T ORDER BY K"), ["1", "2"]);
    a.run(
        "BEGIN DECLARE N INTEGER DEFAULT 0; L: LOOP SET N = N + 1; INSERT INTO T VALUES (10 + N); COMMIT WORK; \
         IF N = 2 THEN LEAVE L; END IF; END LOOP L; END",
    )
    .unwrap();
    a.rollback().unwrap();
    assert_eq!(rows(&a, "SELECT K FROM T WHERE K > 10 ORDER BY K"), ["11", "12"]);
    a.commit().unwrap();
    a.run("BEGIN INSERT INTO T VALUES (20); COMMIT; END").unwrap();
    assert_eq!(a.transaction(), None);
    a.run("SET TRANSACTION READ ONLY").unwrap();
    assert_eq!(rows(&db.session("A"), "SELECT K FROM T WHERE K = 20"), ["20"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §17.7 GR3; ISO/IEC 9075-4:2006 draft §12.2]
/// COMMIT closes a local cursor open before it, so a FETCH after fails
/// (SQLCODE -501, SQLSTATE 24000), and the cursor opens again in the new
/// transaction.
#[test]
fn commit_closes_the_local_cursors_17_7() {
    let db = defined();
    let a = db.session("A");
    let e = a.run("BEGIN DECLARE V INTEGER; DECLARE C CURSOR FOR SELECT K FROM T; OPEN C; COMMIT; FETCH C INTO V; END").unwrap_err();
    assert_eq!((e.sqlcode(), e.sqlstate().as_str()), (-501, "24000"), "{e:?}");
    a.run(
        "BEGIN DECLARE V INTEGER; DECLARE C CURSOR FOR SELECT K FROM T; OPEN C; COMMIT; \
         OPEN C; FETCH C INTO V; INSERT INTO T VALUES (V + 1); CLOSE C; END",
    )
    .unwrap();
    assert_eq!(rows(&a, "SELECT K FROM T ORDER BY K"), ["1", "2"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §17.7 GR2; ISO/IEC 9075-4:2006 draft §14.2 GR3]
/// A COMMIT in the action of a handler that handles a condition raised in
/// an ATOMIC compound statement within its own runs while that statement
/// does, in an atomic execution context: it raises invalid transaction
/// termination (SQLSTATE 2D000), and the ATOMIC statement is undone.
#[test]
fn commit_in_an_atomic_context_is_invalid_17_7() {
    let db = defined();
    let a = db.session("A");
    let e = a
        .run(
            "BEGIN DECLARE CONTINUE HANDLER FOR SQLEXCEPTION COMMIT; \
             BEGIN ATOMIC INSERT INTO T VALUES (9); SIGNAL SQLSTATE '75001'; END; END",
        )
        .unwrap_err();
    assert_eq!((e.code, e.sqlstate().as_str()), (Code::TransactionState, "2D000"), "{e:?}");
    assert_eq!(rows(&a, "SELECT K FROM T ORDER BY K"), ["1"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §14.1 GR3; ISO/IEC 9075:1992 §4.22.6]
/// A statement of a compound statement run directly sees the table a
/// statement before it created, and what it committed, where before it
/// read the state the compound statement began with (regression).
#[test]
fn a_statement_sees_the_definitions_before_it_14_1() {
    let db = defined();
    let a = db.session("A");
    a.run("BEGIN CREATE TABLE X (V INTEGER); INSERT INTO X VALUES (1); COMMIT; INSERT INTO X VALUES (2); END").unwrap();
    a.commit().unwrap();
    assert_eq!(rows(&db.session("A"), "SELECT V FROM X ORDER BY V"), ["1", "2"]);
}

//! MERGE beside another writer [ISO/IEC 9075-2:2023 §14.12 GR4, GR6;
//! ISO/IEC 9075:1992 §4.28; RULES.md D28, D29]. At READ COMMITTED a target
//! row another transaction changed after the statement's snapshot is
//! checked again, with the same source row, against the ON condition and
//! its clause's condition, and left alone if they no longer hold, never
//! inserted instead; a key another transaction inserts is in doubt, then
//! a unique violation to a snapshot that does not see it.

use std::thread;

use turborel::db::{Completion, Database, Session};
use turborel::error::{Code, SqlError};
use turborel::txn::LockWait;

use crate::common::{failure, rows};

/// A database of `T (K UNIQUE, V)` holding (1, 10) and (2, 20) and `S (K,
/// V)` holding (1, 5), committed, whose sessions wait.
fn database() -> Database {
    let db = Database::new();
    db.set_lock_wait(LockWait::Wait).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER NOT NULL UNIQUE, V INTEGER) CREATE TABLE S (K INTEGER, V INTEGER)")
        .unwrap();
    let s = db.session("A");
    s.run("INSERT INTO T VALUES (1, 10), (2, 20)").unwrap();
    s.run("INSERT INTO S VALUES (1, 5)").unwrap();
    s.commit().unwrap();
    db
}

/// The rows of T, ordered by K.
fn contents(s: &Session) -> Vec<String> {
    rows(s, "SELECT K, V FROM T ORDER BY K").unwrap()
}

/// Runs the MERGE `sql` in a READ COMMITTED session of `db` on a thread of
/// its own, once `first` has run `change`; waits until it waits, then
/// commits `first`. What the MERGE completed with, and its session.
fn second(db: &Database, (first, change): (&Session, &str), sql: &str) -> (Result<Completion, SqlError>, Session) {
    first.run(change).unwrap();
    let s = db.session("A");
    s.run("SET TRANSACTION ISOLATION LEVEL READ COMMITTED").unwrap();
    assert_eq!(rows(&s, "SELECT V FROM T WHERE K = 2").unwrap(), ["20"], "its transaction begun before the first ends");
    let sql = sql.to_owned();
    let waiting = thread::spawn(move || {
        let done = s.run(&sql);
        (done, s)
    });
    while db.waiting_statements().unwrap() == 0 {
        assert!(!waiting.is_finished(), "the MERGE waits for the first writer");
        thread::yield_now();
    }
    first.commit().unwrap();
    waiting.join().unwrap()
}

/// The MERGE of S into T whose WHEN MATCHED clause is `clause`, S's row
/// inserted if it matches none.
fn merging(clause: &str) -> String {
    format!("MERGE INTO T USING S ON T.K = S.K WHEN MATCHED {clause} WHEN NOT MATCHED THEN INSERT VALUES (S.K, S.V)")
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 GR4, GR6a i 1 A;
/// ISO/IEC 9075:1992 §4.28] RULES.md D29 at READ COMMITTED a MERGE that
/// waits for a row's first writer checks the version it committed again,
/// with the same source row. Still a subject row, it is written from that
/// version, its SET values computed from it, so no update is lost; no
/// longer one, it is left alone and its source row not inserted, the
/// MERGE completing successfully. MC/DC of the check, ON and the clause's
/// condition both true of the committed version: both true, updated from
/// 11 to 111; ON true and the condition (T.V = 10) false, left alone, for
/// UPDATE and DELETE; ON false (K now 7), left alone; the row deleted,
/// left alone.
#[test]
fn a_row_changed_meanwhile_is_checked_again_14_12() {
    let increment = "UPDATE T SET V = V + 1 WHERE K = 1";
    for (change, clause, left) in [
        (increment, "THEN UPDATE SET V = T.V + 100", vec!["1,111", "2,20"]),
        (increment, "AND T.V = 10 THEN UPDATE SET V = T.V + 100", vec!["1,11", "2,20"]),
        (increment, "AND T.V = 10 THEN DELETE", vec!["1,11", "2,20"]),
        ("UPDATE T SET K = 7 WHERE K = 1", "THEN UPDATE SET V = T.V + 100", vec!["2,20", "7,10"]),
        ("DELETE FROM T WHERE K = 1", "THEN UPDATE SET V = T.V + 100", vec!["2,20"]),
    ] {
        let db = database();
        let first = db.session("A");
        let (done, s) = second(&db, (&first, change), &merging(clause));
        assert_eq!(done.map_err(|e| e.code), Ok(Completion::Success), "{change}; {clause}");
        s.commit().unwrap();
        assert_eq!(contents(&first), left, "{change}; {clause}");
    }
}

/// S's row moved to key 3, inserted into T while T has no row 3, else
/// added to it.
const AT_THREE: &str = "MERGE INTO T USING (SELECT K + 2 AS K, V FROM S) AS Z ON T.K = Z.K \
     WHEN MATCHED THEN UPDATE SET V = T.V + Z.V \
     WHEN NOT MATCHED THEN INSERT VALUES (Z.K, Z.V)";

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.12 GR6a ii 8; §4.25.2]
/// RULES.md D29 a key a MERGE inserts that another open transaction has
/// inserted is in doubt (SQLCODE -911); once that one commits, a MERGE of
/// a snapshot from before the commit, not seeing the row, inserts it
/// again and violates the constraint (SQLSTATE 23000); a MERGE at READ
/// COMMITTED after the commit sees the row and updates it.
#[test]
fn a_key_another_transaction_inserts_14_12() {
    let db = database();
    let (a, b) = (db.session("A"), db.session("A"));
    b.run("SET LOCKS.WAIT = 'OFF'").unwrap();
    b.run("SET TRANSACTION ISOLATION LEVEL READ COMMITTED").unwrap();
    assert_eq!(a.run(AT_THREE).unwrap(), Completion::Success);
    assert_eq!(failure(b.run(AT_THREE)).0, Code::Conflict, "key 3 in doubt");
    b.rollback().unwrap();
    b.run("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ").unwrap();
    assert_eq!(contents(&b), ["1,10", "2,20"], "its snapshot");
    a.commit().unwrap();
    assert_eq!(failure(b.run(AT_THREE)), (Code::Constraint, "23000".to_owned()), "inserted after its snapshot");
    b.rollback().unwrap();
    b.run("SET TRANSACTION ISOLATION LEVEL READ COMMITTED").unwrap();
    assert_eq!(b.run(AT_THREE).unwrap(), Completion::Success);
    b.commit().unwrap();
    assert_eq!(contents(&a), ["1,10", "2,20", "3,10"], "row 3 matched and updated");
}

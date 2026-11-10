//! A row's second writer under LOCKS.WAIT ON [ISO/IEC 9075:1992 §4.28;
//! RULES.md D28, D29]: it waits for the first to end; at READ COMMITTED it
//! then writes the version the first committed if its search condition
//! still holds, and at REPEATABLE READ or SERIALIZABLE it is refused unless
//! the first rolled back. A wait that would deadlock, or lasts past
//! LOCKS.TIMEOUT_MS, is refused.

use std::thread;

use turborel::db::{Completion, Database, Session};
use turborel::error::{Code, SqlError};
use turborel::txn::LockWait;

use crate::common::rows;

/// A database of `T (K, V)` holding (1, 10) and (2, 20), committed, whose
/// sessions wait.
fn database() -> Database {
    let db = Database::new();
    db.set_lock_wait(LockWait::Wait).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER NOT NULL UNIQUE, V INTEGER)").unwrap();
    let s = db.session("A");
    s.run("INSERT INTO T VALUES (1, 10), (2, 20)").unwrap();
    s.commit().unwrap();
    db
}

/// The value of row `k` of `T`, as `s` reads it.
fn value(s: &Session, k: i32) -> String {
    rows(s, &format!("SELECT V FROM T WHERE K = {k}")).unwrap().concat()
}

/// Runs `sql` in a session of `db` at `level` on a thread of its own, once
/// `first` has run `change`; waits until it waits, then ends `first` as
/// `commit` says. What the statement completed with, and the session.
fn second(
    db: &Database,
    (first, change, commit): (&Session, &str, bool),
    (level, sql): (&str, &str),
) -> (Result<Completion, SqlError>, Session) {
    first.run(change).unwrap();
    let s = db.session("A");
    s.run(&format!("SET TRANSACTION ISOLATION LEVEL {level}")).unwrap();
    assert_eq!(value(&s, 2), "20", "its snapshot taken before the first ends");
    let sql = sql.to_owned();
    let waiting = thread::spawn(move || {
        let done = s.run(&sql);
        (done, s)
    });
    while db.waiting_statements().unwrap() == 0 {
        assert!(!waiting.is_finished(), "the second writer waits for the first");
        thread::yield_now();
    }
    if commit {
        first.commit().unwrap();
    } else {
        first.rollback().unwrap();
    }
    waiting.join().unwrap()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.28] RULES.md D28, D29: at READ
/// COMMITTED a row's second writer waits for the first; once it commits,
/// the second writes the version it committed, its SET values computed
/// from that version, so no update is lost.
#[test]
fn a_second_writer_waits_and_writes_the_committed_row_4_28() {
    let db = database();
    let first = db.session("A");
    let (done, s) =
        second(&db, (&first, "UPDATE T SET V = V + 1 WHERE K = 1", true), ("READ COMMITTED", "UPDATE T SET V = V + 100 WHERE K = 1"));
    assert_eq!(done.map_err(|e| e.code), Ok(Completion::Success));
    s.commit().unwrap();
    assert_eq!(value(&first, 1), "111");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.28] RULES.md D29: a waiting
/// writer leaves a row whose committed version its search condition no
/// longer holds of, or that the first deleted (SQLCODE 100); a waiting
/// DELETE likewise.
#[test]
fn a_waiting_writer_skips_a_row_no_longer_matching_4_28() {
    for (change, sql, left) in [
        ("UPDATE T SET V = V + 1 WHERE K = 1", "UPDATE T SET V = V + 100 WHERE K = 1 AND V = 10", "11"),
        ("UPDATE T SET V = V + 1 WHERE K = 1", "DELETE FROM T WHERE K = 1 AND V = 10", "11"),
        ("DELETE FROM T WHERE K = 1", "UPDATE T SET V = V + 100 WHERE K = 1", ""),
    ] {
        let db = database();
        let first = db.session("A");
        let (done, s) = second(&db, (&first, change, true), ("READ COMMITTED", sql));
        assert_eq!(done.map_err(|e| e.code), Ok(Completion::NoData), "{sql}");
        s.commit().unwrap();
        assert_eq!(value(&first, 1), left, "{sql}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.28] RULES.md D29: at every
/// level a waiting writer whose first rolls back writes the row as it
/// found it; at REPEATABLE READ and SERIALIZABLE one whose first commits
/// is refused (SQLCODE -911), its transaction going on.
#[test]
fn one_snapshot_waits_then_is_refused_if_the_first_commits_4_28() {
    for level in ["READ COMMITTED", "REPEATABLE READ", "SERIALIZABLE"] {
        let db = database();
        let first = db.session("A");
        let (done, s) = second(&db, (&first, "UPDATE T SET V = V + 1 WHERE K = 1", false), (level, "UPDATE T SET V = V + 100 WHERE K = 1"));
        assert_eq!(done.map_err(|e| e.code), Ok(Completion::Success), "{level}: rolled back");
        s.commit().unwrap();
        assert_eq!(value(&first, 1), "110", "{level}");
    }
    for level in ["REPEATABLE READ", "SERIALIZABLE"] {
        let db = database();
        let first = db.session("A");
        let (done, s) = second(&db, (&first, "UPDATE T SET V = V + 1 WHERE K = 1", true), (level, "UPDATE T SET V = V + 100 WHERE K = 1"));
        assert_eq!(done.map_err(|e| e.code), Err(Code::Conflict), "{level}: committed after its snapshot");
        assert_eq!(value(&s, 2), "20", "{level}: its transaction goes on");
        s.rollback().unwrap();
        assert_eq!(value(&first, 1), "11", "{level}");
    }
}

/// Verifies: [DESIGN] RULES.md D28: two writers each waiting for a row the
/// other changed would deadlock: the second to wait is refused at once
/// (SQLCODE -911), only its statement undone, and the first then writes.
#[test]
fn writers_whose_rows_cross_are_refused_the_deadlock() {
    let db = database();
    let (one, two) = (db.session("A"), db.session("A"));
    // A deadlock not found ends refused for its timeout, not hung.
    for s in [&one, &two] {
        s.run("SET LOCKS.TIMEOUT_MS = 5000").unwrap();
    }
    one.run("UPDATE T SET V = 11 WHERE K = 1").unwrap();
    two.run("UPDATE T SET V = 21 WHERE K = 2").unwrap();
    let waiting = thread::spawn(move || {
        let done = one.run("UPDATE T SET V = 12 WHERE K = 2").map_err(|e| e.code);
        (done, one)
    });
    while db.waiting_statements().unwrap() == 0 {
        assert!(!waiting.is_finished(), "the first waits");
        thread::yield_now();
    }
    let refused = two.run("UPDATE T SET V = 22 WHERE K = 1").unwrap_err();
    assert_eq!(refused.code, Code::Conflict);
    assert!(refused.msg.contains("deadlock"), "{}", refused.msg);
    two.rollback().unwrap();
    let (done, one) = waiting.join().unwrap();
    assert_eq!(done, Ok(Completion::Success));
    one.commit().unwrap();
    assert_eq!((value(&two, 1), value(&two, 2)), ("11".to_owned(), "12".to_owned()));
}

/// Verifies: [DESIGN] RULES.md D28: a wait for a row past LOCKS.TIMEOUT_MS is
/// refused (SQLCODE -911), saying so; under LOCKS.WAIT OFF the second
/// writer is refused at once.
#[test]
fn a_row_wait_past_its_timeout_is_refused() {
    let db = database();
    let first = db.session("A");
    first.run("UPDATE T SET V = 11 WHERE K = 1").unwrap();
    let waiter = db.session("A");
    waiter.run("SET LOCKS.TIMEOUT_MS = 30").unwrap();
    let failed = waiter.run("UPDATE T SET V = 12 WHERE K = 1").unwrap_err();
    assert_eq!(failed.code, Code::Conflict);
    assert!(failed.msg.contains("LOCKS.TIMEOUT_MS"), "{}", failed.msg);
    waiter.run("SET LOCKS.WAIT = 'OFF'").unwrap();
    waiter.rollback().unwrap();
    assert_eq!(waiter.run("UPDATE T SET V = 12 WHERE K = 1").map_err(|e| e.code), Err(Code::Conflict), "no wait");
}

//! READ COMMITTED writers of a row in demand [ISO/IEC 9075:1992 §4.28;
//! RULES.md D454]: each waits for the row's writer and writes the version
//! it committed, however many writers come before it, ending only on the
//! lock timeout or a deadlock; and a statement of a procedure's body reads
//! at a snapshot taken as it begins, so a wait in one statement leaves the
//! next reading what was committed meanwhile.

use std::thread;
use std::time::Duration;

use turborel::db::Database;
use turborel::txn::LockWait;
use turborel::value::Value;

use crate::common::rows;

/// How many sessions write the one row at once.
const SESSIONS: usize = 32;

/// How many increments each session commits.
const INCREMENTS: usize = 20;

/// How long each writer holds the row before it commits, so the other
/// sessions line up behind it.
const HOLD: Duration = Duration::from_millis(2);

/// A database of `T (K, V)` holding (1, 0) and (2, 20), and `L (X)`,
/// empty, committed, whose sessions wait.
fn database() -> Database {
    let db = Database::new();
    db.set_lock_wait(LockWait::Wait).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER NOT NULL UNIQUE, V INTEGER) CREATE TABLE L (X INTEGER)").unwrap();
    let s = db.session("A");
    s.run("INSERT INTO T VALUES (1, 0), (2, 20)").unwrap();
    s.commit().unwrap();
    db
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.28] RULES.md D454: many
/// READ COMMITTED sessions incrementing one row each wait their turn and
/// write the version committed before them, none refused however many
/// writers it waits behind, and no increment is lost.
#[test]
fn many_writers_of_one_row_lose_nothing_and_are_never_refused_4_28() {
    let db = database();
    let writers: Vec<_> = (0..SESSIONS)
        .map(|_| {
            let s = db.session("A");
            thread::spawn(move || {
                for _ in 0..INCREMENTS {
                    s.run("SET TRANSACTION ISOLATION LEVEL READ COMMITTED").unwrap();
                    if let Err(e) = s.run("UPDATE T SET V = V + 1 WHERE K = 1") {
                        return Err(format!("{:?} {}", e.code, e.msg));
                    }
                    thread::sleep(HOLD);
                    s.commit().map_err(|e| e.msg)?;
                }
                Ok(())
            })
        })
        .collect();
    for w in writers {
        assert_eq!(w.join().unwrap(), Ok(()));
    }
    let s = db.session("A");
    let count = SESSIONS.saturating_mul(INCREMENTS);
    assert_eq!(rows(&s, "SELECT V FROM T WHERE K = 1").unwrap().concat(), count.to_string());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.28] RULES.md D454: at
/// READ COMMITTED a procedure's statement after one that waited for a
/// row reads what was committed while it waited.
#[test]
fn a_procedure_statement_after_a_wait_reads_what_was_committed_4_28() {
    let db = database();
    // Its increment waits for the row's writer, not reserved (RULES.md D452).
    db.set_setting("TRANSACTION.ESCROW", &Value::Char("OFF".to_owned())).unwrap();
    let a = db.session("A");
    a.run(
        "CREATE PROCEDURE P () MODIFIES SQL DATA BEGIN DECLARE X INTEGER; \
         UPDATE T SET V = V + 100 WHERE K = 1; SELECT V INTO X FROM T WHERE K = 2; INSERT INTO L VALUES (X); END",
    )
    .unwrap();
    a.commit().unwrap();
    let first = db.session("A");
    first.run("UPDATE T SET V = V + 1 WHERE K = 1").unwrap();
    let s = db.session("A");
    s.run("SET TRANSACTION ISOLATION LEVEL READ COMMITTED").unwrap();
    let waiting = thread::spawn(move || {
        let done = s.run("CALL P()").map(|_| ());
        (done, s)
    });
    while db.waiting_statements().unwrap() == 0 {
        assert!(!waiting.is_finished(), "the procedure waits for the first writer");
        thread::yield_now();
    }
    first.run("UPDATE T SET V = 21 WHERE K = 2").unwrap();
    first.commit().unwrap();
    let (done, s) = waiting.join().unwrap();
    assert_eq!(done.map_err(|e| e.code), Ok(()));
    s.commit().unwrap();
    assert_eq!(rows(&first, "SELECT X FROM L").unwrap().concat(), "21");
    assert_eq!(rows(&first, "SELECT V FROM T WHERE K = 1").unwrap().concat(), "101");
}

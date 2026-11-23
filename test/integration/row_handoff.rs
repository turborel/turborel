//! \[DESIGN\] A row in demand passed from writer to writer (RULES.md D456,
//! D743, D851): once its writer ends, a transaction that is running takes
//! it, and a waiter overtaken so goes first in the next line. Many
//! sessions taking numbers from one row, at READ COMMITTED and at
//! SERIALIZABLE, each committing every take or retrying it, take each
//! number once with no gap, and every one that tried is served.

use std::thread;

use turborel::db::{Database, Session};
use turborel::txn::LockWait;

use crate::common::rows;

/// Sessions taking numbers at once.
const SESSIONS: usize = 12;
/// The numbers each session takes.
const TAKES: usize = 25;

/// A database whose sessions wait for locks, with `T (ID, N)` holding row
/// 1 of N 0, and `SEEN (N)`, where each take notes its number.
fn database() -> Database {
    let db = Database::new();
    db.set_lock_wait(LockWait::Wait).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (ID INTEGER NOT NULL PRIMARY KEY, N INTEGER) CREATE TABLE SEEN (N INTEGER)")
        .unwrap();
    let s = db.session("A");
    s.run("INSERT INTO T VALUES (1, 0)").unwrap();
    s.commit().unwrap();
    db
}

/// `TAKES` numbers taken by `s` at `level`, each the row's number before
/// its increment, noted and committed, a take refused tried again.
fn taking(s: &Session, level: &str) {
    for _ in 0..TAKES {
        loop {
            let took = s
                .run(&format!("SET TRANSACTION ISOLATION LEVEL {level}"))
                .and_then(|_| s.run("UPDATE T SET N = N + 1 WHERE ID = 1"))
                .and_then(|_| s.run("INSERT INTO SEEN SELECT N - 1 FROM T WHERE ID = 1"))
                .and_then(|_| s.commit());
            if took.is_ok() {
                break;
            }
            s.rollback().unwrap();
        }
    }
}

/// Verifies: [DESIGN] RULES.md D851, D456; [ISO/IEC 9075:1992 §4.28]: at
/// READ COMMITTED and at SERIALIZABLE, sessions taking numbers from one
/// row at once, its writers passing it to whichever runs, lose no update
/// and take every number once.
#[test]
fn a_row_in_demand_gives_each_number_once_at_each_level() {
    for level in ["READ COMMITTED", "SERIALIZABLE"] {
        let db = database();
        let sessions: Vec<_> = (0..SESSIONS)
            .map(|_| {
                let db = db.clone();
                thread::spawn(move || taking(&db.session("A"), level))
            })
            .collect();
        for t in sessions {
            t.join().unwrap();
        }
        let s = db.session("A");
        let all = SESSIONS * TAKES;
        assert_eq!(rows(&s, "SELECT N FROM T").unwrap(), [all.to_string()], "{level}");
        let seen = "SELECT COUNT(*), COUNT(DISTINCT N), MIN(N), MAX(N) FROM SEEN";
        assert_eq!(rows(&s, seen).unwrap(), [format!("{all},{all},0,{}", all - 1)], "{level}");
    }
}

//! \[EXTENSION\] Row locks of FOR UPDATE and FOR SHARE, NOWAIT and SKIP
//! LOCKED (RULES.md D185), and LOCK TABLE (RULES.md D186): taken as a
//! query reads its rows, held to its transaction's end, waited for by
//! writers and other lockers through the one graph of waits.

use std::collections::BTreeSet;
use std::thread;

use turborel::db::{Completion, Database, Session};
use turborel::error::{Code, SqlError};
use turborel::txn::LockWait;

/// A database of `Q (ID, DONE)` holding jobs 1 to `n`, none done,
/// committed, whose sessions wait.
fn queue(n: usize) -> Database {
    let db = Database::new();
    db.set_lock_wait(LockWait::Wait).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE Q (ID INTEGER NOT NULL PRIMARY KEY, DONE INTEGER)").unwrap();
    let s = db.session("A");
    for id in 1..=n {
        s.run(&format!("INSERT INTO Q VALUES ({id}, 0)")).unwrap();
    }
    s.commit().unwrap();
    db
}

/// The rows `sql` gives in `s`, each its values joined by commas.
fn opened(s: &Session, sql: &str) -> Result<Vec<String>, SqlError> {
    let mut c = s.open_cursor(sql)?;
    let mut out = Vec::new();
    while let Some(r) = c.fetch()? {
        out.push(r.iter().map(ToString::to_string).collect::<Vec<_>>().join(","));
    }
    Ok(out)
}

/// Runs `sql` in `s` on a thread of its own, waits until it waits, runs
/// `then`, and gives what it completed with, and the session.
fn waiting_for(db: &Database, s: Session, sql: &str, then: impl FnOnce()) -> (Result<Completion, SqlError>, Session) {
    let sql = sql.to_owned();
    let waiter = thread::spawn(move || (s.run(&sql), s));
    while db.waiting_statements().unwrap() == 0 {
        assert!(!waiter.is_finished(), "it waits");
        thread::yield_now();
    }
    then();
    waiter.join().unwrap()
}

/// Verifies: [EXTENSION] RULES.md D185: a row FOR UPDATE locks is written
/// by another transaction only once the locker ends; a lock NOWAIT asks
/// for is refused at once with SQLSTATE TBL01, the statement undone.
#[test]
fn for_update_holds_a_row_against_writers_and_nowait_d185() {
    let db = queue(3);
    let locker = db.session("A");
    assert_eq!(opened(&locker, "SELECT ID FROM Q WHERE ID = 2 FOR UPDATE").unwrap(), vec!["2"]);
    let other = db.session("A");
    let refused = opened(&other, "SELECT ID FROM Q WHERE ID = 2 FOR UPDATE NOWAIT").unwrap_err();
    assert_eq!((refused.code, refused.sqlstate().as_str().to_owned()), (Code::Conflict, "TBL01".to_owned()));
    assert_eq!(opened(&other, "SELECT ID FROM Q WHERE ID = 3 FOR UPDATE NOWAIT").unwrap(), vec!["3"], "another row is free");
    other.rollback().unwrap();
    let (done, writer) = waiting_for(&db, other, "UPDATE Q SET DONE = 1 WHERE ID = 2", || locker.commit().unwrap());
    assert_eq!(done.map_err(|e| e.code), Ok(Completion::Success));
    writer.commit().unwrap();
    assert_eq!(opened(&locker, "SELECT DONE FROM Q WHERE ID = 2").unwrap(), vec!["1"]);
}

/// Verifies: [EXTENSION] RULES.md D185: locks FOR SHARE are held together;
/// FOR UPDATE NOWAIT of a row shared is refused, and SKIP LOCKED passes
/// over it.
#[test]
fn for_share_locks_are_shared_and_skip_locked_passes_d185() {
    let db = queue(3);
    let (a, b, c) = (db.session("A"), db.session("A"), db.session("A"));
    assert_eq!(opened(&a, "SELECT ID FROM Q WHERE ID <= 2 ORDER BY ID FOR SHARE").unwrap(), vec!["1", "2"]);
    assert_eq!(opened(&b, "SELECT ID FROM Q WHERE ID = 1 FOR SHARE NOWAIT").unwrap(), vec!["1"]);
    let refused = opened(&c, "SELECT ID FROM Q WHERE ID = 1 FOR UPDATE NOWAIT").unwrap_err();
    assert_eq!(refused.sqlstate().as_str(), "TBL01");
    assert_eq!(opened(&c, "SELECT ID FROM Q ORDER BY ID FOR UPDATE SKIP LOCKED").unwrap(), vec!["3"]);
}

/// Verifies: [EXTENSION] RULES.md D185: the row limit counts rows locked,
/// so FETCH FIRST 1 ROW ONLY with SKIP LOCKED gives the first row free
/// and locks no other.
#[test]
fn skip_locked_with_a_limit_locks_only_what_it_gives_d185() {
    let db = queue(4);
    let (a, b) = (db.session("A"), db.session("A"));
    let first = "SELECT ID FROM Q WHERE DONE = 0 ORDER BY ID FETCH FIRST 1 ROW ONLY FOR UPDATE SKIP LOCKED";
    assert_eq!(opened(&a, first).unwrap(), vec!["1"]);
    assert_eq!(opened(&b, first).unwrap(), vec!["2"]);
    assert_eq!(opened(&b, "SELECT ID FROM Q WHERE ID >= 3 ORDER BY ID FOR UPDATE NOWAIT").unwrap(), vec!["3", "4"], "a locked no more");
}

/// Verifies: [EXTENSION] RULES.md D185: two sessions dequeue jobs in a
/// loop with SKIP LOCKED, each marking its job done and committing: no job
/// is taken twice, and none is missed.
#[test]
fn a_job_queue_takes_each_job_once_d185() {
    const JOBS: usize = 60;
    let db = queue(JOBS);
    let worker = |s: Session| {
        thread::spawn(move || {
            let mut taken = Vec::new();
            loop {
                let next = "SELECT ID FROM Q WHERE DONE = 0 ORDER BY ID FETCH FIRST 1 ROW ONLY FOR UPDATE SKIP LOCKED";
                let got = opened(&s, next).unwrap();
                let Some(id) = got.first() else { break };
                s.run(&format!("UPDATE Q SET DONE = 1 WHERE ID = {id}")).unwrap();
                s.commit().unwrap();
                taken.push(id.parse::<usize>().unwrap());
            }
            s.commit().unwrap();
            taken
        })
    };
    let (one, two) = (worker(db.session("A")), worker(db.session("A")));
    let (one, two) = (one.join().unwrap(), two.join().unwrap());
    let all: BTreeSet<usize> = one.iter().chain(&two).copied().collect();
    assert_eq!(one.len() + two.len(), JOBS, "no job taken twice");
    assert_eq!(all, (1..=JOBS).collect(), "no job missed");
}

/// Verifies: [EXTENSION] RULES.md D185: at READ COMMITTED a locker that
/// waited for a writer of the row runs again at a new snapshot, giving the
/// version committed; at REPEATABLE READ it is refused, SQLCODE -911.
#[test]
fn a_locker_after_a_writer_reads_again_or_is_refused_d185() {
    for (level, expected) in [("READ COMMITTED", Ok(vec!["7".to_owned()])), ("REPEATABLE READ", Err(Code::Conflict))] {
        let db = queue(2);
        let writer = db.session("A");
        writer.run("UPDATE Q SET DONE = 7 WHERE ID = 1").unwrap();
        let s = db.session("A");
        s.run(&format!("SET TRANSACTION ISOLATION LEVEL {level}")).unwrap();
        assert_eq!(opened(&s, "SELECT DONE FROM Q WHERE ID = 1").unwrap(), vec!["0"], "{level}: its snapshot taken");
        let locker = thread::spawn(move || opened(&s, "SELECT DONE FROM Q WHERE ID = 1 FOR UPDATE").map_err(|e| e.code));
        while db.waiting_statements().unwrap() == 0 {
            assert!(!locker.is_finished(), "{level}: the locker waits for the writer");
            thread::yield_now();
        }
        writer.commit().unwrap();
        assert_eq!(locker.join().unwrap(), expected, "{level}");
    }
}

/// Verifies: [EXTENSION] RULES.md D185: a cycle of row lock waits is a
/// deadlock, the second waiter refused at once, SQLCODE -911.
#[test]
fn row_lock_waits_find_a_deadlock_d185() {
    let db = queue(2);
    let (a, b) = (db.session("A"), db.session("A"));
    assert_eq!(opened(&a, "SELECT ID FROM Q WHERE ID = 1 FOR UPDATE").unwrap(), vec!["1"]);
    assert_eq!(opened(&b, "SELECT ID FROM Q WHERE ID = 2 FOR UPDATE").unwrap(), vec!["2"]);
    let (done, a) = waiting_for(&db, a, "UPDATE Q SET DONE = 1 WHERE ID = 2", || {
        let refused = b.run("UPDATE Q SET DONE = 1 WHERE ID = 1").unwrap_err();
        assert_eq!(refused.code, Code::Conflict);
        assert!(refused.msg.contains("deadlock"), "{}", refused.msg);
        b.rollback().unwrap();
    });
    assert_eq!(done.map_err(|e| e.code), Ok(Completion::Success));
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] RULES.md D185: OF names a table the query reads;
/// a query of a join, or grouped, does not lock; a query of a view of one
/// table locks that table's rows; a declared cursor takes no FOR SHARE.
#[test]
fn locking_queries_of_other_shapes_are_refused_d185() {
    let db = queue(1);
    let s = db.session("A");
    assert_eq!(opened(&s, "SELECT ID FROM Q AS J FOR UPDATE OF J").unwrap(), vec!["1"]);
    assert_eq!(opened(&s, "SELECT ID FROM Q FOR UPDATE OF X").unwrap_err().code, Code::SyntaxRule);
    assert_eq!(opened(&s, "SELECT COUNT(*) FROM Q FOR UPDATE").unwrap_err().code, Code::Unsupported);
    assert_eq!(opened(&s, "SELECT 1 FROM Q, Q AS R FOR SHARE").unwrap_err().code, Code::Unsupported);
    s.run("CREATE VIEW V AS SELECT ID FROM Q").unwrap();
    assert_eq!(opened(&s, "SELECT ID FROM V FOR UPDATE").unwrap(), vec!["1"], "a view of one table locks its rows");
    let other = db.session("A");
    assert_eq!(opened(&other, "SELECT ID FROM Q FOR UPDATE NOWAIT").unwrap_err().sqlstate().as_str(), "TBL01");
    s.declare_cursor("DECLARE C CURSOR FOR SELECT ID FROM Q FOR SHARE").unwrap();
    assert_eq!(s.run("OPEN C").unwrap_err().code, Code::Unsupported);
}

/// Verifies: [EXTENSION] RULES.md D186: LOCK TABLE takes the table's lock
/// to the transaction's end: ACCESS SHARE alongside another's, EXCLUSIVE
/// refused NOWAIT with SQLSTATE TBL01 while one is held.
#[test]
fn lock_table_modes_and_nowait_d186() {
    let db = queue(1);
    let (a, b) = (db.session("A"), db.session("A"));
    assert_eq!(a.run("LOCK TABLE Q IN ACCESS SHARE MODE").unwrap(), Completion::Success);
    assert_eq!(b.run("LOCK Q IN SHARE MODE NOWAIT").unwrap(), Completion::Success);
    let refused = b.run("LOCK TABLE Q IN EXCLUSIVE MODE NOWAIT").unwrap_err();
    assert_eq!((refused.code, refused.sqlstate().as_str()), (Code::Conflict, "TBL01"));
    a.commit().unwrap();
    assert_eq!(b.run("LOCK TABLE Q NOWAIT").unwrap(), Completion::Success, "ACCESS EXCLUSIVE once a ended");
    assert_eq!(b.run("LOCK TABLE NOPE").unwrap_err().code, Code::SyntaxRule);
}

/// Verifies: [EXTENSION] RULES.md D185: a wait for a row lock ends at
/// LOCKS.TIMEOUT_MS, refused with SQLCODE -911, and a SKIP LOCKED never
/// waits for it.
#[test]
fn a_row_lock_wait_ends_at_its_timeout_d185() {
    let db = queue(2);
    let (a, b) = (db.session("A"), db.session("A"));
    assert_eq!(opened(&a, "SELECT ID FROM Q WHERE ID = 1 FOR UPDATE").unwrap(), vec!["1"]);
    b.run("SET LOCKS.TIMEOUT_MS = 30").unwrap();
    let refused = opened(&b, "SELECT ID FROM Q WHERE ID = 1 FOR SHARE").unwrap_err();
    assert_eq!((refused.code, refused.sqlstate().as_str()), (Code::Conflict, "TB911"));
    assert_eq!(b.run("UPDATE Q SET DONE = 1 WHERE ID = 1").unwrap_err().code, Code::Conflict, "a writer too");
    assert_eq!(opened(&b, "SELECT ID FROM Q ORDER BY ID FOR SHARE SKIP LOCKED").unwrap(), vec!["2"]);
}

//! \[DESIGN\] A statement whose completion is told only as its transaction
//! ends (RULES.md D850, D441): it leaves the row it wrote over a commit
//! not yet durable at once, with no wait for that commit's sync, and the
//! end of its transaction tells nothing before that commit is durable, or
//! tells SQLCODE -922 when its sync fails.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::{Duration, Instant};

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::log::Log;
use turborel::txn::LockWait;

use crate::common::{exec, rows};
use crate::isolation_anomalies::Slow;

/// A database whose log takes 300 ms a sync, with `T (K, V)` holding row
/// 1 of V 10; a session of it whose open transaction has changed that row,
/// its commit to come; and a READ COMMITTED session whose completions are
/// told at the end.
fn database() -> (Database, Arc<Slow>, Session, Session) {
    let db = Database::new();
    let log = Arc::new(Slow::new(Duration::from_millis(300)));
    db.set_log(Arc::clone(&log) as Arc<dyn Log>).unwrap();
    db.set_lock_wait(LockWait::Wait).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER NOT NULL PRIMARY KEY, V INTEGER)").unwrap();
    let first = db.session("A");
    first.run("INSERT INTO T VALUES (1, 10)").unwrap();
    first.commit().unwrap();
    first.run("UPDATE T SET V = 11 WHERE K = 1").unwrap();
    let second = db.session("A");
    second.run("SET TRANSACTION ISOLATION LEVEL READ COMMITTED").unwrap();
    second.tell_at_end(true);
    (db, log, first, second)
}

/// Verifies: [DESIGN] RULES.md D850, D441: a writer over a commit whose
/// sync is under way, its completion told at the end, returns well
/// within that sync; its commit returns once that commit is durable, and
/// both changes hold.
#[test]
fn a_completion_told_at_the_end_does_not_wait_for_the_sync() {
    let (db, _log, first, second) = database();
    let began = Instant::now();
    let committing = thread::spawn(move || first.commit().map_err(|e| e.code));
    let wrote = exec(&second, "UPDATE T SET V = V + 1 WHERE K = 1");
    let early = began.elapsed() < Duration::from_millis(200);
    let committed = second.commit().map_err(|e| e.code);
    assert!(began.elapsed() >= Duration::from_millis(300), "the commit waited for the sync");
    assert_eq!(wrote.map(drop), Ok(()));
    assert_eq!(committing.join().unwrap(), Ok(()));
    assert!(early, "the statement returned before the commit it wrote over was durable");
    assert_eq!(committed, Ok(()));
    assert_eq!(rows(&db.session("A"), "SELECT V FROM T").unwrap(), ["12"]);
}

/// Verifies: [DESIGN] RULES.md D850, D441: when the sync of a commit a
/// writer wrote over fails, the writer's end is in doubt, SQLCODE -922,
/// whether it commits or rolls back, though its statement was told at
/// once; a snapshot then sees neither change.
#[test]
fn an_end_resting_on_a_failed_sync_is_in_doubt() {
    for commit in [true, false] {
        let (db, log, first, second) = database();
        log.failing.store(true, Ordering::SeqCst);
        let committing = thread::spawn(move || first.commit().map_err(|e| e.code));
        assert_eq!(exec(&second, "UPDATE T SET V = V + 1 WHERE K = 1").map(drop), Ok(()), "told at the end");
        let ended = if commit { second.commit() } else { second.rollback() }.map_err(|e| e.code);
        assert_eq!(committing.join().unwrap(), Err(Code::InDoubt));
        assert_eq!(ended, Err(Code::InDoubt), "commit {commit}");
        assert_eq!(rows(&db.session("A"), "SELECT V FROM T").unwrap(), ["10"], "commit {commit}");
    }
}

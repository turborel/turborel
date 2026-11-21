//! When a commit becomes visible [X3.135-1986 §4.16; RULES.md D29, D440,
//! D441]: every row of it at once, to readers at snapshots and to writers,
//! while its versions are stamped after its locks are released; to
//! snapshots once durable, and to a writer at once, whose reply waits.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::log::Log;
use turborel::txn::LockWait;

use crate::common::{exec, rows};
use crate::isolation_anomalies::Slow;

/// Rows of the table the visibility tests write.
const ROWS: usize = 10_000;

/// A database of `T (K, V)` holding `ROWS` rows with `V` 0, whose sessions
/// wait for locks.
fn database() -> Database {
    let db = Database::new();
    db.set_lock_wait(LockWait::Wait).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE D (X INTEGER) CREATE TABLE T (K INTEGER NOT NULL PRIMARY KEY, V INTEGER)")
        .unwrap();
    let s = db.session("A");
    s.run("INSERT INTO D VALUES (0), (1), (2), (3), (4), (5), (6), (7), (8), (9)").unwrap();
    s.run("INSERT INTO T SELECT A.X * 1000 + B.X * 100 + C.X * 10 + E.X, 0 FROM D A, D B, D C, D E").unwrap();
    s.commit().unwrap();
    db
}

/// The least and greatest `V` of `T`, and how many rows, as `s` reads them.
fn spread(s: &Session) -> Vec<String> {
    let read = rows(s, "SELECT MIN(V), MAX(V), COUNT(*) FROM T").unwrap().concat();
    s.commit().unwrap();
    read.split(',').map(str::to_owned).collect()
}

/// Verifies: [X3.135-1986 §4.16]; [DESIGN] RULES.md D29, D440: a reader
/// taking a new snapshot at each statement, while a writer commits updates
/// of every one of `ROWS` rows, sees each commit whole or not at all,
/// though its versions are stamped one by one after the commit is
/// visible.
#[test]
fn a_commit_of_many_rows_is_seen_whole() {
    let db = database();
    let done = Arc::new(AtomicBool::new(false));
    let reader = {
        let (s, done) = (db.session("A"), Arc::clone(&done));
        thread::spawn(move || {
            let mut reads = 0_usize;
            while !done.load(Ordering::SeqCst) || reads == 0 {
                let read = spread(&s);
                assert_eq!(read[0], read[1], "a commit seen in part: {read:?}");
                assert_eq!(read[2], ROWS.to_string());
                reads += 1;
            }
        })
    };
    let w = db.session("A");
    for _ in 0..4 {
        w.run("UPDATE T SET V = V + 1").unwrap();
        w.commit().unwrap();
    }
    done.store(true, Ordering::SeqCst);
    reader.join().unwrap();
    assert_eq!(spread(&w), ["4", "4", &ROWS.to_string()]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.28] RULES.md D29, D440: a
/// writer that writes over rows committed but not yet stamped writes over
/// the committed values, and a reader at a snapshot taken before keeps
/// reading the values before them, from the versions the writer kept.
#[test]
fn a_writer_over_unstamped_rows_keeps_the_older_versions() {
    let db = database();
    let old = db.session("A");
    old.run("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ").unwrap();
    assert_eq!(rows(&old, "SELECT SUM(V) FROM T").unwrap(), ["0"]);
    let (a, b) = (db.session("A"), db.session("A"));
    for _ in 0..3 {
        a.run("UPDATE T SET V = V + 1 WHERE K < 5000").unwrap();
        a.commit().unwrap();
        b.run("UPDATE T SET V = V + 2 WHERE K >= 2500").unwrap();
        b.commit().unwrap();
    }
    assert_eq!(rows(&old, "SELECT SUM(V) FROM T").unwrap(), ["0"], "its snapshot is kept");
    old.commit().unwrap();
    let expected = 3 * 5000 + 2 * 3 * 7500;
    assert_eq!(rows(&old, "SELECT SUM(V) FROM T").unwrap(), [expected.to_string()]);
}

/// Verifies: [DESIGN] RULES.md D25, D441: a READ COMMITTED writer waiting
/// for a row goes on once its first writer releases its locks, before that
/// commit's sync, and writes over it; its statement's reply waits for the
/// sync, and when the sync fails it is in doubt (SQLCODE -922), as the
/// commit is, while a snapshot taken then does not see the commit.
#[test]
fn a_reply_waits_for_the_commit_it_wrote_over() {
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
    let waiting = thread::spawn(move || exec(&second, "UPDATE T SET V = V + 1 WHERE K = 1").map(drop));
    thread::sleep(Duration::from_millis(50));
    log.failing.store(true, Ordering::SeqCst);
    assert_eq!(first.commit().map_err(|e| e.code), Err(Code::InDoubt));
    assert_eq!(waiting.join().unwrap(), Err(Code::InDoubt), "it wrote over the commit in doubt");
    assert_eq!(rows(&db.session("A"), "SELECT V FROM T").unwrap(), ["10"]);
}

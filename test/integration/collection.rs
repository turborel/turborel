//! \[DESIGN\] Garbage collection under many sessions (RULES.md D29, D445):
//! one transaction's end collects a table at a time, the others pass it
//! over, and what they pass over is collected later; a table a batch of
//! commits at a time, busy or quiet (D451, D920), what a batch leaves
//! collected by the maintenance job's pass.

use std::thread;
use std::time::{Duration, Instant};

use turborel::db::Database;
use turborel::threads::JobPolicy;

use crate::common::rows;

/// The rows of T.
const ROWS: usize = 64;
/// The commits a table's earliest garbage waits past the horizon for
/// before an end collects it (RULES.md D451).
const BATCH: usize = 64;
/// The commits a batch may be due before a commit collects it itself
/// (RULES.md D925).
const BEHIND: usize = 256;

/// Verifies: [DESIGN] RULES.md D29, D445: sixteen sessions changing the
/// key of an index's rows at once, each end passing over a collection
/// under way, leave no version behind: once they end, the maintenance
/// pass collects the rest, and the index holds one entry a row.
#[test]
fn collections_passed_over_are_done_later() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (ID INTEGER NOT NULL, X INTEGER)").unwrap();
    db.create_index("T_X", "APP.T", &["X"], "btree").unwrap();
    let s = db.session("APP");
    let values: Vec<String> = (0..ROWS).map(|i| format!("({i}, 0)")).collect();
    s.run(&format!("INSERT INTO T VALUES {}", values.join(", "))).unwrap();
    s.commit().unwrap();
    let threads: Vec<_> = (0..16)
        .map(|t| {
            let db = db.clone();
            thread::spawn(move || {
                let s = db.session("APP");
                for n in 0..40 {
                    let sql = format!("UPDATE T SET X = X + 1 WHERE ID = {}", (n * 5 + t * 3) % ROWS);
                    while s.run(&sql).and_then(|_| s.commit()).is_err() {
                        let _ = s.rollback();
                    }
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    db.maintain().unwrap();
    assert_eq!(db.index_entries("T_X").unwrap(), ROWS, "every superseded version's entry taken back");
    assert_eq!(rows(&s, "SELECT SUM(X) FROM T"), Ok(vec!["640".to_owned()]));
}

/// Verifies: [DESIGN] RULES.md D450, D451, D920, D925: a session
/// collecting after its reply leaves its commit's garbage, and the
/// versions' index entries, until it collects, and then until a batch is
/// due, the horizon `BATCH` commits past it; a rollback's versions go at
/// its next collection; and the maintenance pass collects what it left
/// all the same. The pass runs only when the test asks, since another
/// table's batch due asks for it at once.
#[test]
fn a_session_collects_once_its_reply_is_sent() {
    let db = busy(2);
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.maintain().unwrap();
    let (s, u) = (db.session("APP"), db.session("APP"));
    s.collect_after_reply(true);
    s.run("UPDATE T SET X = 1 WHERE ID = 1").unwrap();
    s.commit().unwrap();
    assert_eq!(db.index_entries("T_X").unwrap(), 3, "the old version's entry kept");
    s.collect();
    assert_eq!(db.index_entries("T_X").unwrap(), 3, "no batch due yet");
    for _ in 0..BATCH {
        u.run("INSERT INTO U VALUES (1)").unwrap();
        u.commit().unwrap();
    }
    assert_eq!(db.index_entries("T_X").unwrap(), 3, "the ends of transactions that wrote another table leave it");
    s.run("UPDATE T SET X = 2 WHERE ID = 2").unwrap();
    s.rollback().unwrap();
    s.collect();
    assert_eq!(db.index_entries("T_X").unwrap(), 2, "collected by its session once the batch was due");
    s.run("UPDATE T SET X = 3 WHERE ID = 1").unwrap();
    s.commit().unwrap();
    assert_eq!(db.index_entries("T_X").unwrap(), 3);
    db.maintain().unwrap();
    assert_eq!(db.index_entries("T_X").unwrap(), 2, "the maintenance pass collected it");
}

/// Verifies: [DESIGN] RULES.md D453, D920: sixteen sessions that only
/// read, ending at once while a table's garbage awaits collection, collect
/// nothing, as the end of a transaction that wrote nothing collects no
/// table; the maintenance job, asked for by an end that left the table
/// awaiting, collects it when the program runs what is due, and leaves no
/// version behind.
#[test]
fn reading_ends_leave_collection_to_the_maintenance_pass() {
    let db = busy(ROWS);
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.maintain().unwrap();
    let s = db.session("APP");
    let reader = db.session("APP");
    rows(&reader, "SELECT COUNT(*) FROM T").unwrap();
    s.run("UPDATE T SET X = X + 1").unwrap();
    s.commit().unwrap();
    let threads: Vec<_> = (0..16)
        .map(|_| {
            let db = db.clone();
            thread::spawn(move || {
                let s = db.session("APP");
                for _ in 0..50 {
                    rows(&s, "SELECT COUNT(*) FROM T WHERE ID = 1").unwrap();
                    s.commit().unwrap();
                }
            })
        })
        .collect();
    reader.commit().unwrap();
    for t in threads {
        t.join().unwrap();
    }
    assert_eq!(db.index_entries("T_X").unwrap(), ROWS * 2, "no reading end collected it");
    db.run_due().unwrap();
    assert_eq!(db.index_entries("T_X").unwrap(), ROWS, "every superseded version's entry taken back");
}

/// Verifies: [DESIGN] RULES.md D920: an end that leaves a table awaiting
/// collection asks for the maintenance job once, the ends after it asking
/// no more; once the job's pass has begun, the next such end asks again,
/// so each batch left is collected.
#[test]
fn the_maintenance_pass_is_asked_for_after_each() {
    let db = busy(1);
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.maintain().unwrap();
    let w = db.session("APP");
    for round in 1..=2 {
        for n in 1..=3 {
            w.run(&format!("UPDATE T SET X = {} WHERE ID = 0", round * 10 + n)).unwrap();
            w.commit().unwrap();
        }
        assert_eq!(db.index_entries("T_X").unwrap(), 4, "round {round}: three old versions' entries kept");
        db.run_due().unwrap();
        assert_eq!(db.index_entries("T_X").unwrap(), 1, "round {round}: collected by the pass");
    }
}

/// Verifies: [DESIGN] RULES.md D920, D55: under JOBS.POLICY AUTOMATIC the
/// maintenance job's thread makes the pass once its second is up, with no
/// other transaction: the one commit's garbage, no batch due, is collected.
#[test]
fn the_maintenance_thread_collects_a_quiet_table_after_its_time() {
    let db = busy(1);
    db.maintain().unwrap();
    let w = db.session("APP");
    w.run("UPDATE T SET X = 1 WHERE ID = 0").unwrap();
    w.commit().unwrap();
    let began = Instant::now();
    while db.index_entries("T_X").unwrap() > 1 && began.elapsed() < Duration::from_secs(30) {
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(db.index_entries("T_X").unwrap(), 1, "collected by the job's thread");
}

/// Verifies: [DESIGN] RULES.md D451, D920, D925: a quiet table, no
/// transaction open in it, is collected a batch of commits at a time too,
/// and by the maintenance job, not by the commits: each commit leaves its
/// garbage, the one that takes the horizon `BATCH` commits past the
/// earliest too, and that one asks for the job, which the program runs
/// and which collects all of it.
#[test]
fn a_batch_due_is_left_to_the_maintenance_pass() {
    let db = busy(1);
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.maintain().unwrap();
    let w = db.session("APP");
    for n in 1..=BATCH + 1 {
        w.run(&format!("UPDATE T SET X = {n} WHERE ID = 0")).unwrap();
        w.commit().unwrap();
        assert_eq!(db.index_entries("T_X").unwrap(), n + 1, "commit {n}: every old version's entry kept");
    }
    db.run_due().unwrap();
    assert_eq!(db.index_entries("T_X").unwrap(), 1, "the batch collected by the pass");
    assert_eq!(rows(&w, "SELECT X FROM T"), Ok(vec![(BATCH + 1).to_string()]));
}

/// Verifies: [DESIGN] RULES.md D925: with the maintenance job not running
/// (JOBS.POLICY PAUSED), a table's garbage is bounded all the same: the
/// commit that finds its batch due for `BEHIND` commits collects it
/// itself, its own garbage with it, and no commit before that does.
#[test]
fn a_batch_long_due_is_collected_by_the_commit() {
    let db = busy(1);
    db.set_job_policy(JobPolicy::Paused).unwrap();
    db.maintain().unwrap();
    let w = db.session("APP");
    for n in 1..=BATCH + BEHIND {
        w.run(&format!("UPDATE T SET X = {n} WHERE ID = 0")).unwrap();
        w.commit().unwrap();
        assert_eq!(db.index_entries("T_X").unwrap(), n + 1, "commit {n}: every old version's entry kept");
    }
    w.run("UPDATE T SET X = 0 WHERE ID = 0").unwrap();
    w.commit().unwrap();
    assert_eq!(db.index_entries("T_X").unwrap(), 1, "collected by the commit that found it long due");
    assert_eq!(rows(&w, "SELECT X FROM T"), Ok(vec!["0".to_owned()]));
}

/// Verifies: [DESIGN] RULES.md D29, D920: a batch never takes a version a
/// reader may still read, at READ COMMITTED or at SERIALIZABLE: a
/// transaction open since before the commits reads its version through
/// every collection, a batch due and the maintenance pass, and once it
/// ends the pass takes the version.
#[test]
fn a_batch_keeps_what_an_open_transaction_reads() {
    for level in ["READ COMMITTED", "SERIALIZABLE"] {
        let db = busy(1);
        db.set_job_policy(JobPolicy::Paused).unwrap();
        db.maintain().unwrap();
        let (w, reader) = (db.session("APP"), db.session("APP"));
        reader.run("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE").unwrap();
        assert_eq!(rows(&reader, "SELECT X FROM T"), Ok(vec!["0".to_owned()]));
        for n in 1..=BATCH * 2 {
            w.run(&format!("SET TRANSACTION ISOLATION LEVEL {level}")).unwrap();
            w.run(&format!("UPDATE T SET X = {n} WHERE ID = 0")).unwrap();
            w.commit().unwrap();
        }
        db.maintain().unwrap();
        assert_eq!(rows(&reader, "SELECT X FROM T"), Ok(vec!["0".to_owned()]), "{level}: its snapshot's version kept");
        assert_eq!(rows(&reader, "SELECT COUNT(*) FROM T WHERE X = 0"), Ok(vec!["1".to_owned()]), "{level}: and its index entry");
        reader.commit().unwrap();
        db.maintain().unwrap();
        assert_eq!(db.index_entries("T_X").unwrap(), 1, "{level}: collected once no transaction reads it");
        assert_eq!(rows(&reader, "SELECT X FROM T"), Ok(vec![(BATCH * 2).to_string()]));
    }
}

/// A database with APP's table T, of `rows` rows (ID, 0) and the index T_X
/// on X, and APP's empty table U.
fn busy(rows: usize) -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (ID INTEGER NOT NULL, X INTEGER) CREATE TABLE U (N INTEGER)").unwrap();
    db.create_index("T_X", "APP.T", &["X"], "btree").unwrap();
    let s = db.session("APP");
    let values: Vec<String> = (0..rows).map(|i| format!("({i}, 0)")).collect();
    s.run(&format!("INSERT INTO T VALUES {}", values.join(", "))).unwrap();
    s.commit().unwrap();
    db
}

/// Verifies: [DESIGN] RULES.md D451, D925: while a transaction is open in
/// a table and the maintenance job does not run, the ends of those that
/// write it pass its garbage over until its batch has been due for
/// `BEHIND` commits of the horizon, then one collects it.
#[test]
fn a_busy_table_is_collected_a_batch_of_commits_at_a_time() {
    let db = busy(2);
    db.set_job_policy(JobPolicy::Paused).unwrap();
    db.maintain().unwrap();
    let w = db.session("APP");
    let readers = [db.session("APP"), db.session("APP")];
    rows(&readers[0], "SELECT X FROM T").unwrap();
    w.run("UPDATE T SET X = 1 WHERE ID = 0").unwrap();
    w.commit().unwrap();
    // Each round a reader begins in T, the other ends, and a commit that
    // changes no key of T moves the clock: T is never without a
    // transaction open.
    for n in 1..=BATCH + BEHIND + 4 {
        rows(&readers[n % 2], "SELECT X FROM T").unwrap();
        readers[(n + 1) % 2].commit().unwrap();
        w.run("UPDATE T SET ID = 1 WHERE ID = 1").unwrap();
        w.commit().unwrap();
        if n == 2 {
            assert_eq!(db.index_entries("T_X").unwrap(), 3, "the old version's entry kept while T is busy");
        }
    }
    assert_eq!(db.index_entries("T_X").unwrap(), 2, "collected once the batch was long due");
}

/// Verifies: [DESIGN] RULES.md D451: a table whose garbage passes the
/// bound of versions is collected at the next end of a transaction that
/// wrote it, however near the horizon; what it collected counts no more.
#[test]
fn a_table_with_much_garbage_is_collected_at_once() {
    let db = busy(5_000);
    db.set_job_policy(JobPolicy::Paused).unwrap();
    db.maintain().unwrap();
    let w = db.session("APP");
    w.run("UPDATE T SET X = 1").unwrap();
    w.commit().unwrap();
    assert_eq!(db.index_entries("T_X").unwrap(), 5_000, "every old version's entry taken back");
    // What was collected no longer counts: one more version waits.
    w.run("UPDATE T SET X = 2 WHERE ID = 0").unwrap();
    w.commit().unwrap();
    assert_eq!(db.index_entries("T_X").unwrap(), 5_001, "the one old version's entry kept, no batch due");
}

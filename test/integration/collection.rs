//! \[DESIGN\] Garbage collection under many sessions (RULES.md D29, D445):
//! one transaction's end collects a table at a time, the others pass it
//! over, and what they pass over is collected by a later end; a busy
//! table a batch of commits at a time (D451).

use std::thread;

use turborel::db::Database;

use crate::common::rows;

/// The rows of T.
const ROWS: usize = 64;

/// Verifies: [DESIGN] RULES.md D29, D445: sixteen sessions changing the
/// key of an index's rows at once, each end passing over a collection
/// under way, leave no version behind: once they end, one transaction's
/// end collects the rest, and the index holds one entry a row.
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
    rows(&s, "SELECT ID FROM T WHERE ID = 0").unwrap();
    s.commit().unwrap();
    assert_eq!(db.index_entries("T_X").unwrap(), ROWS, "every superseded version's entry taken back");
    assert_eq!(rows(&s, "SELECT SUM(X) FROM T"), Ok(vec!["640".to_owned()]));
}

/// Verifies: [DESIGN] RULES.md D450: a session collecting after its reply
/// leaves its commit's garbage, and the versions' index entries, until it
/// collects; a rollback's too; and another transaction's end collects
/// what it left all the same.
#[test]
fn a_session_collects_once_its_reply_is_sent() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (ID INTEGER NOT NULL, X INTEGER)").unwrap();
    db.create_index("T_X", "APP.T", &["X"], "btree").unwrap();
    let s = db.session("APP");
    s.run("INSERT INTO T VALUES (1, 0), (2, 0)").unwrap();
    s.commit().unwrap();
    s.collect_after_reply(true);
    s.run("UPDATE T SET X = 1 WHERE ID = 1").unwrap();
    s.commit().unwrap();
    assert_eq!(db.index_entries("T_X").unwrap(), 3, "the old version's entry kept");
    s.collect();
    assert_eq!(db.index_entries("T_X").unwrap(), 2);
    s.run("UPDATE T SET X = 2 WHERE ID = 2").unwrap();
    s.rollback().unwrap();
    s.run("UPDATE T SET X = 3 WHERE ID = 1").unwrap();
    s.commit().unwrap();
    assert_eq!(db.index_entries("T_X").unwrap(), 3);
    let other = db.session("APP");
    assert_eq!(rows(&other, "SELECT X FROM T WHERE ID = 2").unwrap(), vec!["0".to_owned()]);
    other.commit().unwrap();
    assert_eq!(db.index_entries("T_X").unwrap(), 2, "another end collected it");
}

/// Verifies: [DESIGN] RULES.md D453: sixteen sessions that only read,
/// ending at once while a table's garbage awaits collection, collect it one
/// at a time, the others passing by; once they end, the end of one more
/// that reads leaves no version behind.
#[test]
fn reading_ends_collect_one_at_a_time_and_leave_nothing() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (ID INTEGER NOT NULL, X INTEGER)").unwrap();
    db.create_index("T_X", "APP.T", &["X"], "btree").unwrap();
    let s = db.session("APP");
    let values: Vec<String> = (0..ROWS).map(|i| format!("({i}, 0)")).collect();
    s.run(&format!("INSERT INTO T VALUES {}", values.join(", "))).unwrap();
    s.commit().unwrap();
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
    rows(&s, "SELECT ID FROM T WHERE ID = 0").unwrap();
    s.commit().unwrap();
    assert_eq!(db.index_entries("T_X").unwrap(), ROWS, "every superseded version's entry taken back");
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

/// Verifies: [DESIGN] RULES.md D451: while a transaction is open in a
/// table, the ends of others pass its garbage over until the horizon is
/// a batch of commits past it, then one collects it.
#[test]
fn a_busy_table_is_collected_a_batch_of_commits_at_a_time() {
    let db = busy(1);
    let (w, u) = (db.session("APP"), db.session("APP"));
    let readers = [db.session("APP"), db.session("APP")];
    rows(&readers[0], "SELECT X FROM T").unwrap();
    w.run("UPDATE T SET X = 1 WHERE ID = 0").unwrap();
    w.commit().unwrap();
    // Each round a reader begins in T, the other ends, and a commit in U
    // moves the clock: T is never without a transaction open.
    for n in 1..=100 {
        rows(&readers[n % 2], "SELECT X FROM T").unwrap();
        readers[(n + 1) % 2].commit().unwrap();
        u.run("INSERT INTO U VALUES (1)").unwrap();
        u.commit().unwrap();
        if n == 2 {
            assert_eq!(db.index_entries("T_X").unwrap(), 2, "the old version's entry kept while T is busy");
        }
    }
    assert_eq!(db.index_entries("T_X").unwrap(), 1, "collected once the horizon passed the batch");
}

/// Verifies: [DESIGN] RULES.md D451: a busy table whose garbage passes the
/// bound of versions is collected at the next end, however near the
/// horizon; what it collected counts no more.
#[test]
fn a_busy_table_with_much_garbage_is_collected_at_once() {
    let db = busy(5_000);
    let w = db.session("APP");
    let readers = [db.session("APP"), db.session("APP")];
    rows(&readers[0], "SELECT X FROM T WHERE ID = 0").unwrap();
    w.run("UPDATE T SET X = 1").unwrap();
    w.commit().unwrap();
    rows(&readers[1], "SELECT X FROM T WHERE ID = 0").unwrap();
    readers[0].commit().unwrap();
    assert_eq!(db.index_entries("T_X").unwrap(), 5_000, "every old version's entry taken back");
    // What was collected no longer counts: one more version waits.
    w.run("UPDATE T SET X = 2 WHERE ID = 0").unwrap();
    w.commit().unwrap();
    rows(&readers[0], "SELECT X FROM T WHERE ID = 0").unwrap();
    readers[1].commit().unwrap();
    assert_eq!(db.index_entries("T_X").unwrap(), 5_001, "the one old version's entry kept while T is busy");
    readers[0].commit().unwrap();
}

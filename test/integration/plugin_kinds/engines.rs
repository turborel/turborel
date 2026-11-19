//! \[EXTENSION\] Tables kept by an extension's storage engine (RULES.md
//! D224, D225), FILES of the example: rows written a batch a call at
//! commit, read a page a call; transactions isolated by the coordinator's
//! table locks; rollback, savepoints, a program restarted mid-transaction,
//! and a crash of the database recovered.

use turborel::db::{Database, OpenOptions};
use turborel::error::Code;
use turborel::value::Value;

use super::{KINDS, calls, kinds, state};
use crate::common::{Scratch, after_crash, rows};
use crate::plugins::fixture::{allow, install, manifest};

/// `n` rows of T, from `from`, as one INSERT.
fn insert(from: u32, n: u32) -> String {
    let values: Vec<String> = (from..from + n).map(|k| format!("({k}, 'v{k}')")).collect();
    format!("INSERT INTO T VALUES {}", values.join(", "))
}

/// Verifies: [EXTENSION] RULES.md D224: a table USING an extension's
/// engine keeps its rows in the program, written at commit a BATCH of
/// rows a call and scanned a page a call; updates and deletes are read
/// back; a rollback, and one to a savepoint, leave the program untold.
#[test]
fn an_engines_rows_are_batched() {
    let (_scratch, db) = kinds("plugin-engines-rows", "");
    let s = db.session("U");
    s.run("CREATE TABLE T (K INTEGER, V VARCHAR(20)) USING FILES").unwrap();
    s.commit().unwrap();
    s.run(&insert(0, 1000)).unwrap();
    let before = calls(&s);
    s.commit().unwrap();
    let written = calls(&s) - before;
    assert!((20..=25).contains(&written), "1000 rows read and applied in batches of 100: {written} calls");
    assert_eq!(rows(&s, "SELECT COUNT(*), SUM(K) FROM T").unwrap(), ["1000,499500"]);
    s.run("UPDATE T SET V = 'x' WHERE K < 10").unwrap();
    s.run("DELETE FROM T WHERE K >= 990").unwrap();
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM T WHERE V = 'x'").unwrap(), ["10"], "its own writes");
    s.commit().unwrap();
    assert_eq!(rows(&s, "SELECT COUNT(*), MAX(K) FROM T").unwrap(), ["990,989"]);
    s.run(&insert(5000, 3)).unwrap();
    s.rollback().unwrap();
    s.run(&insert(6000, 1)).unwrap();
    s.run("SAVEPOINT A").unwrap();
    s.run(&insert(7000, 2)).unwrap();
    s.run("DELETE FROM T WHERE K = 0").unwrap();
    s.run("ROLLBACK TO SAVEPOINT A").unwrap();
    s.commit().unwrap();
    assert_eq!(rows(&s, "SELECT COUNT(*), MAX(K), MIN(K) FROM T").unwrap(), ["991,6000,0"]);
}

/// Verifies: [EXTENSION] RULES.md D224: the coordinator isolates an
/// engine's transactions by table locks, so a reader of a table a writer
/// holds is refused (-911, locking no-wait), and writers on threads of
/// their own, waiting for the lock, lose no row.
#[test]
fn an_engines_transactions_are_serialized() {
    let (_scratch, db) = kinds("plugin-engines-concurrent", "");
    let s = db.session("U");
    s.run("CREATE TABLE T (K INTEGER, V VARCHAR(20)) USING FILES").unwrap();
    s.commit().unwrap();
    s.run(&insert(0, 5)).unwrap();
    let other = db.session("U");
    assert_eq!(rows(&other, "SELECT COUNT(*) FROM T"), Err(Code::Conflict));
    other.rollback().unwrap();
    s.commit().unwrap();
    assert_eq!(rows(&other, "SELECT COUNT(*) FROM T").unwrap(), ["5"]);
    other.commit().unwrap();
    std::thread::scope(|scope| {
        for t in 0..4_u32 {
            let db = &db;
            scope.spawn(move || {
                let w = db.session("U");
                w.set_setting("LOCKS.WAIT", &Value::Char("ON".into())).unwrap();
                for n in 0..5 {
                    w.run(&insert(1000 * (t + 1) + 10 * n, 10)).unwrap();
                    w.commit().unwrap();
                }
            });
        }
    });
    assert_eq!(rows(&s, "SELECT COUNT(*), COUNT(DISTINCT K) FROM T").unwrap(), ["205,205"]);
}

/// Verifies: [EXTENSION] RULES.md D225: a process of the extension that
/// stops while a transaction has written a table fails that transaction
/// at commit as a serialization failure (40001) to run again, rolled
/// back, and the table keeps its committed
/// rows, whole.
#[test]
fn a_program_restarted_mid_transaction_fails_it() {
    let (_scratch, db) = kinds("plugin-engines-restart", "");
    let s = db.session("U");
    s.run("CREATE TABLE T (K INTEGER, V VARCHAR(20)) USING FILES").unwrap();
    s.run(&insert(0, 10)).unwrap();
    s.commit().unwrap();
    s.run(&insert(100, 10)).unwrap();
    s.run("DELETE FROM T WHERE K < 5").unwrap();
    assert_eq!(state(&s, "VALUES (CRASH())"), "38T01");
    let refused = s.commit().unwrap_err();
    assert_eq!((refused.code, refused.sqlstate().as_str()), (Code::Conflict, "40001"));
    assert!(refused.msg.contains("stopped while the transaction wrote"), "{}", refused.msg);
    assert_eq!(rows(&s, "SELECT COUNT(*), MIN(K), MAX(K) FROM T").unwrap(), ["10,0,9"]);
    s.run(&insert(200, 1)).unwrap();
    s.commit().unwrap();
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM T").unwrap(), ["11"], "the next transaction commits");
}

/// Verifies: [EXTENSION] RULES.md D225: an engine's table survives a
/// checkpoint, commits after it and a crash, the log's changes applied
/// again past the position the program is durable through; a transaction
/// the crash cut leaves nothing.
#[test]
fn an_engines_table_is_recovered() {
    let scratch = Scratch::new("plugin-engines-recovery");
    let root = scratch.path("plugins");
    install(&root, ("KINDS", "1.0.0"), &manifest("KINDS", "1.0.0", "", KINDS));
    let dir = scratch.path("db");
    let open = || {
        let db = Database::new();
        allow(&db, &root);
        db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
        db
    };
    let db = open();
    db.schema("CREATE SCHEMA AUTHORIZATION U").unwrap();
    let s = db.session("U");
    s.run("CREATE EXTENSION kinds").unwrap();
    s.run("CREATE TABLE T (K INTEGER, V VARCHAR(20)) USING FILES").unwrap();
    s.run(&insert(0, 50)).unwrap();
    s.commit().unwrap();
    db.checkpoint().unwrap();
    s.run(&insert(50, 50)).unwrap();
    s.run("UPDATE T SET V = 'after' WHERE K = 1").unwrap();
    s.commit().unwrap();
    let cut = db.session("U");
    cut.run(&insert(500, 5)).unwrap();
    drop(s);
    drop(db);
    let db = open();
    let s = db.session("U");
    assert_eq!(rows(&s, "SELECT COUNT(*), MAX(K) FROM T").unwrap(), ["100,99"]);
    assert_eq!(rows(&s, "SELECT V FROM T WHERE K = 1").unwrap(), ["after"]);
    s.run(&insert(100, 1)).unwrap();
    s.commit().unwrap();
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM T").unwrap(), ["101"]);
    drop(cut);
}

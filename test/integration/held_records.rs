//! A transaction's log records held until it commits (RULES.md D25,
//! D470): a checkpoint taken while transactions hold records writes them
//! first, so a crash after it recovers exactly what committed; a
//! transaction holding more than a run's worth writes them as it goes,
//! and recovers the same.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use turborel::db::{Database, LogStorage, OpenOptions, Opened, OpenedLog, StorePlace};
use turborel::error::{Code, Result};
use turborel::log::Log;

use crate::common::{Scratch, Watched, after_crash, number, rows, small_pool, sorted};

/// A log storage giving one log, which a test keeps across a crash and
/// has refuse appends: the database's directory holds no log of its own.
struct Given(Arc<Watched>);

impl LogStorage for Given {
    fn name(&self) -> &'static str {
        "given"
    }
    fn open(&self, _place: &StorePlace<'_>) -> Result<Option<OpenedLog>> {
        Ok(Some(OpenedLog { log: Arc::clone(&self.0) as Arc<dyn Log>, cut: 0_u64 }))
    }
    fn exists(&self, _place: &StorePlace<'_>) -> Result<bool> {
        Ok(false)
    }
    fn create(&self, _place: &StorePlace<'_>) -> Result<Arc<dyn Log>> {
        Ok(Arc::clone(&self.0) as Arc<dyn Log>)
    }
}

/// The database in `dir`, as `open` opens it, logging to `log`.
fn open_logging(dir: &Path, log: &Arc<Watched>) -> (Database, Opened) {
    let db = Database::new();
    small_pool(&db, 32);
    db.set_log_storage(Box::new(Given(Arc::clone(log)))).unwrap();
    let opened = db.open_files(&after_crash(), dir, &OpenOptions::default()).unwrap();
    db.set_setting("CHECKPOINT.AFTER_CHANGES", &number(i64::MAX)).unwrap();
    db.set_setting("CHECKPOINT.AFTER_BYTES", &number(i64::MAX)).unwrap();
    (db, opened)
}

/// The database in `dir`, with a small buffer pool, so pages are written
/// back as it runs, and no checkpoint but those asked for.
fn open(dir: &Path) -> (Database, Opened) {
    let db = Database::new();
    small_pool(&db, 32);
    let opened = db.open_files(&after_crash(), dir, &OpenOptions::default()).unwrap();
    db.set_setting("CHECKPOINT.AFTER_CHANGES", &number(i64::MAX)).unwrap();
    db.set_setting("CHECKPOINT.AFTER_BYTES", &number(i64::MAX)).unwrap();
    (db, opened)
}

/// Schema APP: T with a UNIQUE key, 100 rows.
fn define(db: &Database) {
    db.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (K INTEGER NOT NULL UNIQUE, V CHAR(10))").unwrap();
    let s = db.session("APP");
    for k in 0..100 {
        s.run(&format!("INSERT INTO T VALUES ({k}, 'base')")).unwrap();
    }
    s.commit().unwrap();
}

/// Verifies: [X3.135-1986 §8.2, §8.9]; [DESIGN] RULES.md D25, D470, a
/// checkpoint taken mid-transaction: the records each open transaction
/// held are written before it, so after a crash a transaction that
/// committed after it has its changes from both sides of it; one rolled
/// back after it, and one open at the crash, have none; changes rolled
/// back to a savepoint taken before it, and a statement undone after it,
/// are undone; and the image holds no change twice.
#[test]
fn a_checkpoint_mid_transaction_recovers_what_committed() {
    let scratch = Scratch::new("held-checkpoint");
    let dir = scratch.path("db");
    let (crashed, _) = open(&dir);
    define(&crashed);
    let (spans, rolled, open_tx, saved) = (crashed.session("APP"), crashed.session("APP"), crashed.session("APP"), crashed.session("APP"));
    spans.run("UPDATE T SET V = 'spans' WHERE K < 10").unwrap();
    spans.run("INSERT INTO T VALUES (1000, 'before')").unwrap();
    rolled.run("DELETE FROM T WHERE K >= 90").unwrap();
    open_tx.run("INSERT INTO T VALUES (2000, 'open')").unwrap();
    saved.run("INSERT INTO T VALUES (3000, 'kept')").unwrap();
    saved.run("SAVEPOINT S").unwrap();
    saved.run("UPDATE T SET V = 'undone' WHERE K >= 20 AND K < 30").unwrap();
    crashed.checkpoint().unwrap();
    spans.run("INSERT INTO T VALUES (1001, 'after')").unwrap();
    assert!(spans.run("UPDATE T SET K = 1000 WHERE K = 1001").is_err(), "UNIQUE: the statement is undone");
    spans.commit().unwrap();
    rolled.rollback().unwrap();
    saved.run("ROLLBACK TO SAVEPOINT S").unwrap();
    saved.run("INSERT INTO T VALUES (3001, 'after')").unwrap();
    saved.commit().unwrap();
    open_tx.run("UPDATE T SET V = 'open' WHERE K = 50").unwrap();
    let all = "SELECT K, V FROM T WHERE V <> 'base' OR K >= 90";
    let expected = sorted(&crashed.session("APP"), all);
    let count = rows(&crashed.session("APP"), "SELECT COUNT(*) FROM T").unwrap();

    let (db, opened) = open(&dir);
    assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
    assert_eq!(sorted(&db.session("APP"), all), expected);
    assert_eq!(rows(&db.session("APP"), "SELECT COUNT(*) FROM T").unwrap(), count);
    assert_eq!(count, ["104"], "100 + 1000, 1001, 3000, 3001; the open insert and the rolled-back delete undone");
    assert_eq!(rows(&db.session("APP"), "SELECT COUNT(*) FROM T WHERE V = 'undone'"), Ok(vec!["0".into()]));
    assert_eq!(rows(&db.session("APP"), "SELECT COUNT(*) FROM T WHERE V = 'spans'"), Ok(vec!["10".into()]));
    drop((db, spans, rolled, open_tx, saved, crashed));
}

/// Verifies: [X3.135-1986 §8.2, §8.9]; [DESIGN] RULES.md D25, D470, a
/// transaction holding more than a run's worth of records writes them as
/// it goes: after a crash, one that committed has every row; one rolled
/// back after writing has none; a statement undone after its first
/// records were written is undone, and the rest of its transaction kept.
#[test]
fn a_large_transaction_writes_as_it_goes_and_recovers() {
    let scratch = Scratch::new("held-large");
    let dir = scratch.path("db");
    let (crashed, _) = open(&dir);
    crashed.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (K INTEGER NOT NULL UNIQUE, V CHAR(1000))").unwrap();
    let s = crashed.session("APP");
    // About 1.5 MiB of records, past a run's worth.
    for k in 0..1_500 {
        s.run(&format!("INSERT INTO T VALUES ({k}, 'big')")).unwrap();
    }
    s.commit().unwrap();
    let rolled = crashed.session("APP");
    rolled.run("UPDATE T SET V = 'rolled'").unwrap();
    rolled.rollback().unwrap();
    // One statement past a run's worth, failing at its last row.
    assert!(s.run("UPDATE T SET K = CASE WHEN K = 1499 THEN 10000 ELSE K + 10000 END").is_err(), "UNIQUE");
    s.run("INSERT INTO T VALUES (5000, 'after')").unwrap();
    s.commit().unwrap();
    let expected = sorted(&s, "SELECT K, V FROM T WHERE K < 3 OR K >= 1498");

    let (db, opened) = open(&dir);
    assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
    let t = db.session("APP");
    assert_eq!(sorted(&t, "SELECT K, V FROM T WHERE K < 3 OR K >= 1498"), expected);
    assert_eq!(rows(&t, "SELECT COUNT(*) FROM T"), Ok(vec!["1501".into()]));
    assert_eq!(rows(&t, "SELECT COUNT(*) FROM T WHERE V = 'rolled' OR K >= 10000"), Ok(vec!["0".into()]));
    drop((db, t, s, rolled, crashed));
}

/// Verifies: [DESIGN] RULES.md D25, D470, a transaction whose records are
/// written open, past what it holds or by the checkpoint, a checkpoint
/// taken before its commit record: after a crash, each row it changed
/// reads as it committed, a row of a transaction open at the crash as it
/// was, and each is updated and deleted again; for each seed of how its
/// records were written and whether it changes a row after the
/// checkpoint.
#[test]
fn a_change_spanning_a_checkpoint_is_written_again_after_recovery() {
    for seed in 0_u64..4 {
        let scratch = Scratch::new(&format!("held-spanning-{seed}"));
        let dir = scratch.path("db");
        let (crashed, _) = open(&dir);
        crashed.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (K INTEGER NOT NULL UNIQUE, V CHAR(1000))").unwrap();
        let s = crashed.session("APP");
        for k in 0..1_200 {
            s.run(&format!("INSERT INTO T VALUES ({k}, 'base')")).unwrap();
        }
        s.commit().unwrap();
        let (spans, open_tx) = (crashed.session("APP"), crashed.session("APP"));
        // Past a run's worth when the seed is odd: written as it goes.
        let upto = if seed % 2 == 1 { 1_100 } else { 10 };
        spans.run(&format!("UPDATE T SET V = 'spans' WHERE K < {upto}")).unwrap();
        open_tx.run("UPDATE T SET V = 'open' WHERE K = 1150").unwrap();
        crashed.checkpoint().unwrap();
        if seed >= 2 {
            spans.run("UPDATE T SET V = 'twice' WHERE K = 0").unwrap();
        }
        spans.commit().unwrap();
        let expected = sorted(&crashed.session("APP"), "SELECT K, V FROM T WHERE K < 2 OR K = 1150");

        let (db, opened) = open(&dir);
        assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
        let t = db.session("APP");
        assert_eq!(sorted(&t, "SELECT K, V FROM T WHERE K < 2 OR K = 1150"), expected, "seed {seed}");
        t.run("UPDATE T SET V = 'again' WHERE K < 5 OR K = 1150").unwrap();
        t.run("DELETE FROM T WHERE K = 6").unwrap();
        t.commit().unwrap();
        db.checkpoint().unwrap();
        assert_eq!(rows(&t, "SELECT COUNT(*) FROM T WHERE V = 'again'"), Ok(vec!["6".into()]), "seed {seed}");
        drop((db, t, s, spans, open_tx, crashed));
    }
}

/// Verifies: [X3.135-1986 §8.2, §8.9]; [DESIGN] RULES.md D25, D470, D1250,
/// transactions updating rows and rolling back as checkpoints are taken,
/// beside a transaction open since before them, from whose first record
/// the log is kept; then every row updated and committed, a checkpoint,
/// a crash: recovery reads the log from the open transaction's first
/// record and finds no record of a transaction rolled back without its
/// abort record, so each row reads as committed last. Regression test
/// (R-TEST-7): a checkpoint taking the gate as a rollback left it wrote
/// the records the transaction still held, and recovery put those rows
/// back as the records found them, over the commit.
#[test]
fn rollbacks_beside_checkpoints_leave_what_is_committed_after() {
    let scratch = Scratch::new("held-rollbacks");
    let dir = scratch.path("db");
    let (crashed, _) = open(&dir);
    define(&crashed);
    let open_tx = crashed.session("APP");
    open_tx.run("INSERT INTO T VALUES (5000, 'open')").unwrap();
    crashed.checkpoint().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let rolling: Vec<_> = (0..4_u32)
        .map(|r| {
            let (db, stop) = (crashed.clone(), Arc::clone(&stop));
            thread::spawn(move || {
                let s = db.session("APP");
                let mut n = 0_u32;
                while !stop.load(Ordering::Acquire) {
                    s.run(&format!("UPDATE T SET V = 'rolled' WHERE K = {}", r * 25 + n % 25)).unwrap();
                    s.rollback().unwrap();
                    n += 1;
                }
            })
        })
        .collect();
    for _ in 0..40 {
        crashed.checkpoint().unwrap();
    }
    stop.store(true, Ordering::Release);
    for r in rolling {
        r.join().unwrap();
    }
    let s = crashed.session("APP");
    s.run("UPDATE T SET V = 'kept' WHERE K < 100").unwrap();
    s.commit().unwrap();
    crashed.checkpoint().unwrap();
    let expected = sorted(&s, "SELECT K, V FROM T");

    let (db, opened) = open(&dir);
    assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
    assert_eq!(sorted(&db.session("APP"), "SELECT K, V FROM T"), expected);
    assert_eq!(rows(&db.session("APP"), "SELECT COUNT(*) FROM T WHERE V = 'kept'"), Ok(vec!["100".into()]));
    drop((db, s, open_tx, crashed));
}

/// Verifies: [X3.135-1986 §8.9]; [DESIGN] RULES.md D25, D1250, D1258, a
/// transaction whose record a checkpoint wrote, rolled back while its log
/// refuses the abort record: its change is undone, and the database
/// stops, taking no change of the next writer of its row and no
/// checkpoint (SQLCODE -915, naming the abort record) though the log
/// takes appends again, reads going on; after a crash, recovery, reading
/// the log from the first record of a transaction open since before,
/// reads every row as committed and nothing of either transaction, and
/// the database takes changes again. Regression test (R-TEST-7): the
/// failed append was ignored, so the records stood in the log with no
/// end, the next writer of the row committed after them, a checkpoint
/// held its commit, and recovery put the row back over it.
#[test]
fn a_refused_abort_record_stops_the_database() {
    let scratch = Scratch::new("held-abort-refused");
    let dir = scratch.path("db");
    let log = Arc::new(Watched { durable: true, ..Watched::default() });
    let (crashed, _) = open_logging(&dir, &log);
    define(&crashed);
    let open_tx = crashed.session("APP");
    open_tx.run("INSERT INTO T VALUES (5000, 'open')").unwrap();
    // Its first record written before the other's, as a checkpoint writes it.
    crashed.write_held().unwrap();
    let rolled = crashed.session("APP");
    rolled.run("UPDATE T SET V = 'rolled' WHERE K = 1").unwrap();
    crashed.checkpoint().unwrap();
    log.fail_appends.store(true, Ordering::SeqCst);
    rolled.rollback().unwrap();
    log.fail_appends.store(false, Ordering::SeqCst);
    assert_eq!(crashed.log_failure().map(|e| e.code), Some(Code::Io));
    let next = crashed.session("APP");
    let refused = next.run("UPDATE T SET V = 'next' WHERE K = 1").unwrap_err();
    assert_eq!(refused.code, Code::Io, "{refused}");
    assert!(refused.msg.contains("abort record") && refused.msg.contains("opened again"), "{refused}");
    assert_eq!(crashed.checkpoint().err().map(|e| e.code), Some(Code::Io));
    assert_eq!(rows(&crashed.session("APP"), "SELECT V FROM T WHERE K = 1"), Ok(vec!["base      ".into()]));

    let (db, opened) = open_logging(&dir, &log);
    assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
    assert_eq!(rows(&db.session("APP"), "SELECT COUNT(*) FROM T"), Ok(vec!["100".into()]));
    assert_eq!(rows(&db.session("APP"), "SELECT COUNT(*) FROM T WHERE V <> 'base'"), Ok(vec!["0".into()]));
    let s = db.session("APP");
    s.run("UPDATE T SET V = 'after' WHERE K = 1").unwrap();
    s.commit().unwrap();
    assert_eq!(rows(&s, "SELECT V FROM T WHERE K = 1"), Ok(vec!["after     ".into()]));
    drop((db, s, next, rolled, open_tx, crashed));
}

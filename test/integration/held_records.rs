//! A transaction's log records held until it commits (RULES.md D25,
//! D470): a checkpoint taken while transactions hold records writes them
//! first, so a crash after it recovers exactly what committed; a
//! transaction holding more than a run's worth writes them as it goes,
//! and recovers the same.

use std::path::Path;

use turborel::db::{Database, OpenOptions, Opened};

use crate::common::{Scratch, after_crash, number, rows, small_pool, sorted};

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

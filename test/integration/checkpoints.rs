//! Checkpoints (RULES.md D25): a database recovers from its
//! last checkpoint and its log, whatever was under way when it was taken;
//! the log is truncated behind what recovery needs; checkpoints are taken
//! as the database runs, writers going on around them; and recovery's
//! rules undo and redo exactly what a table's pages lack or hold wrongly.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;

use turborel::catalog::{QualifiedName, TableDesc};
use turborel::db::{Database, OpenOptions, Opened};
use turborel::error::{Code, Result};
use turborel::log::record::{Change, Record};
use turborel::log::{Log, Lsn, MemoryLog};
use turborel::params::Params;
use turborel::storage::{Row, RowId, Rows, StorageEngine, TableStorage};
use turborel::txn::{NO_TRANSACTION, TxCapabilities, TxId};

use crate::common::{MemoryTable, Scratch, after_crash, number, os, rows, small_pool, sorted};

/// The database in `dir`, with a small buffer pool, so pages are written
/// back as it runs, and checkpoints after `after` row changes.
fn open(dir: &Path, after: i64) -> (Database, Opened) {
    let db = Database::new();
    small_pool(&db, 32);
    db.set_setting("LOG.SEGMENT_SIZE", &number(16 * 1024)).unwrap();
    let opened = db.open_files(&after_crash(), dir, &OpenOptions::default()).unwrap();
    due(&db, after, i64::MAX);
    (db, opened)
}

/// Checkpoints in `db` after `changes` row changes, or `bytes` written to
/// the side file.
fn due(db: &Database, changes: i64, bytes: i64) {
    db.set_setting("CHECKPOINT.AFTER_CHANGES", &number(changes)).unwrap();
    db.set_setting("CHECKPOINT.AFTER_BYTES", &number(bytes)).unwrap();
}

/// Schema APP: T with a UNIQUE key and a B-link index, 1,000 rows.
fn define(db: &Database) {
    db.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (K INTEGER NOT NULL UNIQUE, V CHAR(10))").unwrap();
    db.create_index("T_V", "APP.T", &["V"], "btree").unwrap();
    let s = db.session("APP");
    for k in 0..1_000 {
        s.run(&format!("INSERT INTO T VALUES ({k}, 'v{}')", k % 10)).unwrap();
    }
    s.commit().unwrap();
}

/// Verifies: [X3.135-1986 §4.16, §8.2, §8.9]; [DESIGN] RULES.md D25, a
/// crash after a checkpoint: transactions open when it was taken keep
/// what they did after, committed, or lose all of it, rolled back or open
/// at the crash; a statement undone after it is undone; a table defined
/// after it is back; rows written after it, and written back to the data
/// file as the pool needed room, are not in the file unless committed;
/// indexes, reopened as it left them and mended, find rows inserted after
/// it and not rows undone, and UNIQUE holds over them; recovery reads the
/// log from the checkpoint, not its start.
#[test]
fn a_crash_after_a_checkpoint_recovers() {
    let scratch = Scratch::new("checkpoint-crash");
    let dir = scratch.path("db");
    let (crashed, _) = open(&dir, i64::MAX);
    define(&crashed);
    let (later, rolled, open_tx) = (crashed.session("APP"), crashed.session("APP"), crashed.session("APP"));
    later.run("UPDATE T SET V = 'later' WHERE K < 10").unwrap();
    rolled.run("UPDATE T SET V = 'rolled' WHERE K >= 990").unwrap();
    open_tx.run("DELETE FROM T WHERE K >= 500 AND K < 510").unwrap();
    crashed.checkpoint().unwrap();
    later.run("INSERT INTO T VALUES (2000, 'after')").unwrap();
    later.commit().unwrap();
    rolled.rollback().unwrap();
    let s = crashed.session("APP");
    for k in 3_000..3_400 {
        s.run(&format!("INSERT INTO T VALUES ({k}, 'bulk')")).unwrap();
    }
    s.commit().unwrap();
    assert!(s.run("UPDATE T SET K = 1 WHERE K = 2000").is_err(), "UNIQUE: the statement is undone");
    crashed.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE U (K INTEGER NOT NULL)").unwrap();
    crashed.session("B").run("INSERT INTO U VALUES (7)").unwrap();
    crashed.session("B").run("COMMIT WORK").unwrap();
    open_tx.run("INSERT INTO T VALUES (9999, 'open')").unwrap();
    let expected = sorted(&crashed.session("APP"), "SELECT K, V FROM T WHERE K < 20 OR K >= 985");
    let count = rows(&crashed.session("APP"), "SELECT COUNT(*) FROM T").unwrap();

    let (db, opened) = open(&dir, i64::MAX);
    let Opened::Recovered(report) = opened else { panic!("{opened:?}") };
    assert!(report.records < 900, "read from the checkpoint: {report:?}");
    assert!(report.undone >= 1 && report.redone >= 400, "{report:?}");
    assert_eq!(sorted(&db.session("APP"), "SELECT K, V FROM T WHERE K < 20 OR K >= 985"), expected);
    assert_eq!(rows(&db.session("APP"), "SELECT COUNT(*) FROM T").unwrap(), count);
    assert_eq!(count, ["1401"], "1,000 + 1 + 400, the open transaction's delete undone");
    assert_eq!(sorted(&db.session("APP"), "SELECT K FROM T WHERE V = 'later'").len(), 10, "the index, reopened");
    assert!(db.session("APP").physical_plan("SELECT K FROM T WHERE V = 'bulk'", &Params::new()).unwrap().contains("T_V"));
    assert_eq!(rows(&db.session("APP"), "SELECT COUNT(*) FROM T WHERE V = 'bulk'"), Ok(vec!["400".into()]), "rows inserted after it");
    assert_eq!(rows(&db.session("APP"), "SELECT K FROM T WHERE V = 'rolled'"), Ok(vec![]), "entries of what was undone");
    let dup = db.session("APP").run("INSERT INTO T VALUES (2000, 'dup')").err().map(|e| e.code);
    assert_eq!(dup, Some(Code::Constraint), "UNIQUE over a row inserted after it");
    assert_eq!(rows(&db.session("B"), "SELECT K FROM U"), Ok(vec!["7".into()]), "defined after the checkpoint");
    assert_eq!(db.session("APP").run("INSERT INTO T VALUES (5, 'dup')").err().map(|e| e.code), Some(Code::Constraint));
    drop((db, s, later, rolled, open_tx, crashed));
}

/// Verifies: [DESIGN] RULES.md D25, the log is truncated behind the oldest
/// position recovery needs: after checkpoints, segments before it are
/// gone, unless a transaction open at a checkpoint began there; recovery
/// from what is left gives the committed rows.
#[test]
fn the_log_is_truncated_behind_the_checkpoint() {
    let scratch = Scratch::new("checkpoint-truncate");
    let dir = scratch.path("db");
    let segments = |dir: &Path| std::fs::read_dir(dir.join("log")).unwrap().count();
    let (db, _) = open(&dir, i64::MAX);
    define(&db);
    let before = segments(&dir);
    assert!(before > 3, "{before} segments");
    let held = db.session("APP");
    held.run("INSERT INTO T VALUES (5000, 'held')").unwrap();
    let s = db.session("APP");
    for k in 1_000..1_300 {
        s.run(&format!("INSERT INTO T VALUES ({k}, 'more')")).unwrap();
    }
    s.commit().unwrap();
    db.checkpoint().unwrap();
    let while_held = segments(&dir);
    held.commit().unwrap();
    db.checkpoint().unwrap();
    let after = segments(&dir);
    assert!(after < while_held && after <= 2, "{before} then {while_held} then {after}");
    let expected = rows(&db.session("APP"), "SELECT COUNT(*), MAX(K) FROM T").unwrap();
    let (back, opened) = open(&dir, i64::MAX);
    assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
    assert_eq!(rows(&back.session("APP"), "SELECT COUNT(*), MAX(K) FROM T").unwrap(), expected);
    drop((back, held, s, db));
}

/// Verifies: [DESIGN] RULES.md D25, checkpoints taken as the database runs:
/// after `checkpoint_after` row changes, a transaction's end takes one, so
/// recovery reads little of the log; threads writing while checkpoints
/// are taken lose no committed row and keep no uncommitted one.
#[test]
fn checkpoints_are_taken_as_writers_run() {
    let scratch = Scratch::new("checkpoint-threads");
    let dir = scratch.path("db");
    let (crashed, _) = open(&dir, 200);
    define(&crashed);
    let committed = Arc::new(Mutex::new(0_u32));
    let writers: Vec<_> = (0..4_u32)
        .map(|w| {
            let (db, committed) = (crashed.clone(), Arc::clone(&committed));
            thread::spawn(move || {
                let s = db.session("APP");
                for n in 0..300_u32 {
                    s.run(&format!("INSERT INTO T VALUES ({}, 'w{w}')", 10_000 + w * 1_000 + n)).unwrap();
                    if n % 3 == 2 {
                        s.rollback().unwrap();
                    } else {
                        s.commit().unwrap();
                        *committed.lock().unwrap() += 1;
                    }
                }
            })
        })
        .collect();
    let checkpointer = {
        let db = crashed.clone();
        thread::spawn(move || {
            for _ in 0..20 {
                db.checkpoint().unwrap();
            }
        })
    };
    for w in writers {
        w.join().unwrap();
    }
    checkpointer.join().unwrap();
    let written = *committed.lock().unwrap();
    assert_eq!(written, 800);
    // A crash, once no checkpoint is under way or due: a checkpoint taken
    // now waits for the thread's, and leaves none due.
    crashed.checkpoint().unwrap();
    drop(crashed);
    let (db, opened) = open(&dir, 200);
    let Opened::Recovered(report) = opened else { panic!("{opened:?}") };
    assert!(report.records < 2_000, "recovered from a recent checkpoint: {report:?}");
    assert_eq!(rows(&db.session("APP"), "SELECT COUNT(*) FROM T WHERE K >= 10000"), Ok(vec![written.to_string()]));
}

/// Verifies: [DESIGN] RULES.md D25, checkpoints are refused for a database
/// not opened from files, and taken after it is closed and opened again.
#[test]
fn checkpoints_need_files() {
    assert_eq!(Database::new().checkpoint().err().map(|e| e.code), Some(Code::Api));
    let scratch = Scratch::new("checkpoint-files");
    let (db, _) = open(&scratch.path("db"), i64::MAX);
    define(&db);
    db.close().unwrap();
    let (db, opened) = open(&scratch.path("db"), i64::MAX);
    assert_eq!(opened, Opened::Reopened);
    db.checkpoint().unwrap();
    db.session("APP").run("INSERT INTO T VALUES (4000, 'x')").unwrap();
    db.session("APP").run("COMMIT WORK").unwrap();
    drop(db);
    let (db, opened) = open(&scratch.path("db"), i64::MAX);
    assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
    assert_eq!(rows(&db.session("APP"), "SELECT COUNT(*) FROM T"), Ok(vec!["1001".into()]));
}

/// An engine of one table whose rows are as a checkpoint at `durable` left
/// them: what recovery's rules are checked against.
struct Checkpointed {
    rows: Arc<MemoryTable>,
    durable: Lsn,
}

struct CheckpointedTable {
    rows: Arc<MemoryTable>,
    durable: Lsn,
}

impl StorageEngine for Checkpointed {
    fn name(&self) -> &'static str {
        "checkpointed"
    }
    fn create_table(&self, _desc: &TableDesc) -> Result<Box<dyn TableStorage>> {
        Ok(Box::new(CheckpointedTable { rows: Arc::clone(&self.rows), durable: self.durable }))
    }
}

impl TableStorage for CheckpointedTable {
    fn insert(&self, tx: TxId, row: Row) -> Result<RowId> {
        self.rows.insert(tx, row)
    }
    fn update(&self, tx: TxId, id: RowId, row: Row) -> Result<()> {
        self.rows.update(tx, id, row)
    }
    fn delete(&self, tx: TxId, id: RowId) -> Result<()> {
        self.rows.delete(tx, id)
    }
    fn restore(&self, tx: TxId, id: RowId, row: Row) -> Result<()> {
        self.rows.restore(tx, id, row)
    }
    fn get(&self, tx: TxId, id: RowId) -> Result<Option<Row>> {
        self.rows.get(tx, id)
    }
    fn scan(self: Arc<Self>, _: TxId, _: &[usize]) -> Rows {
        self.rows.live_rows()
    }
    fn row_count(&self) -> usize {
        self.rows.row_count()
    }
    fn durable_through(&self) -> Lsn {
        self.durable
    }
    fn capabilities(&self) -> TxCapabilities {
        TxCapabilities::default()
    }
}

/// Verifies: [DESIGN] RULES.md D25, recovery's rules against a table whose
/// pages hold every change logged before L: a change committed before L
/// is left; one made before L by a transaction committing after L is redone
/// (its version unstamped); one after L, committed, redone; one before L of
/// a transaction rolled back after L, or open at the crash, undone; one
/// whose rollback came before L left, even where a later commit changed
/// the row (the rule this replaced restored the rolled-back before-image
/// over it); a statement undone after L undone, the transaction's later
/// changes redone.
#[test]
fn recovery_undoes_and_redoes_what_the_pages_need() {
    let log: Arc<dyn Log> = Arc::new(MemoryLog::new());
    let table = QualifiedName { auth: "A".into(), ident: "T".into() };
    let record = |tx: TxId, change: Change| log.append(&Record { tx, change }.encode()).unwrap();
    let row = |k: i128| vec![number(k)];
    let insert = |tx: TxId, id: RowId, k: i128| record(tx, Change::Insert { table: table.clone(), generation: 0, row: id, after: row(k) });
    let schema = "CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER)".to_string();
    record(NO_TRANSACTION, Change::Schema { sql: schema, owner: "A".into(), engines: vec![("T".into(), "checkpointed".into())], zone: 0 });
    record(10, Change::Insert { table: table.clone(), generation: 0, row: 6, after: row(60) });
    record(10, Change::Commit { at: 1 });
    insert(1, 1, 1);
    record(1, Change::Commit { at: 2 });
    insert(2, 2, 2);
    insert(3, 3, 3);
    insert(4, 4, 4);
    insert(5, 5, 5);
    record(5, Change::Abort);
    record(7, Change::Update { table: table.clone(), generation: 0, row: 6, before: row(60), after: row(61) });
    record(7, Change::Abort);
    record(8, Change::Update { table: table.clone(), generation: 0, row: 6, before: row(60), after: row(62) });
    record(8, Change::Commit { at: 3 });
    let cancelled = insert(11, 8, 8);
    let durable = Lsn(log.end().0);
    // After L.
    record(2, Change::Commit { at: 4 });
    record(3, Change::Abort);
    insert(9, 7, 7);
    record(9, Change::Commit { at: 5 });
    record(11, Change::StatementUndone { from: cancelled });
    insert(11, 9, 9);
    record(11, Change::Commit { at: 6 });
    // The pages at L: what every change before it left.
    let pages = Arc::new(MemoryTable::new());
    for (id, k) in [(1, 1), (2, 2), (3, 3), (4, 4), (6, 62), (8, 8)] {
        pages.restore(NO_TRANSACTION, id, row(k)).unwrap();
    }
    let db = Database::new();
    db.register_engine(Box::new(Checkpointed { rows: Arc::clone(&pages), durable })).unwrap();
    let report = db.recover(log).unwrap();
    let mut found: Vec<(RowId, String)> =
        pages.scan(NO_TRANSACTION, &[0]).map(|r| r.map(|(id, row)| (id, row[0].to_string()))).collect::<Result<_>>().unwrap();
    found.sort();
    let expected: Vec<(RowId, String)> = [(1, "1"), (2, "2"), (6, "62"), (7, "7"), (9, "9")].map(|(id, k)| (id, k.to_string())).to_vec();
    assert_eq!(found, expected);
    assert_eq!((report.undone, report.redone), (3, 3), "undone 3, 4, 8; redone 2, 7, 9: {report:?}");
}

/// Waits up to five seconds for `db` to have taken `n` checkpoints.
fn wait_for_checkpoints(db: &Database, n: u64) -> bool {
    for _ in 0..500 {
        if db.checkpoints_taken() >= n {
            return true;
        }
        thread::sleep(std::time::Duration::from_millis(10));
    }
    false
}

/// Verifies: [DESIGN] RULES.md D25, checkpoints come due: after
/// CHECKPOINT.AFTER_CHANGES row changes, and after CHECKPOINT.AFTER_BYTES written to
/// the tables file's side file, the checkpoint thread takes one; a database
/// not opened from files counts none and has no failure; the thread ends
/// when the database is closed.
#[test]
fn checkpoints_come_due() {
    assert_eq!((Database::new().checkpoints_taken(), Database::new().checkpoint_failure().is_none()), (0, true));
    let scratch = Scratch::new("checkpoint-due");
    let (db, _) = open(&scratch.path("changes"), 50);
    define(&db);
    assert!(wait_for_checkpoints(&db, 1), "1,000 changes, a checkpoint every 50");
    assert!(db.checkpoint_failure().is_none());
    db.close().unwrap();
    let by_bytes = Database::new();
    small_pool(&by_bytes, 32);
    by_bytes.open_files(&os(), &scratch.path("bytes"), &OpenOptions::default()).unwrap();
    due(&by_bytes, i64::MAX, 1);
    by_bytes.schema("CREATE SCHEMA AUTHORIZATION W CREATE TABLE W (S CHAR(240))").unwrap();
    let s = by_bytes.session("W");
    // 480 KB of rows (stored trimmed, so filled), past a pool of 32 pages
    // of 8 KB.
    for n in 0..2_000 {
        s.run(&format!("INSERT INTO W VALUES ('{n:x>240}')")).unwrap();
    }
    s.commit().unwrap();
    assert!(wait_for_checkpoints(&by_bytes, 1), "pages evicted to the side file");
    drop(s);
    drop(by_bytes);
}

/// Verifies: [DESIGN] RULES.md D25, a checkpoint that fails part way: one
/// whose image cannot be recorded, after its tables were committed, leaves
/// them ahead of the last image, which recovery from that image handles;
/// one whose sealed batch cannot be committed has it committed by the
/// next; either way, after a crash, the committed rows are all there.
#[test]
fn a_checkpoint_failing_part_way_loses_nothing() {
    let scratch = Scratch::new("checkpoint-failing");
    let dir = scratch.path("db");
    let (fs, faults) = crate::faults::FaultyFs::with_faults();
    let crashed = Database::new();
    small_pool(&crashed, 32);
    crashed.open_files(&fs, &dir, &OpenOptions::default()).unwrap();
    due(&crashed, i64::MAX, i64::MAX);
    define(&crashed);
    let s = crashed.session("APP");
    s.run("UPDATE T SET V = 'one' WHERE K < 100").unwrap();
    s.commit().unwrap();
    faults.inject(crate::faults::Rule::fail(crate::faults::Op::Write).on("catalog"));
    assert!(crashed.checkpoint().is_err(), "the image cannot be recorded");
    faults.clear();
    s.run("UPDATE T SET V = 'two' WHERE K >= 100 AND K < 200").unwrap();
    s.commit().unwrap();
    faults.inject(crate::faults::Rule::fail(crate::faults::Op::Sync).on("shadow"));
    assert!(crashed.checkpoint().is_err(), "the sealed batch cannot be committed");
    faults.clear();
    s.run("UPDATE T SET V = 'three' WHERE K >= 200 AND K < 300").unwrap();
    s.commit().unwrap();
    crashed.checkpoint().unwrap();
    s.run("DELETE FROM T WHERE K >= 900").unwrap();
    s.commit().unwrap();
    let expected = sorted(&crashed.session("APP"), "SELECT V, COUNT(*) FROM T GROUP BY V");
    let (db, opened) = open(&dir, i64::MAX);
    assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
    assert_eq!(sorted(&db.session("APP"), "SELECT V, COUNT(*) FROM T GROUP BY V"), expected);
    assert_eq!(rows(&db.session("APP"), "SELECT COUNT(*) FROM T"), Ok(vec!["900".into()]));
    drop((db, s, crashed));
}

/// Verifies: [DESIGN] RULES.md D25, D29, a crash loses the collection of
/// versions a reader still held at the last checkpoint, not their index
/// entries' taking back: the checkpoint records them, and recovery takes
/// them back, so the index holds one entry a row.
#[test]
fn a_crash_leaves_no_stale_index_entries() {
    let scratch = Scratch::new("checkpoint-stale");
    let dir = scratch.path("db");
    let (crashed, _) = open(&dir, i64::MAX);
    define(&crashed);
    let reader = crashed.session("APP");
    assert_eq!(rows(&reader, "SELECT COUNT(*) FROM T"), Ok(vec!["1000".into()]), "a snapshot, held open");
    let s = crashed.session("APP");
    for round in 0..3 {
        s.run(&format!("UPDATE T SET V = 'r{round}' WHERE K < 200")).unwrap();
        s.commit().unwrap();
    }
    assert_eq!(crashed.index_entries("T_V").unwrap(), 1_600, "versions the reader may see keep theirs");
    crashed.checkpoint().unwrap();
    let (db, opened) = open(&dir, i64::MAX);
    assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
    assert_eq!(db.index_entries("T_V").unwrap(), 1_000, "one a row");
    assert_eq!(rows(&db.session("APP"), "SELECT COUNT(*) FROM T WHERE V = 'r2'"), Ok(vec!["200".into()]));
    assert!(db.index_entries("NOPE").is_err());
    drop((db, s, reader, crashed));
}

/// Verifies: [DESIGN] RULES.md D25, D29: the stale index entries a
/// checkpoint logs are taken back by recovery from that checkpoint only: a
/// later one that logged its own but failed to record its image is not
/// recovered from, and its entries, of the same versions, are not taken
/// back again, which would take a live version's entry under the same key.
#[test]
fn only_the_recovered_checkpoints_stale_entries_are_taken_back() {
    let scratch = Scratch::new("checkpoint-stale-failed");
    let dir = scratch.path("db");
    let (fs, faults) = crate::faults::FaultyFs::with_faults();
    let crashed = Database::new();
    crashed.open_files(&fs, &dir, &OpenOptions::default()).unwrap();
    due(&crashed, i64::MAX, i64::MAX);
    define(&crashed);
    let reader = crashed.session("APP");
    assert_eq!(rows(&reader, "SELECT COUNT(*) FROM T"), Ok(vec!["1000".into()]), "a snapshot, held open");
    let s = crashed.session("APP");
    // Twenty rows changed and changed back: each live version has the key
    // of the one the reader holds, awaiting collection.
    s.run("UPDATE T SET V = 'x' WHERE K < 200 AND V = 'v3'").unwrap();
    s.commit().unwrap();
    s.run("UPDATE T SET V = 'v3' WHERE K < 200 AND V = 'x'").unwrap();
    s.commit().unwrap();
    assert!(crashed.index_entries("T_V").unwrap() >= 1_020, "the reader's versions keep their entries");
    crashed.checkpoint().unwrap();
    faults.inject(crate::faults::Rule::fail(crate::faults::Op::Write).on("catalog"));
    assert!(crashed.checkpoint().is_err(), "its image cannot be recorded");
    faults.clear();
    let (db, opened) = open(&dir, i64::MAX);
    assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
    assert_eq!(db.index_entries("T_V").unwrap(), 1_000, "one a row");
    assert_eq!(rows(&db.session("APP"), "SELECT K FROM T WHERE V = 'v3' AND K = 103"), Ok(vec!["103".into()]));
    drop((db, s, reader, crashed));
}

/// Verifies: [DESIGN] RULES.md D25: once a sync of the log fails, a
/// database kept in a directory takes no checkpoint, whose image would
/// claim what the log may not hold, and cannot be closed cleanly; opened
/// again, it is recovered from what its log holds, and takes changes again.
#[test]
fn a_failed_log_sync_stops_checkpoints_until_reopened() {
    let scratch = Scratch::new("checkpoint-log-failed");
    let dir = scratch.path("db");
    let (fs, faults) = crate::faults::FaultyFs::with_faults();
    let crashed = Database::new();
    crashed.open_files(&fs, &dir, &OpenOptions::default()).unwrap();
    due(&crashed, i64::MAX, i64::MAX);
    define(&crashed);
    let s = crashed.session("APP");
    s.run("UPDATE T SET V = 'kept' WHERE K < 10").unwrap();
    faults.inject(crate::faults::Rule::fail(crate::faults::Op::Sync).on("/log/"));
    assert_eq!(s.commit().err().map(|e| e.code), Some(Code::Io));
    faults.clear();
    assert!(crashed.log_failure().is_some());
    assert_eq!(crashed.checkpoint().err().map(|e| e.code), Some(Code::Io), "no checkpoint");
    assert_eq!(crashed.close().err().map(|e| e.code), Some(Code::Io), "no clean close");
    drop((s, crashed));
    let (db, opened) = open(&dir, i64::MAX);
    assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
    assert!(db.log_failure().is_none());
    let s = db.session("APP");
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM T WHERE V = 'kept'"), Ok(vec!["10".into()]), "the log held it");
    s.run("UPDATE T SET V = 'again' WHERE K = 0").unwrap();
    s.commit().unwrap();
    db.checkpoint().unwrap();
}

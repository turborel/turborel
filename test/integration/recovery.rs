//! Recovery on start (RULES.md D25): a database rebuilt
//! from its log after a crash holds exactly what was committed, with its
//! definitions, indexes and constraints; engines that keep their own data
//! have uncommitted changes undone; and recovery refuses what it cannot do.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use turborel::catalog::TableDesc;
use turborel::db::{Database, OpenOptions, Opened, RecoveryReport};
use turborel::error::{Code, Result};
use turborel::log::{FileLog, FileLogOptions, Log, MemoryLog};
use turborel::storage::{Row, RowId, Rows, StorageEngine, TableStorage};
use turborel::txn::{Persistence, TxCapabilities, TxId};

use turborel::value::{Decimal, Value};

use crate::common::{Counted, MemoryTable, Scratch, after_crash, crc32c, os, rows, sorted};

fn new_log(dir: &Path) -> Arc<dyn Log> {
    Arc::new(FileLog::create(os(), dir, crc32c(), FileLogOptions::default()).unwrap())
}

/// The log in `dir`, opened again, as after a restart.
fn reopened(dir: &Path) -> Arc<dyn Log> {
    Arc::new(FileLog::open(os(), dir, &[crc32c()], &[], FileLogOptions::default()).unwrap().0)
}

/// A new database recovered from `log`.
fn recovered(log: Arc<dyn Log>) -> (Database, RecoveryReport) {
    let db = Database::new();
    let report = db.recover(log).unwrap();
    (db, report)
}

/// Verifies: [X3.135-1986 §4.16, §8.2 GR3, §8.9]; [DESIGN] RULES.md D25,
/// recovery: after a crash, a database rebuilt from its file log holds the
/// committed transactions' rows and only those: not a rolled-back one's,
/// not a failed statement's, not those of a transaction still open at the
/// crash; its `<schema>`, index and UNIQUE constraint are back, the index
/// rebuilt from the rows; new transactions take ids past the log's; and
/// recovering again, from the log grown since, gives the same rows and the
/// new ones.
#[test]
fn a_crashed_database_recovers_what_was_committed() {
    let scratch = Scratch::new("recovery-crash");
    let dir = scratch.path("log");
    let db = Database::new();
    db.set_log(new_log(&dir)).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER NOT NULL UNIQUE, V CHAR(4))").unwrap();
    db.create_index("T_V", "A.T", &["V"], "hash").unwrap();
    let s = db.session("A");
    for k in 1..=4 {
        s.run(&format!("INSERT INTO T VALUES ({k}, 'v{k}')")).unwrap();
    }
    s.commit().unwrap();
    s.run("UPDATE T SET V = 'new' WHERE K = 2").unwrap();
    s.run("DELETE FROM T WHERE K = 3").unwrap();
    s.commit().unwrap();
    s.run("INSERT INTO T VALUES (9, 'gone')").unwrap();
    s.rollback().unwrap();
    s.run("INSERT INTO T VALUES (5, 'v5')").unwrap();
    assert!(s.run("INSERT INTO T VALUES (5, 'dup')").is_err(), "UNIQUE: undone");
    s.commit().unwrap();
    let expected = sorted(&db.session("A"), "SELECT K, V FROM T");
    // A transaction still open at the crash.
    let open = db.session("A");
    open.run("INSERT INTO T VALUES (7, 'open')").unwrap();
    open.run("UPDATE T SET V = 'open' WHERE K = 1").unwrap();
    let (back, report) = recovered(reopened(&dir));
    assert_eq!(sorted(&back.session("A"), "SELECT K, V FROM T"), expected);
    assert_eq!(expected, ["1,v1  ", "2,new ", "4,v4  ", "5,v5  "]);
    assert_eq!((report.definitions, report.committed, report.not_committed), (2, 3, 2), "the rollback and the open transaction did not");
    assert_eq!(rows(&back.session("A"), "SELECT K FROM T WHERE V = 'new'"), Ok(vec!["2".into()]), "the index works");
    let b = back.session("A");
    assert_eq!(b.run("INSERT INTO T VALUES (1, 'x')").err().map(|e| e.code), Some(Code::Constraint), "UNIQUE is back");
    b.run("INSERT INTO T VALUES (8, 'v8')").unwrap();
    assert!(b.transaction().unwrap() > open.transaction().unwrap(), "past the log's transaction ids");
    b.commit().unwrap();
    drop((b, back));
    let (again, _) = recovered(reopened(&dir));
    assert_eq!(sorted(&again.session("A"), "SELECT K FROM T"), ["1", "2", "4", "5", "8"]);
}

/// Verifies: [DESIGN] RULES.md D25, recovery: tables are recovered into the
/// engines they were placed in, which must be registered before recovery
/// (SQLCODE -910 otherwise).
#[test]
fn tables_return_to_their_engines() {
    let log: Arc<dyn Log> = Arc::new(MemoryLog::new());
    let db = Database::new();
    db.register_engine(Box::new(Shared::new(Persistence::Logged))).unwrap();
    db.place_table("A.T", "shared").unwrap();
    db.set_log(Arc::clone(&log)).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER)").unwrap();
    let missing = Database::new();
    assert_eq!(missing.recover(Arc::clone(&log)).err().map(|e| e.code), Some(Code::Api), "engine \"shared\" not registered");
    let back = Database::new();
    let engine = Shared::new(Persistence::Logged);
    let tables = Arc::clone(&engine.tables);
    back.register_engine(Box::new(engine)).unwrap();
    back.recover(log).unwrap();
    assert!(tables.lock().unwrap().contains_key("T"), "A.T was created in engine \"shared\"");
}

/// An engine whose tables outlive a database: every database it is
/// registered with shares them, as a file-backed engine's would be, found
/// again by name after a crash.
struct Shared {
    persistence: Persistence,
    tables: Arc<Mutex<HashMap<String, Arc<MemoryTable>>>>,
}

impl Shared {
    fn new(persistence: Persistence) -> Shared {
        Shared { persistence, tables: Arc::default() }
    }
}

struct SharedTable {
    rows: Arc<MemoryTable>,
    persistence: Persistence,
}

impl StorageEngine for Shared {
    fn name(&self) -> &'static str {
        "shared"
    }
    fn create_table(&self, desc: &TableDesc) -> Result<Box<dyn TableStorage>> {
        let rows = Arc::clone(self.tables.lock().unwrap().entry(desc.name.ident.clone()).or_default());
        Ok(Box::new(SharedTable { rows, persistence: self.persistence }))
    }
}

impl TableStorage for SharedTable {
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
    fn capabilities(&self) -> TxCapabilities {
        TxCapabilities { persistence: self.persistence, ..TxCapabilities::default() }
    }
}

/// Verifies: [DESIGN] RULES.md D25, recovery: a `Forced` engine keeps its
/// own data, so nothing is redone into it, and the changes of a
/// transaction open at the crash, which it holds, are undone from their
/// before-images: a row it inserted removed, a row it updated put back.
#[test]
fn a_forced_engine_has_uncommitted_changes_undone() {
    let log: Arc<dyn Log> = Arc::new(MemoryLog::new());
    let engine = Shared::new(Persistence::Forced);
    let tables = Arc::clone(&engine.tables);
    let db = Database::new();
    db.register_engine(Box::new(engine)).unwrap();
    db.use_engine("shared").unwrap();
    // A memory log is not durable, which `Strict` refuses for a `Forced`
    // engine's writes.
    db.set_transaction_policy(turborel::txn::Policy::Permissive).unwrap();
    db.set_log(Arc::clone(&log)).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, V CHAR(4))").unwrap();
    let s = db.session("A");
    s.run("INSERT INTO T VALUES (1, 'kept')").unwrap();
    s.commit().unwrap();
    s.run("INSERT INTO T VALUES (2, 'open')").unwrap();
    s.run("UPDATE T SET V = 'open' WHERE K = 1").unwrap();
    // The crash: the engine's tables hold the open transaction's changes.
    let back = Database::new();
    back.register_engine(Box::new(Shared { persistence: Persistence::Forced, tables })).unwrap();
    let report = back.recover(log).unwrap();
    assert_eq!((report.redone, report.undone), (0, 2));
    assert_eq!(sorted(&back.session("A"), "SELECT K, V FROM T"), ["1,kept"]);
}

/// Verifies: [DESIGN] RULES.md D25, recovery: a database that already holds
/// a schema is refused (SQLCODE -910), as is a log holding a malformed
/// record (SQLCODE -916).
#[test]
fn recovery_refuses_what_it_cannot_do() {
    let db = crate::common::setup();
    assert_eq!(db.recover(Arc::new(MemoryLog::new())).err().map(|e| e.code), Some(Code::Api));
    let log = MemoryLog::new();
    log.append(b"not a record").unwrap();
    assert_eq!(Database::new().recover(Arc::new(log)).err().map(|e| e.code), Some(Code::Corrupt));
}

/// Verifies: [X3.135-1986 §4.16]; [DESIGN] RULES.md D25, D38: the row
/// changes recovery reads past WORK_MEMORY are written to spill areas and
/// read back to undo, redo and mend the indexes: committed changes stand,
/// the open transaction's are undone, newest first, and the index holds
/// one entry a row,
/// under its key now.
#[test]
fn recovery_reads_its_changes_a_segment_at_a_time() {
    let scratch = Scratch::new("recovery-spilled");
    let dir = scratch.path("db");
    let open = |counted: &Counted| {
        let db = Database::new();
        db.set_spill(Box::new(counted.clone())).unwrap();
        let opened = db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
        (db, opened)
    };
    let (crashed, _) = open(&Counted::default());
    crashed.set_setting("WORK_MEMORY", &Value::Exact(Decimal::new(64 << 10, 0))).unwrap();
    crashed.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER NOT NULL UNIQUE, V CHARACTER(40))").unwrap();
    crashed.create_index("T_V", "S.T", &["V"], "btree").unwrap();
    crashed.checkpoint().unwrap();
    let (s, open_tx) = (crashed.session("S"), crashed.session("S"));
    for from in (0..3000).step_by(500) {
        let values: Vec<String> = (from..from + 500).map(|k| format!("({k}, 'v{k}')")).collect();
        s.run(&format!("INSERT INTO T VALUES {}", values.join(", "))).unwrap();
    }
    s.commit().unwrap();
    s.run("UPDATE T SET V = 'u' WHERE K < 1500").unwrap();
    s.commit().unwrap();
    // Still open at the crash: its session kept, as the files are reopened.
    // Each of its rows changed twice: undone newest first.
    open_tx.run("UPDATE T SET V = 'open' WHERE K >= 1500").unwrap();
    open_tx.run("UPDATE T SET V = 'again' WHERE K >= 1500").unwrap();
    // Its changes in the pages the checkpoint holds, for recovery to undo.
    crashed.checkpoint().unwrap();
    drop((s, crashed));
    let counted = Counted::default();
    let (db, opened) = open(&counted);
    assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
    assert!(counted.taken() > 0, "its changes spilled");
    let s = db.session("S");
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM T WHERE V = 'u'"), Ok(vec!["1500".into()]));
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM T WHERE V = 'open' OR V = 'again'"), Ok(vec!["0".into()]));
    assert_eq!(rows(&s, "SELECT K FROM T WHERE V = 'v2999'"), Ok(vec!["2999".into()]));
    assert_eq!(db.index_entries("T_V").unwrap(), 3000, "one a row");
    drop(open_tx);
}

/// Verifies: [DESIGN] RULES.md D25, D40, recovery from a damaged or torn
/// log: a database whose log is damaged in its middle, a record failing its
/// checksum before valid ones, is refused as it opens (SQLCODE -916), its
/// log unchanged; one whose last record a crash tore is recovered, the
/// torn record cut, and the bytes cut are reported (`RecoveryReport::cut`)
/// and told in the `opened` event.
///
/// Regression test (R-TEST-7): the damaged log was cut at the damage,
/// committed records after it lost, and no report or event said so.
#[test]
fn a_damaged_log_is_refused_and_a_torn_one_reported() {
    let scratch = Scratch::new("recovery-torn-log");
    let dir = scratch.path("db");
    let crashed = Database::new();
    crashed.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
    crashed.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER)").unwrap();
    let s = crashed.session("A");
    for k in 0..5_i32 {
        s.run(&format!("INSERT INTO T VALUES ({k})")).unwrap();
        s.commit().unwrap();
    }
    let segment = dir.join("log").join("segment-0000000000.log");
    let bytes = std::fs::read(&segment).unwrap();
    let mut damaged = bytes.clone();
    damaged[usize::try_from(turborel::disk::block::HEADER_LEN).unwrap() + 9] ^= 0x55;
    std::fs::write(&segment, &damaged).unwrap();
    let refused = Database::new().open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap_err();
    assert_eq!(refused.code, Code::Corrupt);
    assert!(refused.msg.contains("is damaged at offset"), "{refused}");
    assert_eq!(std::fs::read(&segment).unwrap(), damaged, "unchanged");
    // A record's first 10 bytes written, the crash before the rest.
    let mut torn = bytes.clone();
    torn.extend_from_slice(&200_u32.to_le_bytes());
    torn.extend_from_slice(&[0xAB; 6]);
    std::fs::write(&segment, &torn).unwrap();
    let kept = crate::common::Kept::default();
    let db = Database::new();
    db.set_diagnostics(Box::new(kept.clone()));
    let opened = db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
    let Opened::Recovered(report) = opened else { panic!("{opened:?}") };
    assert_eq!(report.cut, 10);
    assert!(kept.taken().iter().any(|(_, e)| e.contains("Opened(Recovered") && e.contains("cut: 10")), "told");
    assert_eq!(rows(&db.session("A"), "SELECT COUNT(*) FROM T"), Ok(vec!["5".into()]));
    drop((s, crashed));
}

//! Tables damaged by a failed undo (RULES.md D19), and their repair: a
//! transaction a statement of which could not be undone can only roll
//! back; damage blocks checkpoints, so the last one and the log can repair
//! a table's rows, by opening the database again; damage to the index
//! entries alone is repaired at once, the indexes built again from the rows.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use turborel::catalog::{QualifiedName, TableDesc};
use turborel::db::{Database, OpenOptions, Opened};
use turborel::error::{Code, Result, fail};
use turborel::index::{BTreeMethod, Index, IndexMethod, Key, Probe};
use turborel::storage::{Row, RowId, Rows, StorageEngine, TableStorage};
use turborel::txn::{Concurrency, TxCapabilities, TxId, Undo};
use turborel::value::{CompOp, DataType};

use crate::common::{MemoryTable, Scratch, after_crash, exec, rows};

/// An engine whose `delete`, by which the coordinator undoes an insert,
/// fails while `failing` is set.
struct Flaky {
    failing: Arc<AtomicBool>,
}

struct FlakyTable {
    rows: MemoryTable,
    failing: Arc<AtomicBool>,
}

impl StorageEngine for Flaky {
    fn name(&self) -> &'static str {
        "flaky"
    }
    fn create_table(&self, _: &TableDesc) -> Result<Box<dyn TableStorage>> {
        Ok(Box::new(FlakyTable { rows: MemoryTable::new(), failing: Arc::clone(&self.failing) }))
    }
}

impl TableStorage for FlakyTable {
    fn insert(&self, tx: TxId, row: Row) -> Result<RowId> {
        self.rows.insert(tx, row)
    }
    fn update(&self, tx: TxId, id: RowId, row: Row) -> Result<()> {
        self.rows.update(tx, id, row)
    }
    fn delete(&self, tx: TxId, id: RowId) -> Result<()> {
        if self.failing.load(Ordering::SeqCst) {
            return fail(Code::Io, "told to fail");
        }
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
        TxCapabilities { undo: Undo::Log, concurrency: Concurrency::Coordinator, ..TxCapabilities::default() }
    }
}

/// The built-in ordered index method, named "flaky", whose indexes'
/// `remove`, by which the coordinator undoes an entry, fails while
/// `failing` is set.
struct FlakyIndexes {
    inner: BTreeMethod,
    failing: Arc<AtomicBool>,
}

struct FlakyIndex {
    inner: Box<dyn Index>,
    failing: Arc<AtomicBool>,
}

impl IndexMethod for FlakyIndexes {
    fn name(&self) -> &'static str {
        "flaky"
    }
    fn supports(&self, op: CompOp) -> bool {
        self.inner.supports(op)
    }
    fn ordered(&self) -> bool {
        self.inner.ordered()
    }
    fn create(&self, columns: &[DataType]) -> Result<Box<dyn Index>> {
        Ok(Box::new(FlakyIndex { inner: self.inner.create(columns)?, failing: Arc::clone(&self.failing) }))
    }
}

impl Index for FlakyIndex {
    fn insert(&self, key: Key, row: RowId) -> Result<()> {
        self.inner.insert(key, row)
    }
    fn remove(&self, key: &Key, row: RowId) -> Result<()> {
        if self.failing.load(Ordering::SeqCst) {
            return fail(Code::Io, "told to fail");
        }
        self.inner.remove(key, row)
    }
    fn rows(&self, key: &Key) -> Result<Vec<RowId>> {
        self.inner.rows(key)
    }
    fn estimate(&self, probe: &Probe) -> Option<usize> {
        self.inner.estimate(probe)
    }
    fn search(&self, probe: &Probe) -> Result<Vec<(Key, RowId)>> {
        self.inner.search(probe)
    }
}

fn table_u() -> QualifiedName {
    QualifiedName { auth: "X".into(), ident: "U".into() }
}

/// Verifies: [X3.135-1986 §7.3 GR3b i]; [DESIGN] RULES.md D19: a
/// transaction a statement of which could not be undone cannot commit what
/// is left of it: it is rolled back (SQLCODE -913). Its table's rows
/// damaged, in a database kept nowhere, cannot be repaired (SQLCODE -914),
/// as no log holds them, but the table can be dropped.
#[test]
fn rows_left_undone_doom_the_transaction_and_the_table() {
    let failing = Arc::new(AtomicBool::new(false));
    let db = Database::new();
    db.register_engine(Box::new(Flaky { failing: Arc::clone(&failing) })).unwrap();
    db.place_table("X.U", "flaky").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE U (K INTEGER NOT NULL UNIQUE) CREATE TABLE V (K INTEGER)").unwrap();
    let s = db.session("X");
    s.run("INSERT INTO U VALUES (1)").unwrap();
    s.run("INSERT INTO V VALUES (1)").unwrap();
    failing.store(true, Ordering::SeqCst);
    assert_eq!(exec(&s, "INSERT INTO U VALUES (1)"), Err(Code::Damaged));
    failing.store(false, Ordering::SeqCst);
    assert_eq!(s.commit().err().map(|e| e.code), Some(Code::CommitFailed), "rolled back");
    assert_eq!(rows(&db.session("X"), "SELECT K FROM V"), Ok(vec![]), "nothing of it committed");
    let refused = db.repair_table("X.U").unwrap_err();
    assert_eq!(refused.code, Code::Damaged);
    assert!(refused.msg.contains("drop the table"), "{refused}");
    assert_eq!(db.damaged_tables().unwrap(), [table_u()]);
    db.session("X").run("DROP TABLE U CASCADE").unwrap();
    db.session("X").commit().unwrap();
    assert_eq!(db.damaged_tables().unwrap(), Vec::<QualifiedName>::new());
}

/// Verifies: [X3.135-1986 §7.3 GR3b i]; [DESIGN] RULES.md D19: index
/// entries a failed statement left behind damage the table's indexes
/// alone; `repair_table` builds them again from its rows, after which the
/// table takes statements again and its index finds its rows, and only
/// them.
#[test]
fn index_entries_left_undone_are_repaired_at_once() {
    let failing = Arc::new(AtomicBool::new(false));
    let db = Database::new();
    db.register_index_method(Box::new(FlakyIndexes { inner: BTreeMethod::in_memory(), failing: Arc::clone(&failing) })).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE U (K INTEGER NOT NULL UNIQUE, V INTEGER)").unwrap();
    db.create_index("U_V", "X.U", &["V"], "flaky").unwrap();
    let s = db.session("X");
    s.run("INSERT INTO U VALUES (1, 10)").unwrap();
    s.commit().unwrap();
    failing.store(true, Ordering::SeqCst);
    assert_eq!(exec(&s, "INSERT INTO U VALUES (1, 20)"), Err(Code::Damaged), "its entry in U_V left");
    failing.store(false, Ordering::SeqCst);
    s.rollback().unwrap();
    assert_eq!(rows(&s, "SELECT K FROM U"), Err(Code::Damaged));
    assert_eq!(db.index_entries("U_V").unwrap(), 2, "the one left behind");
    db.repair_table("X.U").unwrap();
    assert_eq!(db.damaged_tables().unwrap(), Vec::<QualifiedName>::new());
    assert_eq!(db.index_entries("U_V").unwrap(), 1);
    assert_eq!(rows(&s, "SELECT K FROM U WHERE V = 10"), Ok(vec!["1".into()]));
    s.run("INSERT INTO U VALUES (2, 20)").unwrap();
    s.commit().unwrap();
    db.repair_table("X.U").unwrap();
}

/// Verifies: [DESIGN] RULES.md D19, D25: a damaged table stops checkpoints,
/// and a clean close, so the database's last checkpoint and its log still
/// hold what the table should be; opened again, it is recovered from them,
/// the table repaired.
#[test]
fn damaged_rows_are_repaired_by_opening_again() {
    let scratch = Scratch::new("damage-reopened");
    let dir = scratch.path("db");
    let failing = Arc::new(AtomicBool::new(false));
    let open = || {
        let db = Database::new();
        db.register_engine(Box::new(Flaky { failing: Arc::clone(&failing) })).unwrap();
        let opened = db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
        (db, opened)
    };
    let (db, _) = open();
    db.place_table("X.U", "flaky").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE U (K INTEGER NOT NULL UNIQUE)").unwrap();
    let s = db.session("X");
    s.run("INSERT INTO U VALUES (1)").unwrap();
    s.commit().unwrap();
    failing.store(true, Ordering::SeqCst);
    s.run("INSERT INTO U VALUES (2)").unwrap();
    assert_eq!(exec(&s, "ROLLBACK"), Err(Code::Damaged), "its insert left");
    failing.store(false, Ordering::SeqCst);
    assert!(db.repair_table("X.U").unwrap_err().msg.contains("open the database again"));
    assert_eq!(db.checkpoint().err().map(|e| e.code), Some(Code::Damaged));
    assert_eq!(db.close().err().map(|e| e.code), Some(Code::Damaged));
    drop((s, db));
    let (db, opened) = open();
    assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
    assert_eq!(db.damaged_tables().unwrap(), Vec::<QualifiedName>::new());
    assert_eq!(rows(&db.session("X"), "SELECT K FROM U"), Ok(vec!["1".into()]));
    db.checkpoint().unwrap();
}

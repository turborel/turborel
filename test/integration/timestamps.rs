//! Logical timestamps and the commit protocol for engines that isolate
//! transactions themselves (RULES.md D29): who joins a transaction, the one
//! commit timestamp every participant validates and commits at, a refused
//! validation, and the table lock such engines' writers share.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use turborel::catalog::TableDesc;
use turborel::db::Database;
use turborel::error::{Code, Result, fail};
use turborel::params::Params;
use turborel::storage::{Row, RowId, Rows, StorageEngine, TableStorage};
use turborel::txn::{Concurrency, Timestamp, TxCapabilities, TxId, Undo};

use crate::common::{MemoryTable, count};

/// A transaction call an engine received, with the table it went to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Call {
    Begin(TxId, Timestamp),
    Validate(TxId, Timestamp),
    Commit(TxId, Timestamp),
    Rollback(TxId),
}

/// The calls every table of an `Isolating` engine received, in order.
type Calls = Arc<Mutex<Vec<(String, Call)>>>;

/// An engine that declares it isolates transactions itself (serializably),
/// records the transaction calls it receives, and refuses validation with
/// SQLCODE -911 while `refuse` is set, when it also fails to read the
/// versions an index is built from. Its rows are a `MemoryTable`'s.
struct Isolating {
    calls: Calls,
    refuse: Arc<AtomicBool>,
}

struct IsolatingTable {
    name: String,
    rows: MemoryTable,
    calls: Calls,
    refuse: Arc<AtomicBool>,
}

impl StorageEngine for Isolating {
    fn name(&self) -> &'static str {
        "isolating"
    }
    fn create_table(&self, desc: &TableDesc) -> Result<Box<dyn TableStorage>> {
        Ok(Box::new(IsolatingTable {
            name: desc.name.ident.clone(),
            rows: MemoryTable::new(),
            calls: self.calls.clone(),
            refuse: self.refuse.clone(),
        }))
    }
}

impl IsolatingTable {
    fn record(&self, call: Call) {
        self.calls.lock().unwrap().push((self.name.clone(), call));
    }
}

impl TableStorage for IsolatingTable {
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
    fn versions(self: Arc<Self>, _: Timestamp, _: Timestamp, _: &[usize]) -> Rows {
        if self.refuse.load(Ordering::SeqCst) {
            return Box::new(std::iter::once(fail(Code::Corrupt, "a version cannot be read")));
        }
        self.rows.live_rows()
    }
    fn row_count(&self) -> usize {
        self.rows.row_count()
    }
    fn capabilities(&self) -> TxCapabilities {
        TxCapabilities { undo: Undo::Log, concurrency: Concurrency::Engine { serializable: true }, ..TxCapabilities::default() }
    }
    fn begin(&self, tx: TxId, snapshot: Timestamp) -> Result<()> {
        self.record(Call::Begin(tx, snapshot));
        Ok(())
    }
    fn validate(&self, tx: TxId, at: Timestamp) -> Result<()> {
        self.record(Call::Validate(tx, at));
        if self.refuse.load(Ordering::SeqCst) { fail(Code::Conflict, "a row it read was changed") } else { Ok(()) }
    }
    fn commit(&self, tx: TxId, at: Timestamp) {
        self.record(Call::Commit(tx, at));
    }
    fn rollback(&self, tx: TxId) -> Result<()> {
        self.record(Call::Rollback(tx));
        Ok(())
    }
}

/// A database with schema I and tables A and B in an `Isolating` engine,
/// with its record of calls and its refusal switch.
fn isolating() -> (Database, Calls, Arc<AtomicBool>) {
    let db = Database::new();
    let calls = Calls::default();
    let refuse = Arc::new(AtomicBool::new(false));
    db.register_engine(Box::new(Isolating { calls: calls.clone(), refuse: refuse.clone() })).unwrap();
    db.use_engine("isolating").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION I CREATE TABLE A (K INTEGER NOT NULL) CREATE TABLE B (K INTEGER NOT NULL)").unwrap();
    (db, calls, refuse)
}

/// The calls recorded so far, and none afterwards.
fn take(calls: &Calls) -> Vec<(String, Call)> {
    std::mem::take(&mut *calls.lock().unwrap())
}

fn at(table: &str, call: Call) -> (String, Call) {
    (table.to_owned(), call)
}

/// Verifies: [DESIGN] RULES.md D29: a transaction that only reads a table
/// whose engine isolates transactions joins it, and commits at its
/// snapshot, with no validation: it read one committed state. A writer's
/// commit takes a timestamp later than its snapshot, and every participant
/// validates and commits at that one timestamp. Ids follow the order
/// transactions begin; one that begins after a commit reads past it.
#[test]
fn participants_commit_at_one_timestamp() {
    let (db, calls, _) = isolating();
    let s = db.session("I");
    assert_eq!(count(&s, "SELECT K FROM A"), 0);
    s.commit().unwrap();
    let read = take(&calls);
    let [(_, Call::Begin(t, snapshot)), (_, Call::Commit(..))] = read.as_slice() else { panic!("a reader joins and commits: {read:?}") };
    assert_eq!(read, vec![at("A", Call::Begin(*t, *snapshot)), at("A", Call::Commit(*t, *snapshot))], "at its snapshot");

    s.run("INSERT INTO A VALUES (1)").unwrap();
    s.run("INSERT INTO B VALUES (1)").unwrap();
    s.run("INSERT INTO A VALUES (2)").unwrap();
    s.commit().unwrap();
    let written = take(&calls);
    let Some((_, Call::Begin(t2, s2))) = written.first() else { panic!("the writer joins A first: {written:?}") };
    let Some((_, Call::Commit(_, c2))) = written.last() else { panic!("and commits last: {written:?}") };
    assert!(t2 > t && c2 > s2, "ids in the order transactions begin; the commit later than the snapshot");
    let expected = [
        at("A", Call::Begin(*t2, *s2)),
        at("B", Call::Begin(*t2, *s2)),
        at("A", Call::Validate(*t2, *c2)),
        at("B", Call::Validate(*t2, *c2)),
        at("A", Call::Commit(*t2, *c2)),
        at("B", Call::Commit(*t2, *c2)),
    ];
    assert_eq!(written, expected, "each table joins once, and all validate before any commits");
    assert_eq!(count(&s, "SELECT K FROM A"), 2);
    let after = take(&calls);
    let Some((_, Call::Begin(_, s3))) = after.first() else { panic!("a reader joins: {after:?}") };
    assert!(s3 >= c2, "a transaction that begins after a commit reads at or past it");
}

/// Verifies: [X3.135-1986 §8.2]; [DESIGN] RULES.md D29: an engine refusing
/// validation with a conflict fails COMMIT with SQLCODE -911, and the
/// transaction is rolled back in every participant, committed in none.
#[test]
fn a_refused_validation_rolls_back_every_participant() {
    let (db, calls, refuse) = isolating();
    let s = db.session("I");
    s.run("INSERT INTO A VALUES (1)").unwrap();
    s.run("INSERT INTO B VALUES (1)").unwrap();
    refuse.store(true, Ordering::SeqCst);
    assert_eq!(s.commit().map_err(|e| e.code), Err(Code::Conflict));
    refuse.store(false, Ordering::SeqCst);
    let ended = take(&calls);
    assert!(ended.iter().all(|(_, c)| !matches!(c, Call::Commit(..))), "nothing committed: {ended:?}");
    for table in ["A", "B"] {
        assert!(ended.iter().any(|(t, c)| t == table && matches!(c, Call::Rollback(_))), "{table} rolled back: {ended:?}");
    }
    assert_eq!(count(&s, "SELECT K FROM A") + count(&s, "SELECT K FROM B"), 0, "the inserts were undone");
    assert_eq!(db.active_transactions().unwrap(), 1, "only the reading transaction is open");
}

/// Verifies: [DESIGN] RULES.md D29: writers of a table whose engine isolates
/// transactions share its lock, so they do not exclude each other; an index
/// created while they are open refuses their commits (SQLCODE -911), as it
/// has no entries for what they wrote, and their rows are written again.
#[test]
fn writers_of_an_isolating_engine_share_the_table() {
    let (db, _, _) = isolating();
    let (first, second) = (db.session("I"), db.session("I"));
    first.run("INSERT INTO A VALUES (1)").unwrap();
    second.run("INSERT INTO A VALUES (2)").unwrap();
    db.create_index("A_K", "I.A", &["K"], "btree").unwrap();
    assert_eq!(first.commit().map_err(|e| e.code), Err(Code::Conflict));
    assert_eq!(second.commit().map_err(|e| e.code), Err(Code::Conflict));
    second.run("INSERT INTO A VALUES (2)").unwrap();
    second.commit().unwrap();
    assert_eq!(count(&first, "SELECT K FROM A WHERE K = 2"), 1);
}

/// Verifies: [EXTENSION] RULES.md D27: an index whose table's versions
/// cannot be read is not built: `create_index` reports the table's error,
/// and the index leaves the catalog, so no query reads it and it can be
/// created again.
#[test]
fn a_failed_index_build_leaves_no_index() {
    let (db, _, refuse) = isolating();
    let s = db.session("I");
    s.run("INSERT INTO A VALUES (1)").unwrap();
    s.commit().unwrap();
    refuse.store(true, Ordering::SeqCst);
    assert_eq!(db.create_index("A_K", "I.A", &["K"], "btree").map_err(|e| e.code), Err(Code::Corrupt));
    refuse.store(false, Ordering::SeqCst);
    assert!(!s.physical_plan("SELECT K FROM A WHERE K = 1", &Params::new()).unwrap().contains("A_K"), "not read");
    s.run("INSERT INTO A VALUES (2)").unwrap();
    s.commit().unwrap();
    db.create_index("A_K", "I.A", &["K"], "btree").unwrap();
    assert_eq!(count(&s, "SELECT K FROM A WHERE K = 2"), 1);
}

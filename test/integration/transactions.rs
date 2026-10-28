//! Transactions [X3.135-1986 §4.16, §7.1, §7.3, §8.2, §8.9], and storage engines
//! with different transaction capabilities.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use turborel::catalog::{QualifiedName, TableDesc};
use turborel::db::{Completion, Database, Session};
use turborel::error::{Code, Result, fail};
use turborel::params::Params;
use turborel::storage::{Row, RowId, Rows, StorageEngine, TableStorage};
use turborel::txn::{Concurrency, Policy, SessionEnd, Timestamp, TxCapabilities, TxId, Undo};

use crate::common::{MemoryTable, big, count, exec, rows, setup};

/// Verifies: [X3.135-1986 §4.16] a transaction sees its own changes.
#[test]
fn a_transaction_sees_its_own_changes_4_16() {
    let db = setup();
    let s = db.session("APP");
    s.run("INSERT INTO T (ID) VALUES (10)").unwrap();
    assert_eq!(count(&s, "SELECT ID FROM T"), 4); // its own changes are visible [X3.135-1986 §4.16]
}

/// Verifies: [X3.135-1986 §8.9 GR1] ROLLBACK WORK cancels the transaction's changes.
#[test]
fn rollback_cancels_the_changes_8_9() {
    let db = setup();
    let s = db.session("APP");
    s.run("INSERT INTO T (ID) VALUES (10)").unwrap();
    assert_eq!(exec(&s, "ROLLBACK WORK"), Ok(Completion::Success));
    assert_eq!(count(&s, "SELECT ID FROM T"), 3); // [X3.135-1986 §8.9 GR1]
}

/// Verifies: [X3.135-1986 §4.16, §8.2] after COMMIT WORK, a ROLLBACK cannot cancel the committed changes.
#[test]
fn committed_changes_survive_a_rollback_8_2() {
    let db = setup();
    let s = db.session("APP");
    s.run("INSERT INTO T (ID) VALUES (10)").unwrap();
    assert_eq!(exec(&s, "COMMIT WORK"), Ok(Completion::Success));
    exec(&s, "ROLLBACK WORK").unwrap();
    assert_eq!(count(&s, "SELECT ID FROM T"), 4); // committed changes cannot be cancelled [X3.135-1986 §4.16]
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §14.3 F, §14.4 F]: WORK, which
/// SQL-86 requires [X3.135-1986 §8.2 F, §8.9 F], is optional after COMMIT
/// and ROLLBACK; nothing else may follow them.
#[test]
fn commit_and_rollback_take_work_or_not_14_3_14_4() {
    let db = setup();
    let s = db.session("APP");
    s.run("INSERT INTO T (ID) VALUES (10)").unwrap();
    assert_eq!(exec(&s, "COMMIT"), Ok(Completion::Success));
    s.run("DELETE FROM T WHERE ID = 10").unwrap();
    assert_eq!(exec(&s, "ROLLBACK"), Ok(Completion::Success));
    assert_eq!(count(&s, "SELECT ID FROM T WHERE ID = 10"), 1);
    assert_eq!(exec(&s, "COMMIT TRANSACTION"), Err(Code::Syntax));
}

/// Verifies: [X3.135-1986 §8.9 GR1]; [DESIGN] index undo: on the row
/// store, which undoes its own changes, and on an engine whose changes the
/// coordinator's undo log undoes (`Undo::Log`): updates, deletes and inserts
/// are cancelled, and every index agrees with the table again.
#[test]
fn rollback_restores_rows_and_indexes() {
    for db in [big(), crate::common::locking_big()] {
        let s = db.session("APP");
        let ids = |s: &Session| rows(s, "SELECT ID FROM T WHERE ID > 55").unwrap();
        let before = ids(&s);
        exec(&s, "UPDATE T SET ID = ID + 100 WHERE ID > 55").unwrap();
        exec(&s, "DELETE FROM T WHERE ID = 57").unwrap();
        s.run("INSERT INTO T (ID) VALUES (99)").unwrap();
        s.rollback().unwrap();
        // The btree on ID answers this query; it must agree with the table again.
        assert!(s.physical_plan("SELECT ID FROM T WHERE ID > 55", &Params::new()).unwrap().contains("IndexScan"));
        assert_eq!(ids(&s), before);
        assert_eq!(count(&s, "SELECT ID FROM T"), 60);
    }
}

/// Verifies: [X3.135-1986 §7.3 GR3b, §4.5], on the row store and on an
/// engine undone by the coordinator's undo log.
#[test]
fn a_failed_statement_is_undone_7_3_gr3b() {
    for db in [setup(), crate::common::locking_setup()] {
        let s = db.session("APP");
        s.run("INSERT INTO T (ID) VALUES (10)").unwrap();
        // Sets every ID to 5: violates UNIQUE, so the whole statement is undone...
        assert_eq!(exec(&s, "UPDATE T SET ID = 5"), Err(Code::Constraint));
        assert_eq!(rows(&s, "SELECT ID FROM T ORDER BY 1"), Ok(vec!["1".into(), "2".into(), "3".into(), "10".into()]));
        // ...but the transaction and its earlier changes continue.
        assert!(s.transaction().is_some());
        // UNIQUE is checked after the statement [X3.135-1986 §4.5]: passing through
        // duplicate keys while it runs is fine.
        assert_eq!(exec(&s, "UPDATE T SET ID = ID + 1"), Ok(Completion::Success));
        assert_eq!(rows(&s, "SELECT ID FROM T ORDER BY 1"), Ok(vec!["2".into(), "3".into(), "4".into(), "11".into()]));
    }
}

/// Verifies: [X3.135-1986 §4.16], for an engine isolated by the
/// coordinator's table locks (`test/integration/mvcc.rs` for the row
/// store's versions).
#[test]
fn isolation_between_sessions_4_16() {
    let db = crate::common::locking_setup();
    let a = db.session("APP");
    let b = db.session("APP");
    // A's uncommitted insert is invisible to B: B's read is refused, not served stale.
    a.run("INSERT INTO T (ID) VALUES (10)").unwrap();
    assert_eq!(b.open_cursor("SELECT ID FROM T").err().map(|e| e.code), Some(Code::Conflict));
    // B's failed statement left B's transaction usable.
    assert_eq!(count(&b, "SELECT ID FROM U"), 2);
    a.commit().unwrap();
    assert_eq!(count(&b, "SELECT ID FROM T"), 4);
    // Now B holds a read lock on T until it ends: A cannot change T...
    assert_eq!(exec(&a, "DELETE FROM T WHERE ID = 10"), Err(Code::Conflict));
    // ...so B's reads are reproducible within its transaction.
    assert_eq!(count(&b, "SELECT ID FROM T"), 4);
    b.commit().unwrap();
    assert_eq!(exec(&a, "DELETE FROM T WHERE ID = 10"), Ok(Completion::Success));
    // Readers share.
    a.commit().unwrap();
    assert_eq!(count(&a, "SELECT ID FROM T"), 3);
    assert_eq!(count(&b, "SELECT ID FROM T"), 3);
}

/// Verifies: [X3.135-1986 §7.1 GR2]; [DESIGN] changing the optimizer or cost
/// model leaves no borrow behind, so a session dropped afterwards still ends
/// its transaction and releases its locks.
///
/// Regression test: `optimizer_mut()`
/// returned a guard, and a session dropped while it was held kept its
/// transaction forever. The closure-style accessors make that unwritable.
#[test]
fn a_session_dropped_after_optimizer_changes_ends_its_transaction() {
    let db = setup();
    let s = db.session("APP");
    s.run("INSERT INTO T (ID) VALUES (10)").unwrap();
    let rule_count = db.with_optimizer(|o| o.rules().len()).unwrap();
    db.with_cost_model(|c| c.sort_compare = 1.0).unwrap();
    drop(s);
    assert!(rule_count > 0);
    assert_eq!(db.active_transactions().unwrap(), 0);
    assert_eq!(count(&db.session("APP"), "SELECT ID FROM T"), 4);
}

/// Verifies: [X3.135-1986 §7.1 GR2]
#[test]
fn session_end_7_1_gr2() {
    let db = setup();
    // Default: commit when the session ends.
    db.session("APP").run("INSERT INTO T (ID) VALUES (10)").unwrap();
    assert_eq!(count(&db.session("APP"), "SELECT ID FROM T"), 4);
    db.set_session_end(SessionEnd::Rollback).unwrap();
    db.session("APP").run("INSERT INTO T (ID) VALUES (11)").unwrap();
    assert_eq!(count(&db.session("APP"), "SELECT ID FROM T"), 4);
    assert_eq!(db.active_transactions().unwrap(), 0);
}

/// An engine that keeps its own undo (`Undo::Native`), and whose validation
/// can be made to fail, to exercise two-phase commit.
struct NativeEngine {
    fail_validate: Arc<AtomicBool>,
}

enum NativeUndo {
    Delete(RowId),
    Put(RowId, Row),
    Restore(RowId, Row),
}

struct NativeTable {
    rows: MemoryTable,
    log: Mutex<Vec<NativeUndo>>,
    fail_validate: Arc<AtomicBool>,
}

impl StorageEngine for NativeEngine {
    fn name(&self) -> &'static str {
        "native"
    }
    fn create_table(&self, _: &TableDesc) -> Result<Box<dyn TableStorage>> {
        Ok(Box::new(NativeTable { rows: MemoryTable::new(), log: Mutex::new(Vec::new()), fail_validate: self.fail_validate.clone() }))
    }
}

impl TableStorage for NativeTable {
    fn insert(&self, tx: TxId, row: Row) -> Result<RowId> {
        let id = self.rows.insert(tx, row)?;
        self.log.lock().unwrap().push(NativeUndo::Delete(id));
        Ok(id)
    }
    fn update(&self, tx: TxId, id: RowId, row: Row) -> Result<()> {
        let old = self.rows.get(tx, id)?.unwrap();
        self.rows.update(tx, id, row)?;
        self.log.lock().unwrap().push(NativeUndo::Put(id, old));
        Ok(())
    }
    fn delete(&self, tx: TxId, id: RowId) -> Result<()> {
        let old = self.rows.get(tx, id)?.unwrap();
        self.rows.delete(tx, id)?;
        self.log.lock().unwrap().push(NativeUndo::Restore(id, old));
        Ok(())
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
        TxCapabilities { undo: Undo::Native, concurrency: Concurrency::Coordinator, ..TxCapabilities::default() }
    }
    fn savepoint(&self, _: TxId) -> u64 {
        self.log.lock().unwrap().len() as u64
    }
    fn rollback_to(&self, tx: TxId, sp: u64) -> Result<()> {
        while self.log.lock().unwrap().len() as u64 > sp {
            let undo = self.log.lock().unwrap().pop().unwrap();
            match undo {
                NativeUndo::Delete(id) => self.rows.delete(tx, id),
                NativeUndo::Put(id, row) => self.rows.update(tx, id, row),
                NativeUndo::Restore(id, row) => self.rows.restore(tx, id, row),
            }?;
        }
        Ok(())
    }
    fn validate(&self, _: TxId, _: Timestamp) -> Result<()> {
        if self.fail_validate.load(Ordering::SeqCst) { fail(Code::Data, "disk full") } else { Ok(()) }
    }
    fn commit(&self, _: TxId, _: Timestamp) {
        self.log.lock().unwrap().clear();
    }
    fn rollback(&self, tx: TxId) -> Result<()> {
        self.rollback_to(tx, 0)
    }
}

/// Verifies: [X3.135-1986 §8.2]; [DESIGN] two-phase commit
#[test]
fn two_phase_commit_across_engines() {
    let db = setup();
    let fail_validate = Arc::new(AtomicBool::new(false));
    db.register_engine(Box::new(NativeEngine { fail_validate: fail_validate.clone() })).unwrap();
    // One schema, two engines: LOG keeps its own undo, ACCT uses the coordinator's.
    db.place_table("N.LOG", "native").unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION N
           CREATE TABLE LOG (MSG CHAR(10) NOT NULL UNIQUE)
           CREATE TABLE ACCT (ID INT NOT NULL, BAL INT)",
    )
    .unwrap();
    let n = db.session("N");
    n.run("INSERT INTO LOG VALUES ('FIRST')").unwrap();
    n.run("INSERT INTO ACCT VALUES (1, 100)").unwrap();
    // Statement atomicity through the engine's own savepoint, and through the
    // coordinator's undo log for the UNIQUE index: the duplicate is undone.
    assert_eq!(n.run("INSERT INTO LOG VALUES ('FIRST')").map_err(|e| e.code), Err(Code::Constraint));
    assert_eq!(count(&n, "SELECT MSG FROM LOG"), 1);
    n.commit().unwrap();

    // One transaction writes to both engines. LOG cannot validate, so
    // neither commits: ACCT's change is rolled back as well.
    n.run("INSERT INTO LOG VALUES ('SECOND')").unwrap();
    exec(&n, "UPDATE ACCT SET BAL = BAL - 30").unwrap();
    fail_validate.store(true, Ordering::SeqCst);
    assert_eq!(n.commit().map_err(|e| e.code), Err(Code::CommitFailed));
    fail_validate.store(false, Ordering::SeqCst);
    assert_eq!(rows(&n, "SELECT MSG FROM LOG"), Ok(vec!["FIRST     ".into()]));
    assert_eq!(rows(&n, "SELECT BAL FROM ACCT"), Ok(vec!["100".into()]));
    // Both work again, and the UNIQUE index was rolled back with LOG's rows.
    n.run("INSERT INTO LOG VALUES ('SECOND')").unwrap();
    exec(&n, "UPDATE ACCT SET BAL = BAL - 30").unwrap();
    n.commit().unwrap();
    assert_eq!(count(&n, "SELECT MSG FROM LOG"), 2);
    assert_eq!(rows(&n, "SELECT BAL FROM ACCT"), Ok(vec!["70".into()]));
}

/// An engine with native undo whose `begin` always fails, and which counts
/// the transaction calls it receives afterwards.
struct BeginFails {
    later_calls: Arc<AtomicUsize>,
}
struct BeginFailsTable {
    rows: MemoryTable,
    later_calls: Arc<AtomicUsize>,
}
impl StorageEngine for BeginFails {
    fn name(&self) -> &'static str {
        "begin_fails"
    }
    fn create_table(&self, _: &TableDesc) -> Result<Box<dyn TableStorage>> {
        Ok(Box::new(BeginFailsTable { rows: MemoryTable::new(), later_calls: self.later_calls.clone() }))
    }
}
impl BeginFailsTable {
    fn called(&self) {
        self.later_calls.fetch_add(1, Ordering::SeqCst);
    }
}
impl TableStorage for BeginFailsTable {
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
        TxCapabilities { undo: Undo::Native, concurrency: Concurrency::Coordinator, ..TxCapabilities::default() }
    }
    fn begin(&self, _: TxId, _: Timestamp) -> Result<()> {
        fail(Code::Data, "cannot begin")
    }
    fn validate(&self, _: TxId, _: Timestamp) -> Result<()> {
        self.called();
        Ok(())
    }
    fn commit(&self, _: TxId, _: Timestamp) {
        self.called();
    }
    fn rollback(&self, _: TxId) -> Result<()> {
        self.called();
        Ok(())
    }
}

/// Verifies: [DESIGN] two-phase commit: an engine joins a transaction only once
/// its `begin` succeeded, so a failed begin leaves it out of commit and rollback.
///
/// Regression test: the table was recorded
/// as written before `begin`, so commit and rollback reached an engine that
/// had never begun the transaction.
#[test]
fn an_engine_whose_begin_failed_is_not_asked_to_end_the_transaction() {
    let db = setup();
    let later_calls = Arc::new(AtomicUsize::new(0));
    db.register_engine(Box::new(BeginFails { later_calls: later_calls.clone() })).unwrap();
    db.place_table("B.LOG", "begin_fails").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE LOG (MSG CHAR(10))").unwrap();
    let s = db.session("B");
    assert_eq!(s.run("INSERT INTO LOG VALUES ('X')").map_err(|e| e.code), Err(Code::Data));
    s.rollback().unwrap();
    s.run("INSERT INTO LOG VALUES ('Y')").unwrap_err();
    s.commit().unwrap();
    assert_eq!(later_calls.load(Ordering::SeqCst), 0);
}

/// An engine whose undo always fails: with `Undo::Native` its `rollback_to`
/// and `rollback`, with `Undo::Log` its `delete`, which the coordinator uses
/// to undo an insert.
struct UndoFails {
    undo: Undo,
}
struct UndoFailsTable {
    rows: MemoryTable,
    undo: Undo,
}
impl StorageEngine for UndoFails {
    fn name(&self) -> &'static str {
        "undo_fails"
    }
    fn create_table(&self, _: &TableDesc) -> Result<Box<dyn TableStorage>> {
        Ok(Box::new(UndoFailsTable { rows: MemoryTable::new(), undo: self.undo }))
    }
}
impl TableStorage for UndoFailsTable {
    fn insert(&self, tx: TxId, row: Row) -> Result<RowId> {
        self.rows.insert(tx, row)
    }
    fn update(&self, tx: TxId, id: RowId, row: Row) -> Result<()> {
        self.rows.update(tx, id, row)
    }
    fn delete(&self, _: TxId, _: RowId) -> Result<()> {
        fail(Code::Data, "cannot delete")
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
        TxCapabilities { undo: self.undo, concurrency: Concurrency::Coordinator, ..TxCapabilities::default() }
    }
    fn rollback_to(&self, _: TxId, _: u64) -> Result<()> {
        fail(Code::Data, "cannot roll back to a savepoint")
    }
    fn rollback(&self, _: TxId) -> Result<()> {
        fail(Code::Data, "cannot roll back")
    }
}

/// Database `setup()` plus schema X with table U (K NOT NULL UNIQUE) in an
/// `UndoFails` engine of the given kind.
fn with_failing_undo(undo: Undo) -> Database {
    let db = setup();
    db.register_engine(Box::new(UndoFails { undo })).unwrap();
    db.place_table("X.U", "undo_fails").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE U (K INTEGER NOT NULL UNIQUE)").unwrap();
    db
}

/// Verifies: [X3.135-1986 §7.3 GR3b i]; [DESIGN] RULES.md D19: when a failed
/// statement cannot be undone, the statement reports SQLCODE -914 naming its
/// own failure, the table is damaged and refused afterwards, and other tables
/// are unaffected. Both kinds of undo, the coordinator's log and an engine's own.
///
/// Regression test: undo failures were
/// dropped, leaving a half-undone table with nothing reported.
#[test]
fn a_statement_whose_undo_fails_damages_its_table() {
    for undo in [Undo::Log, Undo::Native] {
        let db = with_failing_undo(undo);
        let s = db.session("X");
        s.run("INSERT INTO U VALUES (1)").unwrap();
        let failure = s.run("INSERT INTO U VALUES (1)").unwrap_err();
        assert_eq!(failure.code, Code::Damaged, "{undo:?}");
        assert!(failure.msg.contains("§6.4 GR2"), "{undo:?}: {failure}"); // names the UNIQUE violation
        assert_eq!(db.damaged_tables().unwrap(), vec![QualifiedName { auth: "X".into(), ident: "U".into() }], "{undo:?}");
        assert_eq!(rows(&s, "SELECT K FROM U"), Err(Code::Damaged), "{undo:?}");
        assert_eq!(count(&db.session("APP"), "SELECT ID FROM T"), 3, "{undo:?}");
    }
}

/// Verifies: [X3.135-1986 §8.9 GR1, GR3]; [DESIGN] RULES.md D19: a ROLLBACK
/// that cannot undo a change reports SQLCODE -914 and still ends the
/// transaction; the table is damaged.
///
/// Regression test: undo failures were dropped, with nothing reported.
#[test]
fn a_rollback_whose_undo_fails_is_reported_and_ends_the_transaction() {
    let db = with_failing_undo(Undo::Native);
    let s = db.session("X");
    s.run("INSERT INTO U VALUES (1)").unwrap();
    assert_eq!(exec(&s, "ROLLBACK WORK"), Err(Code::Damaged));
    assert_eq!(s.transaction(), None);
    assert_eq!(db.damaged_tables().unwrap().len(), 1);
}

/// An engine that cannot cancel changes (`Undo::Unsupported`).
struct AppendOnly;
struct AppendOnlyTable(MemoryTable);
impl StorageEngine for AppendOnly {
    fn name(&self) -> &'static str {
        "append_only"
    }
    fn create_table(&self, _: &TableDesc) -> Result<Box<dyn TableStorage>> {
        Ok(Box::new(AppendOnlyTable(MemoryTable::new())))
    }
}
impl TableStorage for AppendOnlyTable {
    fn insert(&self, tx: TxId, row: Row) -> Result<RowId> {
        self.0.insert(tx, row)
    }
    fn update(&self, tx: TxId, id: RowId, row: Row) -> Result<()> {
        self.0.update(tx, id, row)
    }
    fn delete(&self, tx: TxId, id: RowId) -> Result<()> {
        self.0.delete(tx, id)
    }
    fn restore(&self, tx: TxId, id: RowId, row: Row) -> Result<()> {
        self.0.restore(tx, id, row)
    }
    fn get(&self, tx: TxId, id: RowId) -> Result<Option<Row>> {
        self.0.get(tx, id)
    }
    fn scan(self: Arc<Self>, _: TxId, _: &[usize]) -> Rows {
        self.0.live_rows()
    }
    fn row_count(&self) -> usize {
        self.0.row_count()
    }
    fn capabilities(&self) -> TxCapabilities {
        TxCapabilities { undo: Undo::Unsupported, concurrency: Concurrency::Coordinator, ..TxCapabilities::default() }
    }
}

/// Verifies: [X3.135-1986 §3.4]; [DESIGN] transaction `Policy`. Strict refuses
/// writes an engine cannot undo. Permissive allows them, which §3.4 permits: "An
/// implementation remains conforming even if it provides user options to process
/// ... conforming SQL language in a nonconforming manner."
#[test]
fn policy_for_engines_that_cannot_undo() {
    let db = Database::new();
    db.register_engine(Box::new(AppendOnly)).unwrap();
    db.use_engine("append_only").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE EVENTS (E INT)").unwrap();
    let s = db.session("A");
    // Strict (the default): writes are refused, reads are fine.
    assert_eq!(s.run("INSERT INTO EVENTS VALUES (1)").map_err(|e| e.code), Err(Code::Policy));
    assert_eq!(count(&s, "SELECT E FROM EVENTS"), 0);
    drop(s);
    // Permissive [X3.135-1986 §3.4]: allowed, and a rollback cannot take it back.
    db.set_transaction_policy(Policy::Permissive).unwrap();
    let s = db.session("A");
    s.run("INSERT INTO EVENTS VALUES (1)").unwrap();
    s.rollback().unwrap();
    assert_eq!(count(&s, "SELECT E FROM EVENTS"), 1);
}

/// An engine that isolates transactions itself, but not serializably.
struct Snapshotting;

/// Its tables: `MemoryTable`s that declare so.
struct SnapshottingTable(MemoryTable);

impl StorageEngine for Snapshotting {
    fn name(&self) -> &'static str {
        "snapshotting"
    }
    fn create_table(&self, _: &TableDesc) -> Result<Box<dyn TableStorage>> {
        Ok(Box::new(SnapshottingTable(MemoryTable::new())))
    }
}

impl TableStorage for SnapshottingTable {
    fn insert(&self, tx: TxId, row: Row) -> Result<RowId> {
        self.0.insert(tx, row)
    }
    fn update(&self, tx: TxId, id: RowId, row: Row) -> Result<()> {
        self.0.update(tx, id, row)
    }
    fn delete(&self, tx: TxId, id: RowId) -> Result<()> {
        self.0.delete(tx, id)
    }
    fn restore(&self, tx: TxId, id: RowId, row: Row) -> Result<()> {
        self.0.restore(tx, id, row)
    }
    fn get(&self, tx: TxId, id: RowId) -> Result<Option<Row>> {
        self.0.get(tx, id)
    }
    fn scan(self: Arc<Self>, _: TxId, _: &[usize]) -> Rows {
        self.0.live_rows()
    }
    fn row_count(&self) -> usize {
        self.0.row_count()
    }
    fn capabilities(&self) -> TxCapabilities {
        TxCapabilities { concurrency: Concurrency::Engine { serializable: false }, ..TxCapabilities::default() }
    }
}

/// Verifies: [X3.135-1986 §4.16]; [DESIGN] RULES.md D29, transaction policy:
/// under `Strict`, a table whose engine isolates transactions but not
/// serializably is refused for reading and for writing (SQLCODE -912);
/// under `Permissive` it is used.
#[test]
fn strict_refuses_an_engine_that_is_not_serializable() {
    let db = Database::new();
    db.register_engine(Box::new(Snapshotting)).unwrap();
    db.use_engine("snapshotting").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER)").unwrap();
    let s = db.session("A");
    assert_eq!(exec(&s, "INSERT INTO T VALUES (1)"), Err(Code::Policy));
    assert_eq!(rows(&s, "SELECT K FROM T"), Err(Code::Policy));
    db.set_transaction_policy(Policy::Permissive).unwrap();
    s.run("INSERT INTO T VALUES (1)").unwrap();
    assert_eq!(rows(&s, "SELECT K FROM T"), Ok(vec!["1".into()]));
}

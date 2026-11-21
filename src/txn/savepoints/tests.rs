//! Unit tests of a transaction's savepoints: going back to one across
//! statements in an engine that undoes its own changes, an engine joined
//! after one, the statement's bookkeeping clamped, a mark undone taking
//! the savepoints within it, and where each notes its first change
//! (RULES.md R-TEST-1, R-TEST-2).

use std::sync::Arc;

use crate::log::Lsn;

use super::*;
use crate::catalog::{ColumnDesc, MemoryCatalog, TableDesc};
use crate::engine::rows::{RowStore, TableOptions};
use crate::storage::StorageEngine;
use crate::value::{DataType, Decimal, Spelled, Value};

/// A database's state and the storage of a table of one column, not in
/// its catalog, in an engine that undoes its own changes.
fn table() -> (State, Joined) {
    let st = State::with_builtins(Box::new(MemoryCatalog::default()));
    let name = QualifiedName { auth: "A".into(), ident: "T".into() };
    let column = ColumnDesc {
        name: "K".into(),
        data_type: DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric },
        not_null: false,
        default: None,
        domain: None,
        collation: None,
        generated: None,
    };
    let desc = TableDesc {
        periods: Vec::new(),
        typed: None,
        name: name.clone(),
        columns: vec![column],
        constraints: vec![],
        engine: "rowstore".into(),
        options: Vec::new(),
        generation: 0,
        temporary: None,
    };
    let engine = RowStore::in_memory(TableOptions::default());
    let storage: Arc<dyn TableStorage> = Arc::from(engine.create_table(&desc).unwrap());
    (st, Joined { table: Arc::new(name), storage })
}

/// `joined` made a participant of `tx`, as `State::join` makes one.
fn join(tx: &Txn, joined: &Joined) {
    joined.storage.begin(tx.id, tx.snapshot()).unwrap();
    lock_builtin(&tx.state).joined.push(joined.clone());
    tx.joined_after_savepoints(joined);
}

/// The values of the rows `tx` sees in `joined`.
fn seen(tx: &Txn, joined: &Joined) -> Vec<String> {
    Arc::clone(&joined.storage).scan(tx.id, &[0]).map(|r| r.unwrap().1[0].to_string()).collect()
}

/// A row of the value `k`.
fn row(k: i64) -> Vec<Value> {
    vec![Value::Exact(Decimal::new(i128::from(k), 0))]
}

/// Verifies: [ISO/IEC 9075-2:2023 §17.8 GR3d, GR3e] going back to a
/// savepoint taken in an earlier statement cancels, in an engine that
/// undoes its own changes, every change after it and none before; it
/// stays, and a savepoint taken after it is gone. Destroyed, a savepoint
/// is no longer one the transaction has.
#[test]
fn a_savepoint_is_gone_back_to_across_statements() {
    let (st, t) = table();
    let tx = st.begin();
    tx.begin_statement(|| false).unwrap();
    join(&tx, &t);
    t.storage.insert(tx.id, row(1)).unwrap();
    let first = tx.savepoint().unwrap();
    tx.begin_statement(|| false).unwrap();
    t.storage.insert(tx.id, row(2)).unwrap();
    let second = tx.savepoint().unwrap();
    t.storage.insert(tx.id, row(3)).unwrap();
    assert_eq!(seen(&tx, &t), ["1", "2", "3"]);
    st.undo_to_savepoint(&tx, first).unwrap();
    assert_eq!(seen(&tx, &t), ["1"]);
    assert!(tx.has_savepoint(first) && !tx.has_savepoint(second));
    t.storage.insert(tx.id, row(4)).unwrap();
    st.undo_to_savepoint(&tx, first).unwrap();
    assert_eq!(seen(&tx, &t), ["1"], "again, of what was changed since");
    tx.forget_savepoint(first);
    assert!(!tx.has_savepoint(first));
    assert_eq!(st.undo_to_savepoint(&tx, first).map_err(|e| e.code), Err(Code::Api));
    st.rollback(&tx).unwrap();
}

/// Verifies: [ISO/IEC 9075-2:2023 §17.8 GR3d] an engine that undoes its
/// own changes and joins after a savepoint has a savepoint taken as it
/// joins, which going back returns it to: none of its changes stays.
#[test]
fn an_engine_joined_after_a_savepoint_is_returned_to_its_beginning() {
    let (st, t) = table();
    let tx = st.begin();
    tx.begin_statement(|| false).unwrap();
    let id = tx.savepoint().unwrap();
    join(&tx, &t);
    t.storage.insert(tx.id, row(1)).unwrap();
    st.undo_to_savepoint(&tx, id).unwrap();
    assert_eq!(seen(&tx, &t), Vec::<String>::new());
    st.rollback(&tx).unwrap();
}

/// Verifies: [DESIGN] going back to a savepoint clamps the statement
/// running to it: its undo entries begin no later, and the savepoint it
/// took of an engine after the savepoint is taken again, where the
/// engine is now; one taken before it, in the same statement, stays. So
/// the statement failing after cancels its own changes since and no
/// more.
#[test]
fn the_statement_running_is_clamped_to_the_savepoint() {
    let (st, t) = table();
    let tx = st.begin();
    tx.begin_statement(|| false).unwrap();
    join(&tx, &t);
    t.storage.insert(tx.id, row(1)).unwrap();
    let before = t.storage.savepoint(tx.id);
    lock_builtin(&tx.state).statement_savepoints.push((t.clone(), before));
    let id = tx.savepoint().unwrap();
    st.undo_to_savepoint(&tx, id).unwrap();
    assert_eq!(lock_builtin(&tx.state).statement_savepoints.iter().map(|(_, sp)| *sp).collect::<Vec<_>>(), [before]);
    tx.begin_statement(|| false).unwrap();
    t.storage.insert(tx.id, row(2)).unwrap();
    let later = tx.savepoint().unwrap();
    lock_builtin(&tx.state).statement_savepoints.push((t.clone(), t.storage.savepoint(tx.id)));
    t.storage.insert(tx.id, row(3)).unwrap();
    lock_builtin(&tx.state).statement_start = usize::MAX;
    st.undo_to_savepoint(&tx, later).unwrap();
    let state = lock_builtin(&tx.state);
    assert!(state.statement_start < usize::MAX, "clamped");
    assert_eq!(state.statement_savepoints.iter().map(|(_, sp)| *sp).collect::<Vec<_>>(), [t.storage.savepoint(tx.id)]);
    drop(state);
    t.storage.insert(tx.id, row(4)).unwrap();
    st.fail_statement(&tx, &SqlError::new(Code::Data, "failed")).unwrap();
    assert_eq!(seen(&tx, &t), ["1", "2"], "the statement's own change since cancelled");
    st.rollback(&tx).unwrap();
}

/// Verifies: [ISO/IEC 9075-4:2006 draft §14.1 GR6] a mark undone takes
/// the savepoints taken within it, whose changes it cancels; one taken
/// before it stays.
#[test]
fn a_mark_undone_takes_its_savepoints() {
    let (st, t) = table();
    let tx = st.begin();
    tx.begin_statement(|| false).unwrap();
    join(&tx, &t);
    let outside = tx.savepoint().unwrap();
    let mark = tx.mark().unwrap();
    let within = tx.savepoint().unwrap();
    st.undo_to(&tx, mark, &SqlError::new(Code::Data, "failed")).unwrap();
    assert!(tx.has_savepoint(outside) && !tx.has_savepoint(within));
    st.rollback(&tx).unwrap();
}

/// Verifies: [DESIGN] the participants whose storage a cancelled change
/// of definitions made are rolled back in their engines and are no longer
/// participants, nor in any savepoint; others stay.
#[test]
fn participants_of_storage_discarded_are_forgotten() {
    let (st, t) = table();
    let (_, other) = table();
    let tx = st.begin();
    tx.begin_statement(|| false).unwrap();
    join(&tx, &t);
    let id = tx.savepoint().unwrap();
    join(&tx, &other);
    tx.forget_participants(&[Arc::clone(&other.storage)]);
    let state = lock_builtin(&tx.state);
    assert_eq!(state.joined.len(), 1);
    assert!(state.joined.iter().all(|j| j.is(&t.storage)));
    assert!(state.savepoints.open.iter().all(|s| s.engines.iter().all(|(j, _)| !j.is(&other.storage))));
    drop(state);
    st.undo_to_savepoint(&tx, id).unwrap();
    st.rollback(&tx).unwrap();
}

/// Verifies: [DESIGN] the first change logged after each savepoint that
/// has none is noted for it, the newest ones only. MC/DC of
/// `from.is_none()` taking the newest: an older savepoint's place kept as
/// newer ones' are set.
#[test]
fn each_savepoint_notes_its_first_change() {
    let mut savepoints = Savepoints::default();
    let taken = |s: &mut Savepoints, id: u64| {
        s.open.push(Savepoint { id, undo: 0, engines: Vec::new(), from: None, statement: 0, kept: 0 });
    };
    taken(&mut savepoints, 0);
    assert!(savepoints.logged_at(LogPos::At(Lsn(10))), "the first after it");
    taken(&mut savepoints, 1);
    taken(&mut savepoints, 2);
    assert!(savepoints.logged_at(LogPos::Held(20)));
    assert!(!savepoints.logged_at(LogPos::Held(30)), "each has its first");
    let held = |n| Some(LogPos::Held(n));
    assert_eq!(savepoints.open.iter().map(|s| s.from).collect::<Vec<_>>(), [Some(LogPos::At(Lsn(10))), held(20), held(20)]);
    assert_eq!(savepoints.places().count(), 3);
    savepoints.cancel_from(1);
    assert_eq!(savepoints.open.len(), 1);
    savepoints.statement_began();
    assert_eq!((savepoints.statement, savepoints.next()), (1, 0));
}

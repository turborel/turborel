//! Unit tests of what a statement's writes resolve once (RULES.md D872,
//! R-TEST-1): what a write finds done in its transaction, which no API
//! shows, and that one `Writing` held across statements writes as a table's
//! name does.

use std::mem;
use std::sync::Arc;

use crate::catalog::{Catalog, ColumnDesc, MemoryCatalog, QualifiedName, TableDesc};
use crate::db::State;
use crate::engine::rows::{RowStore, TableOptions};
use crate::error::{Code, SqlError};
use crate::storage::{Row, StorageEngine, TableStorage};
use crate::txn::{TableLock, Txn};
use crate::value::{DataType, Decimal, Spelled, Value};

/// A database with table A.T (K), in the row store, and a transaction open.
fn database() -> (State, Txn, QualifiedName) {
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
    let storage: Arc<dyn TableStorage> = Arc::from(RowStore::in_memory(TableOptions::default()).create_table(&desc).unwrap());
    let mut catalog = MemoryCatalog::default();
    catalog.create_schema("A", "A", vec![desc]);
    let mut st = State::with_builtins(Box::new(catalog));
    st.storage.insert(Arc::new(name.clone()), storage);
    st.locks.insert(name.clone(), Arc::new(TableLock::default()));
    let tx = st.begin();
    (st, tx, name)
}

fn row(k: i64) -> Row {
    vec![Value::Exact(Decimal::new(k.into(), 0))]
}

/// Verifies: [DESIGN] RULES.md D872: a write finds its first work done
/// only once the transaction wrote the table, joined its engine and took
/// its statement's savepoint there, and while no table it wrote has had
/// its definitions changed. MC/DC of `Txn::prepared`: all four hold
/// against each taken away alone, not written, not joined, no savepoint of
/// the statement, definitions changed.
#[test]
fn a_write_finds_its_first_work_done_only_while_it_holds() {
    let (st, tx, name) = database();
    let writing = st.writing(&name).unwrap();
    assert!(!tx.prepared(&writing).unwrap(), "nothing written yet");
    st.insert_row_in(&tx, &writing, &row(1)).unwrap();
    assert!(tx.prepared(&writing).unwrap(), "written, joined, its statement's savepoint taken");
    let written = mem::take(&mut tx.state().unwrap().written);
    assert!(!tx.prepared(&writing).unwrap(), "not written");
    tx.state().unwrap().written = written;
    let joined = mem::take(&mut tx.state().unwrap().joined);
    assert!(!tx.prepared(&writing).unwrap(), "not joined");
    tx.state().unwrap().joined = joined;
    let saved = mem::take(&mut tx.state().unwrap().statement_savepoints);
    assert!(!tx.prepared(&writing).unwrap(), "no savepoint of the statement");
    tx.state().unwrap().statement_savepoints = saved;
    assert!(tx.prepared(&writing).unwrap());
    st.lock_of(&name).unwrap().definitions_changed(st.generation.saturating_add(1));
    assert!(!tx.prepared(&writing).unwrap(), "its definitions changed");
    assert_eq!(st.insert_row_in(&tx, &writing, &row(2)).err().map(|e| e.code), Some(Code::Conflict), "refused as by name");
}

/// Verifies: [DESIGN] RULES.md D872, [X3.135-1986 §7.3 GR3b i]: one
/// `Writing` held across statements writes as the table's name does. A
/// statement that fails after writing through it is undone alone, its
/// savepoint taken anew though the transaction wrote the table before;
/// rows updated and deleted through it are undone with it.
#[test]
fn a_writing_held_across_statements_writes_as_the_name_does() {
    let (st, tx, name) = database();
    let writing = st.writing(&name).unwrap();
    let storage = Arc::clone(st.storage_of(&name).unwrap());
    let rows = |by: &Txn| -> Vec<String> {
        let mut rows: Vec<String> = Arc::clone(&storage).scan(by.id, &[0]).map(|r| r.unwrap().1[0].to_string()).collect();
        rows.sort();
        rows
    };
    tx.begin_statement(|| false).unwrap();
    let first = st.insert_row_in(&tx, &writing, &row(1)).unwrap();
    let second = st.insert_row_in(&tx, &writing, &row(2)).unwrap();
    tx.begin_statement(|| false).unwrap();
    st.insert_row_in(&tx, &writing, &row(3)).unwrap();
    st.update_row_in(&tx, &writing, first, &row(10)).unwrap();
    st.fail_statement(&tx, &SqlError::new(Code::Api, "[DESIGN] failed")).unwrap();
    assert_eq!(rows(&tx), ["1", "2"], "the failed statement undone alone");
    tx.begin_statement(|| false).unwrap();
    st.update_row_in(&tx, &writing, first, &row(10)).unwrap();
    st.delete_row_in(&tx, &writing, second).unwrap();
    assert_eq!(rows(&tx), ["10"]);
    st.insert_row_in(&tx, &writing, &row(11)).unwrap();
    st.fail_statement(&tx, &SqlError::new(Code::Api, "[DESIGN] failed")).unwrap();
    assert_eq!(rows(&tx), ["1", "2"]);
    st.commit(&tx).unwrap();
    assert_eq!(Arc::clone(&storage).scan(crate::txn::NO_TRANSACTION, &[0]).count(), 2);
}

/// Verifies: [DESIGN] RULES.md D875: of a statement that said which
/// columns it sets, the key columns it does not set are taken as kept with
/// no value compared, and those it sets are compared; of one that said
/// nothing, every key column is compared. MC/DC of a key column kept: set
/// and alike, set and changed, not set.
#[test]
fn key_columns_a_statement_does_not_set_are_kept_unread() {
    let (st, _tx, name) = database();
    let mut writing = st.writing(&name).unwrap();
    writing.key_columns = 0b011;
    let (old, new) =
        (vec![row(1)[0].clone(), row(2)[0].clone(), row(3)[0].clone()], vec![row(9)[0].clone(), row(2)[0].clone(), row(4)[0].clone()]);
    assert_eq!(writing.kept_columns(&old, &new), 0b010, "nothing said: both key columns compared");
    let third = writing.setting(0b100);
    assert_eq!(third.kept_columns(&old, &new), 0b011, "neither key column set: kept, the first not looked at");
    let first = third.setting(0b001);
    assert_eq!(first.kept_columns(&old, &new), 0b010, "the first set and changed, the second not set");
    let both = first.setting(0b011);
    assert_eq!(both.kept_columns(&old, &old), 0b011, "set and alike");
}

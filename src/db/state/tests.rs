//! Unit tests: the lookups report a broken invariant instead of panicking
//! (RULES.md R-ERR-1, R-TEST-2 robustness).

use super::*;
use crate::catalog::{ColumnDesc, IndexDesc, MemoryCatalog};
use crate::error::Code;
use crate::value::Spelled;
use crate::value::{DataType, Decimal, Value};

fn table(engine: &str) -> TableDesc {
    let column = ColumnDesc {
        name: "K".into(),
        data_type: DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Numeric },
        not_null: true,
        default: None,
        domain: None,
        collation: None,
        generated: None,
    };
    TableDesc {
        periods: Vec::new(),
        typed: None,
        name: QualifiedName { auth: "A".into(), ident: "T".into() },
        columns: vec![column],
        constraints: vec![],
        engine: engine.into(),
        options: Vec::new(),
        generation: 0,
        temporary: None,
    }
}

/// Verifies: [DESIGN] `storage_of` and `index_of` return SQLCODE -910 for a
/// table without storage and an index that does not exist.
#[test]
fn lookups_report_missing_objects() {
    let mut catalog = MemoryCatalog::default();
    catalog.create_schema("A", "A", vec![table(ROW_STORE)]);
    let st = State::with_builtins(Box::new(catalog)); // nothing attached: the invariant is broken
    let name = QualifiedName { auth: "A".into(), ident: "T".into() };
    assert_eq!(st.storage_of(&name).err().map(|e| e.code), Some(Code::Api));
    assert_eq!(st.index_of("NOPE").err().map(|e| e.code), Some(Code::Api));
}

/// A database with table A.T (K NOT NULL UNIQUE), one row inserted by an
/// open transaction, which is returned.
fn with_open_insert() -> (State, Txn, QualifiedName) {
    let mut st = State::with_builtins(Box::new(MemoryCatalog::default()));
    st.schema(
        "CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INT NOT NULL UNIQUE)",
        (None, crate::value::datetime::Now::default()),
        (None, &mut crate::db::sequences::Epochs::fresh()),
    )
    .unwrap();
    let name = QualifiedName { auth: "A".into(), ident: "T".into() };
    let tx = st.begin();
    st.insert_row(&tx, &name, &vec![Value::Exact(Decimal::new(1, 0))]).unwrap();
    (st, tx, name)
}

/// Verifies: [DESIGN] RULES.md D33: ROLLBACK undoes a change in the
/// storage and index it was made in, even when the definitions no
/// longer have them (as after a DROP TABLE): the row and its UNIQUE
/// entry are gone there, nothing is damaged, and the transaction ends.
#[test]
fn rollback_undoes_where_the_changes_were_made() {
    let (mut st, tx, name) = with_open_insert();
    let storage = Arc::clone(st.storage_of(&name).unwrap());
    let index = st.indexes.values().next().map(Arc::clone).unwrap();
    st.storage.remove(&name);
    st.indexes.clear();
    st.rollback(&tx).unwrap();
    drop(tx);
    assert_eq!((st.tx.active(), st.damaged_tables()), (0, vec![]));
    assert_eq!(Arc::clone(&storage).scan(crate::txn::NO_TRANSACTION, &[0]).count(), 0, "the row undone");
    assert_eq!(index.search(&crate::index::Probe::All, crate::index::KeyOrder::Ascending).count(), 0, "its entry undone");
}

/// Verifies: [DESIGN] attaching a catalog whose index names an index method
/// that is not built in fails with SQLCODE -910.
#[test]
fn attaching_an_unknown_index_method_fails() {
    let mut catalog = MemoryCatalog::default();
    catalog.create_schema("A", "A", vec![table(ROW_STORE)]);
    let name = QualifiedName { auth: "A".into(), ident: "T".into() };
    catalog.add_index(IndexDesc {
        name: "I".into(),
        table: name,
        columns: vec![0],
        method: "nope".into(),
        unique: false,
        options: Vec::new(),
        collations: Vec::new(),
        nulls: false,
        expressions: Vec::new(),
    });
    let mut st = State::with_builtins(Box::new(catalog));
    assert_eq!(st.attach_catalog_objects().err().map(|e| e.code), Some(Code::Api));
}

/// Verifies: [DESIGN] RULES.md D27: a statement whose snapshot predates a
/// change to its table's definitions (an index added) may not write the
/// table, which would skip the index: SQLCODE -911.
#[test]
fn a_writer_with_a_stale_snapshot_is_refused() {
    let (st, tx, name) = with_open_insert();
    st.lock_of(&name).unwrap().definitions_changed(st.generation.saturating_add(1));
    let stale = st.insert_row(&tx, &name, &vec![Value::Exact(Decimal::new(2, 0))]);
    assert_eq!(stale.err().map(|e| e.code), Some(Code::Conflict));
}

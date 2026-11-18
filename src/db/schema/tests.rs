//! Unit tests of `<schema>` definition (`db/schema.rs`).

use super::*;
use crate::catalog::Catalog;
use crate::catalog::{ColumnDesc, ConstraintDesc, ConstraintKind, MemoryCatalog};
use crate::engine::rows::ROW_STORE;
use crate::index::BTREE_METHOD;
use crate::value::DataType;
use crate::value::Spelled;

fn state() -> State {
    State::with_builtins(Box::new(MemoryCatalog::default()))
}

fn table(engine: &str, unique: bool) -> TableDesc {
    let column = ColumnDesc {
        name: "K".into(),
        data_type: DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Numeric },
        not_null: true,
        default: None,
        domain: None,
        collation: None,
        generated: None,
    };
    let name = QualifiedName { auth: "A".into(), ident: "T".into() };
    let key = ConstraintDesc {
        name: QualifiedName { auth: "A".into(), ident: "T_UNIQUE1".into() },
        kind: ConstraintKind::Unique { columns: vec![0], primary: false, nulls_distinct: true, period: None },
        attributes: crate::catalog::ConstraintAttributes::default(),
        domain: None,
    };
    TableDesc {
        periods: Vec::new(),
        typed: None,
        name,
        columns: vec![column],
        constraints: if unique { vec![key] } else { vec![] },
        engine: engine.into(),
        options: Vec::new(),
        generation: 0,
        temporary: None,
    }
}

/// Verifies: [DESIGN] a `<schema>` needing an engine or constraint index
/// method that is not registered is refused with SQLCODE -910 before
/// anything changes [X3.135-1986 §3.3].
#[test]
fn unknown_engines_and_methods_are_refused_before_any_change() {
    let mut st = state();
    assert_eq!(st.check_new_objects(&[table("nope", false)]).err().map(|e| e.code), Some(Code::Api));
    st.settings.set("TABLES.UNIQUE_INDEX_METHOD", Some(&Value::Char("nope".into()))).unwrap();
    assert_eq!(st.check_new_objects(&[table(ROW_STORE, true)]).err().map(|e| e.code), Some(Code::Api));
    assert_eq!(
        st.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INT NOT NULL UNIQUE)", (None, 0), (None, &mut Epochs::fresh()))
            .err()
            .map(|e| e.code),
        Some(Code::Api)
    );
    assert!(!st.catalog.schema_exists("A"), "the refused <schema> left no trace");
}

/// Verifies: [DESIGN] `apply_schema` reports an unknown engine or method
/// rather than panicking, if called without `check_new_objects`.
#[test]
fn apply_schema_reports_what_it_cannot_find() {
    let mut st = state();
    let catalog = Arc::clone(&st.catalog);
    assert_eq!(st.apply_schema(&*catalog, &[table("nope", false)], vec![], None).err().map(|e| e.code), Some(Code::Api));
    let mut no_method = state();
    no_method.settings.set("TABLES.UNIQUE_INDEX_METHOD", Some(&Value::Char("nope".into()))).unwrap();
    let other = Arc::clone(&no_method.catalog);
    assert_eq!(no_method.apply_schema(&*other, &[table(ROW_STORE, true)], vec![], None).err().map(|e| e.code), Some(Code::Api));
}

/// Verifies: [DESIGN] `create_index` on a catalog table without storage
/// reports SQLCODE -910 instead of panicking.
#[test]
fn create_index_without_storage_is_reported() {
    let mut catalog = MemoryCatalog::default();
    catalog.create_schema("A", "A", vec![table(ROW_STORE, false)]);
    let mut st = State::with_builtins(Box::new(catalog));
    let name = QualifiedName { auth: "A".into(), ident: "T".into() };
    let (columns, method) = (vec!["K".into()], BTREE_METHOD.into());
    let spec = IndexSpec { name: "I".into(), table: name, columns, method, options: Vec::new(), nulls: false, expressions: Vec::new() };
    assert_eq!(st.create_index(&spec).err().map(|e| e.code), Some(Code::Api));
}

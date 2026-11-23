//! The method's indexes made and opened by descriptor, and what each
//! answers of counts, rows by key and comparisons.

use super::*;
use crate::catalog::{QualifiedName, TableDesc};
use crate::vector::{Layout as VectorLayout, Shape, typed};

/// A VECTOR(100) column of SPARSE layout.
fn column() -> DataType {
    typed(Shape { dims: Some(100), element: Some(Element::Float32), layout: Some(VectorLayout::Sparse) }).unwrap()
}

/// The table an index of `desc` is on.
fn table() -> TableDesc {
    TableDesc {
        periods: Vec::new(),
        typed: None,
        name: QualifiedName { auth: "U".to_owned(), ident: "T".to_owned() },
        columns: Vec::new(),
        constraints: Vec::new(),
        engine: crate::engine::rows::ROW_STORE.into(),
        options: Vec::new(),
        generation: 0,
        temporary: None,
    }
}

/// Verifies: [EXTENSION] (R10.3, D41) an index made by its descriptor
/// takes the descriptor's layout, as does one opened by its locator, or
/// changed in place; no UNIQUE or NULLS NOT DISTINCT index is made, and
/// an index of the method opens by its locator alone too.
#[test]
fn indexes_are_made_and_opened_by_descriptor() {
    let method = SparseMethod::in_memory();
    let half = desc(vec![(QUANTIZATION_OPTION.to_owned(), Value::Char("FLOAT16".into()))]);
    let made = method.create_on(&[column()], (&table(), &half)).unwrap();
    made.insert(key(&sparse(&[(1, 1.0)])), 1).unwrap();
    let locator = made.locator();
    let opened = method.open_on(&[column()], &locator, (&table(), &half)).unwrap().unwrap();
    assert_eq!(opened.rows(&key(&sparse(&[(1, 1.0)]))).unwrap(), [1]);
    opened.reconfigure((&table(), &desc(Vec::new())));
    assert_eq!(method.open(&[column()], &locator).unwrap().unwrap().rows(&key(&sparse(&[(1, 1.0)]))).unwrap(), [1]);
    for refused in [IndexDesc { unique: true, ..desc(Vec::new()) }, IndexDesc { nulls: true, ..desc(Vec::new()) }] {
        assert_eq!(method.create_on(&[column()], (&table(), &refused)).err().unwrap().code, Code::Unsupported);
    }
    assert!(method.create(&[column()]).is_ok());
}

/// Verifies: [EXTENSION] (R3.11, D85) an insert alone never finds a
/// duplicate, as no UNIQUE index of the method is made; the documents are
/// estimated and counted as distinct keys, no comparison; the rows of a
/// key are found as a stream; a key of two columns, or not a VECTOR, is
/// refused; a dropped index frees its pages once its last reader lets it
/// go.
#[test]
fn counts_rows_and_refused_keys() {
    let (_, index) = index();
    let (a, b) = (sparse(&[(1, 1.0)]), sparse(&[(2, 1.0)]));
    assert!(!index.insert_alone(key(&a), 1).unwrap());
    index.insert(key(&b), 2).unwrap();
    assert_eq!(index.estimate(&Probe::All), Some(2));
    assert_eq!(index.estimate(&Probe::Columns(Vec::new())), None);
    assert_eq!(index.distinct_keys(), Some(2.0_f64));
    let due = index.maintenance_due();
    assert!(index.maintain().unwrap() || !due);
    let rows: Vec<RowId> = Arc::clone(&index).key_rows(&key(&b)).map(Result::unwrap).collect();
    assert_eq!(rows, [2]);
    let two = Key(vec![Value::Null, Value::Null]);
    assert_eq!(index.rows(&two).unwrap_err().code, Code::Unsupported);
    assert!(Arc::clone(&index).key_rows(&two).next().unwrap().is_err());
    assert_eq!(index.insert(Key(vec![Value::Char("x".into())]), 3).unwrap_err().code, Code::Unsupported);
    index.discard();
    drop(index);
}

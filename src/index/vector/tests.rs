//! The vector index methods through `Index`, apart from SQL: counted
//! entries, nulls, rows once each, filters, radii, a refused budget and
//! locators, each of the three methods.

use std::sync::Arc;

use super::Column;
use super::index::located;
use crate::error::{Code, Result};
use crate::index::{
    Approximation, DiskAnnMethod, FilterHint, HnswMethod, Index, IndexMethod, IvfMethod, Key, NearestQuery, Probe, RowFilter,
};
use crate::memory::MemoryBudget;
use crate::storage::RowId;
use crate::value::{DataType, Decimal, Value};
use crate::vector::{Element, Layout, Shape, functions, typed};

const DIMS: u32 = 8;
const ROWS: u64 = 400;
const K: usize = 10;

fn column() -> DataType {
    typed(Shape { dims: Some(DIMS), element: Some(Element::Float32), layout: Some(Layout::Dense) }).unwrap()
}

fn methods() -> [Box<dyn IndexMethod>; 3] {
    [Box::new(DiskAnnMethod::in_memory()), Box::new(HnswMethod::in_memory()), Box::new(IvfMethod::in_memory())]
}

fn call(name: &str, args: &[Value]) -> Value {
    functions().into_iter().find(|f| f.name() == name).unwrap().call(args).unwrap()
}

fn vector(v: &[f32]) -> Value {
    let text: Vec<String> = v.iter().map(f32::to_string).collect();
    call(
        "TO_VECTOR",
        &[Value::Char(format!("[{}]", text.join(","))), Value::Exact(Decimal::new(i128::from(DIMS), 0)), Value::Char("FLOAT32".into())],
    )
}

/// A number of `seed` in [-1, 1), the same each run.
fn unit(seed: u64) -> f32 {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0x2545_F491_4F6C_DD1D;
    x = (x ^ (x >> 31)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    let top = u16::try_from(x >> 48_u32).unwrap();
    f32::from(top) / 32_768.0 - 1.0
}

/// Row `n`'s vector, `shift` added to each element.
fn shifted(n: u64, shift: f32) -> Vec<f32> {
    (0..u64::from(DIMS)).map(|d| unit(n.wrapping_mul(64).wrapping_add(d)) + shift).collect()
}

fn point(n: u64) -> Vec<f32> {
    shifted(n, 0.0)
}

fn key(n: u64) -> Key {
    Key(vec![vector(&point(n))])
}

fn filled(method: &dyn IndexMethod) -> Arc<dyn Index> {
    let index: Arc<dyn Index> = Arc::from(method.create(&[column()]).unwrap());
    for n in 0..ROWS {
        index.insert(key(n), n).unwrap();
    }
    index
}

/// The query of a call of `metric` near `point(near)`.
fn query(metric: &str, near: u64, radius: Option<f64>) -> NearestQuery {
    let function = functions().into_iter().find(|f| f.name() == "L2_DISTANCE").unwrap();
    NearestQuery {
        function: crate::functions::ScalarRef(function),
        column: 0,
        argument: 0,
        value: vector(&point(near)),
        descending: false,
        statistics: None,
        metric: Some(metric.to_owned()),
        approximate: Some(Approximation {
            first: K,
            search_list: Some(64),
            beam_width: None,
            probes: Some(64),
            accuracy: None,
            widen: 1,
            radius,
            filter: FilterHint::None,
        }),
    }
}

fn found(index: &Arc<dyn Index>, q: &NearestQuery, filter: Option<&mut dyn RowFilter>) -> Result<Option<Vec<u64>>> {
    let budget = MemoryBudget::new(1 << 24);
    let Some(entries) = Arc::clone(index).approximate(q, &budget, filter)? else { return Ok(None) };
    Ok(Some(entries.map(|e| e.map(|(_, _, row)| row)).collect::<Result<Vec<_>>>()?))
}

/// Verifies: [DESIGN] RULES.md D29, the vector design document R10.6: an
/// entry is counted, a row with two entries answered once, at its nearer;
/// a null is not indexed, and every entry is listed by `Probe::All`.
#[test]
fn entries_are_counted_and_rows_answered_once() {
    for method in methods() {
        let index = filled(&*method);
        index.insert(key(5), 5).unwrap();
        index.insert(Key(vec![vector(&shifted(7, 0.001))]), 6).unwrap();
        index.insert(Key(vec![vector(&shifted(7, 0.002))]), 6).unwrap();
        index.insert(Key(vec![Value::Null]), 999).unwrap();
        assert_eq!(index.rows(&key(5)).unwrap(), [5], "{}", method.name());
        let rows = found(&index, &query("COSINE", 7, None), None).unwrap().unwrap();
        assert!(rows.contains(&6) && rows.contains(&7), "{}: {rows:?}", method.name());
        let unique: std::collections::HashSet<_> = rows.iter().collect();
        assert_eq!(unique.len(), rows.len(), "{}: {rows:?}", method.name());
        index.remove(&key(5), 5).unwrap();
        assert_eq!(index.rows(&key(5)).unwrap(), [5], "{}: one entry left", method.name());
        index.remove(&key(5), 5).unwrap();
        assert!(index.rows(&key(5)).unwrap().is_empty(), "{}", method.name());
        let mut all: Vec<u64> = Arc::clone(&index).search(&Probe::All, crate::index::KeyOrder::Ascending).map(|e| e.unwrap().1).collect();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), usize::try_from(ROWS).unwrap() - 1, "{}", method.name());
        assert!(!all.contains(&999), "{}", method.name());
    }
}

/// One row in `SPARSE` a filter admits.
const SPARSE: u64 = 20;

/// Admits one row in `SPARSE`, counting the rows it is asked of.
struct Sparse(usize);

impl RowFilter for Sparse {
    fn admits(&mut self, _key: &Key, row: RowId) -> Result<bool> {
        self.0 = self.0.saturating_add(1);
        Ok(row.is_multiple_of(SPARSE))
    }
}

/// A filter that fails.
struct Failing;

impl RowFilter for Failing {
    fn admits(&mut self, _key: &Key, _row: RowId) -> Result<bool> {
        Err(crate::error::SqlError::new(Code::Api, "[DESIGN] a failing filter"))
    }
}

/// Verifies: [EXTENSION] RULES.md D-APPROX: an in-filter is asked of the
/// candidates and only rows it admits are answered, as many as asked, of
/// one in twenty, past the list the search was asked for; its
/// first error is the search's.
#[test]
fn an_in_filter_admits_rows_and_its_error_ends_the_search() {
    for method in methods() {
        let index = filled(&*method);
        let mut sparse = Sparse(0);
        let rows = found(&index, &query("COSINE", 100, None), Some(&mut sparse)).unwrap().unwrap();
        assert!(rows.iter().all(|r| r.is_multiple_of(SPARSE)), "{}: {rows:?}", method.name());
        assert!(rows.len() >= K, "{}: {rows:?}", method.name());
        assert!(sparse.0 > 0, "{}", method.name());
        let failed = found(&index, &query("COSINE", 100, None), Some(&mut Failing));
        assert_eq!(failed.err().map(|e| e.code), Some(Code::Api), "{}", method.name());
    }
}

/// Verifies: [EXTENSION] the vector design document R13.1: an index
/// declines a call of another metric, and a search whose candidates the
/// budget refuses declines too (RULES.md D38), never failing.
#[test]
fn another_metric_or_a_refused_budget_declines() {
    for method in methods() {
        let index = filled(&*method);
        assert!(found(&index, &query("EUCLIDEAN", 3, None), None).unwrap().is_none(), "{}", method.name());
        let starved = MemoryBudget::new(0);
        let declined = Arc::clone(&index).approximate(&query("COSINE", 3, None), &starved, None).unwrap();
        assert!(declined.is_none(), "{}", method.name());
    }
}

/// Verifies: [DESIGN] RULES.md D-DISKANN, D-HNSW, D-IVF: a locator names
/// its version, its column's format and dimensions; another of each is
/// SQLCODE -916.
#[test]
fn a_locator_names_its_version_and_column() {
    let column = Column { dims: DIMS, element: Element::Float32 };
    for method in methods() {
        let index = method.create(&[self::column()]).unwrap();
        let locator = index.locator();
        assert!(located(&locator, column).is_ok(), "{}", method.name());
        let other = Column { dims: DIMS + 1, element: Element::Float32 };
        assert_eq!(located(&locator, other).err().map(|e| e.code), Some(Code::Corrupt), "{}", method.name());
        let mut later = locator.clone();
        if let Some(v) = later.first_mut() {
            *v = 2;
        }
        assert_eq!(located(&later, column).err().map(|e| e.code), Some(Code::Corrupt), "{}", method.name());
        let reopened = method.open(&[self::column()], &locator).unwrap();
        assert!(reopened.is_some(), "{}", method.name());
    }
}

/// Verifies: [EXTENSION] the vector design document R10.3, R3.11: a
/// method refuses a column that is no fixed dense VECTOR, two columns, and
/// FLOAT64 (SQLCODE -900); it answers no comparison.
#[test]
fn a_method_refuses_columns_it_cannot_index() {
    let open = crate::vector::data_type();
    let wide = typed(Shape { dims: Some(DIMS), element: Some(Element::Float64), layout: Some(Layout::Dense) }).unwrap();
    let int = DataType::Exact { precision: crate::value::INTEGER_PRECISION, scale: 0, spelled: crate::value::Spelled::Integer };
    for method in methods() {
        for columns in [vec![open.clone()], vec![wide.clone()], vec![int.clone()], vec![column(), column()]] {
            assert_eq!(method.create(&columns).err().map(|e| e.code), Some(Code::Unsupported), "{}", method.name());
        }
        assert!(!method.supports(crate::value::CompOp::Eq), "{}", method.name());
    }
}

/// Verifies: [EXTENSION] vector_standard R7.1, §10.2 RERANK: DiskANN's
/// RERANK ON gives the whole search list to be ordered again by the exact
/// distance, and OFF the rows asked alone, as ALTER INDEX sets it.
#[test]
fn diskann_rerank_gives_the_whole_list_or_the_rows_asked() {
    let index = filled(&DiskAnnMethod::in_memory());
    let q = query("COSINE", 3, None);
    let on = found(&index, &q, None).unwrap().unwrap();
    assert!(on.len() > K, "the list of 64: {}", on.len());
    let name = crate::catalog::QualifiedName { auth: "A".into(), ident: "T".into() };
    let table = crate::catalog::TableDesc {
        periods: Vec::new(),
        typed: None,
        name: name.clone(),
        columns: vec![],
        constraints: vec![],
        engine: crate::engine::rows::ROW_STORE.into(),
        options: Vec::new(),
        generation: 0,
        temporary: None,
    };
    let off = crate::catalog::IndexDesc {
        name: "T_V".into(),
        table: name,
        columns: vec![0],
        method: "diskann".into(),
        unique: false,
        options: vec![("DISKANN.RERANK".into(), Value::Char("OFF".into()))],
        collations: Vec::new(),
        nulls: false,
        expressions: Vec::new(),
    };
    index.reconfigure((&table, &off));
    assert_eq!(found(&index, &q, None).unwrap().unwrap().len(), K);
}

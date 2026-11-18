//! Hybrid index method requirements: a change reaches both parts or
//! neither, nulls held by the part of the other column, every row once,
//! the options and locator of each part, and nearest searches routed by
//! the column's position (RULES.md R-TEST-1, R-TEST-2).

use super::options::{located, locator, parts};
use super::*;
use crate::catalog::QualifiedName;
use crate::error::Code;
use crate::functions::ScalarRef;
use crate::index::{Approximation, FilterHint, Key, KeyOrder, Probe, RowFilter};
use crate::memory::MemoryBudget;
use crate::storage::RowId;
use crate::value::Value;
use crate::vector::{Element, Layout, Shape, Vector, typed};

/// The method, keeping its indexes in a pool of memory of its own.
fn in_memory() -> HybridMethod {
    HybridMethod::sharing(&MemoryPages::new(crate::engine::MEMORY_POOL_BYTES))
}

/// The two columns: a text and a VECTOR(4).
fn columns() -> [DataType; 2] {
    let vector = typed(Shape { dims: Some(4), element: Some(Element::Float32), layout: Some(Layout::Dense) }).unwrap();
    [DataType::varying(100), vector]
}

/// `v` as a VECTOR value.
fn vector(v: &[f64]) -> Value {
    Vector::dense(Element::Float32, v.to_vec()).unwrap().value().unwrap()
}

/// The key of `text`, or null, and the vector `v`, or null.
fn key(text: Option<&str>, v: Option<&[f64]>) -> Key {
    Key(vec![text.map_or(Value::Null, |t| Value::Char(t.to_owned())), v.map_or(Value::Null, vector)])
}

/// Every row the index holds, in order.
fn every_row(index: &Arc<dyn Index>) -> Vec<RowId> {
    let mut rows: Vec<RowId> = Arc::clone(index).search(&Probe::All, KeyOrder::Ascending).map(|e| e.unwrap().1).collect();
    rows.sort_unstable();
    rows
}

/// A descriptor of an index of the hybrid method of `options`.
fn desc(options: Vec<(String, Value)>) -> IndexDesc {
    IndexDesc {
        name: "H".to_owned(),
        table: QualifiedName { auth: "U".to_owned(), ident: "T".to_owned() },
        columns: vec![3, 5],
        method: HYBRID_METHOD.to_owned(),
        unique: false,
        options,
        collations: vec![None, None],
        nulls: true,
        expressions: Vec::new(),
    }
}

/// VECTOR_DISTANCE by COSINE, approximately for `first` rows, of `v`, of
/// the key's column `column`.
fn query(column: usize, v: &[f64], first: usize) -> NearestQuery {
    let function = crate::vector::functions().into_iter().find(|f| f.name() == "VECTOR_DISTANCE").unwrap();
    let approximate = Approximation {
        first,
        search_list: None,
        beam_width: None,
        probes: None,
        accuracy: None,
        widen: 1,
        radius: None,
        filter: FilterHint::InFilter,
    };
    NearestQuery {
        function: ScalarRef(function),
        column,
        argument: 0,
        value: vector(v),
        descending: false,
        statistics: None,
        metric: Some("COSINE".to_owned()),
        approximate: Some(approximate),
    }
}

/// Admits every row but `0`, of the projected key alone.
struct AllBut(RowId);

impl RowFilter for AllBut {
    fn admits(&mut self, key: &Key, row: RowId) -> Result<bool> {
        Ok(row != self.0 && key.0.len() == 1)
    }
}

/// Vectors of the tests.
const EAST: [f64; 4] = [1.0, 0.0, 0.0, 0.0];
const NEAR_EAST: [f64; 4] = [0.9, 0.1, 0.0, 0.0];
const NORTH: [f64; 4] = [0.0, 1.0, 0.0, 0.0];
const ONES: [f64; 4] = [1.0; 4];
const WIDE: [f64; 5] = [1.0, 0.0, 0.0, 0.0, 0.0];

/// Verifies: [EXTENSION] vector_standard §11.1, RULES.md D19, D85: an
/// entry is in both parts, each of its own column, a null in neither,
/// every row found once; a change the vector part refuses leaves the text
/// part as it was; a removal takes it from both; each key projects to one.
#[test]
fn a_change_reaches_both_parts_or_neither() {
    let method = in_memory();
    let index: Arc<dyn Index> = Arc::from(method.create(&columns()).unwrap());
    index.insert(key(Some("quick brown fox"), Some(&EAST)), 1).unwrap();
    index.insert(key(None, Some(&NORTH)), 2).unwrap();
    index.insert(key(Some("lazy dog"), None), 3).unwrap();
    index.insert(key(None, None), 4).unwrap();
    assert_eq!(every_row(&index), [1, 2, 3]);
    let wide = Key(vec![Value::Char("ghost words".to_owned()), vector(&WIDE)]);
    assert!(index.insert(wide.clone(), 5).is_err());
    assert_eq!(every_row(&index), [1, 2, 3], "the text part's entry taken back");
    assert!(index.insert(Key(vec![Value::Null]), 6).is_err());
    index.remove(&key(Some("quick brown fox"), Some(&EAST)), 1).unwrap();
    assert_eq!(every_row(&index), [2, 3]);
    let projection = index.key_projection().unwrap();
    assert_eq!(projection.project(&key(Some("a"), None)).unwrap(), projection.project(&key(None, Some(&[0.0_f64; 4]))).unwrap());
    assert!(projection.project(&Key(vec![Value::Null])).is_err());
    let projected = projection.project(&key(None, None)).unwrap();
    assert_eq!(index.rows(&projected).unwrap().len(), 2);
    // Of every entry, the greater of the parts' counts.
    assert_eq!(index.estimate(&Probe::All), Some(1));
    assert_eq!(index.estimate(&Probe::Columns(Vec::new())), None);
}

/// Verifies: [EXTENSION] RULES.md D41, D25: each part is described by the
/// options of its method's group, of its own column; HYBRID.VECTOR_METHOD
/// names the vector part's method, HNSW if none; a locator holds both
/// parts' and is refused as corrupt if of another version or cut short;
/// an index reopened by it holds its rows.
#[test]
fn each_part_has_its_options_and_locator() {
    let options = vec![
        (VECTOR_METHOD_OPTION.to_owned(), Value::Char("IVF".to_owned())),
        ("IVF.METRIC".to_owned(), Value::Char("EUCLIDEAN".to_owned())),
        ("HNSW.M".to_owned(), Value::Char("8".to_owned())),
        ("FULLTEXT.K1".to_owned(), Value::Approx(1.5_f64)),
    ];
    let (text, vector) = parts(&desc(options));
    assert_eq!((text.method.as_str(), text.columns.as_slice()), (crate::index::FULLTEXT_METHOD, [3].as_slice()));
    assert_eq!(text.options.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["FULLTEXT.K1"]);
    assert_eq!((vector.method.as_str(), vector.columns.as_slice()), (crate::index::IVF_METHOD, [5].as_slice()));
    assert_eq!(vector.options.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["IVF.METRIC"]);
    assert!(!text.nulls && !vector.nulls);
    assert_eq!(parts(&desc(Vec::new())).1.method, crate::index::HNSW_METHOD);
    let made = locator(b"text", b"vec");
    assert_eq!(located(&made).unwrap(), (b"text".as_slice(), b"vec".as_slice()));
    let mut later = made.clone();
    later[0] = 2;
    assert_eq!(located(&later).err().map(|e| e.code), Some(Code::Corrupt));
    assert_eq!(located(made.get(..7).unwrap_or_default()).err().map(|e| e.code), Some(Code::Corrupt));
    let method = in_memory();
    let index = method.create(&columns()).unwrap();
    index.insert(key(Some("quick brown fox"), Some(&EAST)), 7).unwrap();
    let reopened: Arc<dyn Index> = Arc::from(method.open(&columns(), &index.locator()).unwrap().unwrap());
    assert_eq!(every_row(&reopened), [7]);
}

/// Verifies: [EXTENSION] vector_standard §11.1, R13.1: the method serves a
/// distance of its vector column approximately, as its vector method
/// does, of that method's metric; of its text column, a score only; of
/// no other column; it indexes a text and a vector, never UNIQUE.
#[test]
fn the_method_serves_each_column_as_its_part_does() {
    let method = in_memory();
    let both = columns();
    assert_eq!(method.supports_nearest(&query(VECTOR, &ONES, 2), &both), NearestSupport::Approximate);
    assert_eq!(method.supports_nearest(&query(TEXT, &ONES, 2), &both), NearestSupport::No);
    assert_eq!(method.supports_nearest(&query(2, &ONES, 2), &both), NearestSupport::No);
    assert_eq!(method.supports_nearest(&query(VECTOR, &ONES, 2), &both[..1]), NearestSupport::No);
    let ivf = desc(vec![(VECTOR_METHOD_OPTION.to_owned(), Value::Char("IVF".to_owned()))]);
    assert_eq!(method.metric(&ivf), IvfMethod::in_memory().metric(&parts(&ivf).1));
    assert!(method.metric(&desc(Vec::new())).is_some());
    assert_eq!(method.create(&both[..1]).err().map(|e| e.code), Some(Code::Unsupported));
    let unique = IndexDesc { unique: true, ..desc(Vec::new()) };
    let table = TableDesc {
        periods: Vec::new(),
        typed: None,
        name: unique.table.clone(),
        columns: vec![],
        constraints: vec![],
        engine: String::new(),
        options: Vec::new(),
        generation: 0,
        temporary: None,
    };
    assert_eq!(method.create_on(&both, (&table, &unique)).err().map(|e| e.code), Some(Code::Unsupported));
    assert!(method.create_on(&both, (&table, &desc(Vec::new()))).is_ok());
    assert!(method.index_options().iter().any(|o| o.name == VECTOR_METHOD_OPTION));
}

/// Verifies: [EXTENSION] RULES.md D94, D100, D85: an approximate search of
/// the vector column is the vector part's, its entries of the projected
/// key, the query's filter asked of each as of it; of the text column
/// none is approximate.
#[test]
fn an_approximate_search_reads_the_vector_part() {
    let method = in_memory();
    let index: Arc<dyn Index> = Arc::from(method.create(&columns()).unwrap());
    for (row, v) in [(1, EAST), (2, NEAR_EAST), (3, NORTH)] {
        index.insert(key(Some("text"), Some(&v)), row).unwrap();
    }
    let budget = MemoryBudget::new(usize::MAX);
    let found = |column: usize, filter: Option<&mut dyn RowFilter>| {
        let entries = Arc::clone(&index).approximate(&query(column, &EAST, 2), &budget, filter).unwrap();
        entries.map(|e| e.map(|r| r.unwrap()).collect::<Vec<_>>())
    };
    let nearest = found(VECTOR, None).unwrap();
    let projected = index.key_projection().unwrap().project(&key(None, None)).unwrap();
    assert!(nearest.iter().all(|(_, k, _)| *k == projected));
    assert_eq!(nearest.iter().take(2).map(|(_, _, r)| *r).collect::<Vec<_>>(), [1, 2]);
    let filtered = found(VECTOR, Some(&mut AllBut(1))).unwrap();
    assert_eq!(filtered.first().map(|(_, _, r)| *r), Some(2));
    assert!(found(TEXT, None).is_none());
    assert!(Arc::clone(&index).nearest(&query(VECTOR, &ONES, 1), &budget).unwrap().is_none());
}

//! Inverted index method requirements: counted entries under projected
//! keys, every entry once, bulk loads, options read and changed in place,
//! locators, refusals, top-k through the method's index, and approximate
//! searches (`tests/nearest.rs`).

mod method;
mod nearest;

use std::sync::Arc;

use super::*;
use crate::catalog::QualifiedName;
use crate::engine::sparse::{Ask, Quantization};
use crate::memory::MemoryBudget;
use crate::value::types::ExtensionValue;

/// A VECTOR key of `vector`'s image. The method reads only the image, so
/// any kind carries it here.
fn key(vector: &SparseVector) -> Key {
    Key(vec![Value::Extension(ExtensionValue { kind: crate::geometry::GEOMETRY, bytes: vector.image().into() })])
}

fn sparse(entries: &[(u32, f32)]) -> SparseVector {
    SparseVector::new(100, entries.iter().copied()).unwrap()
}

fn desc(options: Vec<(String, Value)>) -> IndexDesc {
    IndexDesc {
        name: "I".to_owned(),
        table: QualifiedName { auth: "U".to_owned(), ident: "T".to_owned() },
        columns: vec![0],
        method: SPARSE_METHOD.to_owned(),
        unique: false,
        options,
        collations: Vec::new(),
        nulls: false,
        expressions: Vec::new(),
    }
}

fn index() -> (SparseMethod, Arc<SparseIndex>) {
    let method = SparseMethod::in_memory();
    let made = Arc::new(method.make(Layout::default()).unwrap());
    (method, made)
}

/// Verifies: [EXTENSION] (D85) Entries are counted under the hash of
/// their vector: `rows` finds them by the vector, and by the projected key
/// every vector shares finds every row; `Probe::All` gives each (projected
/// key, row) once, and each insert is taken back by one remove.
#[test]
fn entries_are_counted_under_projected_keys() {
    let (_, index) = index();
    let (a, b) = (sparse(&[(1, 1.0), (2, 2.0)]), sparse(&[(3, 0.5)]));
    index.insert(key(&a), 1).unwrap();
    index.insert(key(&a), 1).unwrap();
    index.insert(key(&a), 2).unwrap();
    index.insert(key(&b), 3).unwrap();
    let projected = index.key_projection().unwrap().project(&key(&a)).unwrap();
    assert_eq!(index.rows(&key(&a)).unwrap(), [1, 2]);
    assert_eq!(index.rows(&projected).unwrap(), [1, 2, 3]);
    assert_eq!(index.key_projection().unwrap().project(&key(&b)).unwrap(), projected);
    let mut all: Vec<(Vec<Value>, RowId)> =
        Arc::clone(&index).search(&Probe::All, KeyOrder::Ascending).map(|e| e.map(|(k, r)| (k.0, r)).unwrap()).collect();
    all.sort_by_key(|(_, r)| *r);
    assert_eq!(all.iter().map(|(_, r)| *r).collect::<Vec<_>>(), [1, 2, 3]);
    assert_eq!(Key(all[0].0.clone()), projected);
    index.remove(&key(&a), 1).unwrap();
    assert_eq!(index.rows(&key(&a)).unwrap(), [1, 2]);
    index.remove(&key(&a), 1).unwrap();
    assert_eq!(index.rows(&key(&a)).unwrap(), [2]);
    assert_eq!(index.estimate(&Probe::All), Some(2));
    let found = index.tree().top_k(&sparse(&[(2, 1.0), (3, 1.0)]), 5, Ask::default(), &MemoryBudget::new(usize::MAX)).unwrap();
    assert_eq!(found.hits.iter().map(|h| h.row).collect::<Vec<_>>(), [2, 3]);
}

/// Verifies: [EXTENSION] (R10.3, R10.6, R3.11) The method makes no index
/// of a column not VECTOR, nor a UNIQUE one; refuses a NULL key and a
/// dense image; and answers no comparison.
#[test]
fn refusals() {
    let (method, index) = index();
    let integer = DataType::Exact { precision: 10, scale: 0, spelled: crate::value::Spelled::Integer };
    assert_eq!(method.create(&[integer]).err().unwrap().code, Code::Unsupported);
    assert!(!method.supports(CompOp::Eq));
    assert_eq!(index.insert(Key(vec![Value::Null]), 1).unwrap_err().code, Code::Unsupported);
    let mut dense = sparse(&[(1, 1.0)]).image();
    dense[2] = 0;
    let dense = Key(vec![Value::Extension(ExtensionValue { kind: crate::geometry::GEOMETRY, bytes: dense.into() })]);
    assert_eq!(index.insert(dense, 1).unwrap_err().code, Code::Data);
    let probe = Probe::Compare { op: CompOp::Eq, value: Value::Null };
    assert!(Arc::clone(&index).search(&probe, KeyOrder::Ascending).next().unwrap().is_err());
}

/// Verifies: [EXTENSION] (D41) The options read from a descriptor give
/// the layout, the defaults UINT8 and 128 (§10.4); a block size out of
/// range or an unknown quantization is refused with SQLCODE -103; a
/// layout changes in place.
#[test]
fn options_give_the_layout_and_change_in_place() {
    let named = |n: &str, v: Value| vec![(n.to_owned(), v)];
    assert_eq!(options::layout_of(&desc(Vec::new())).unwrap(), Layout { quantization: Quantization::Uint8, block: 128 });
    let half = named(QUANTIZATION_OPTION, Value::Char("FLOAT16".into()));
    assert_eq!(options::layout_of(&desc(half.clone())).unwrap().quantization, Quantization::Float16);
    let small = named(BLOCK_SIZE_OPTION, Value::Exact(crate::value::Decimal::new(8, 0)));
    assert_eq!(options::layout_of(&desc(small)).unwrap_err().code, Code::SyntaxRule);
    let unknown = named(QUANTIZATION_OPTION, Value::Char("PQ".into()));
    assert_eq!(options::layout_of(&desc(unknown)).unwrap_err().code, Code::SyntaxRule);
    let (method, index) = index();
    assert_eq!(method.index_options().len(), 2);
    index.tree().set_layout(options::layout_of(&desc(half)).unwrap());
    assert_eq!(index.tree().layout().quantization, Quantization::Float16);
}

/// Verifies: [DESIGN] A locator of format 1 finds the index again with
/// its entries; another format is refused with SQLCODE -916; a bulk load
/// through the method leaves the entries inserts would.
#[test]
fn locators_reopen_and_loads_fill() {
    let (method, index) = index();
    let entries: Vec<_> = (0..200_u32).map(|n| (sparse(&[(n % 7, 1.0), (n % 11 + 20, 2.0)]), u64::from(n))).collect();
    let areas = WorkAreas::new(MemoryBudget::new(usize::MAX));
    let mut keyed = entries.iter().map(|(v, r)| Ok((key(v), *r)));
    index.load(&mut keyed, (&areas, Area { bytes: 4 << 10_u32, batch: 1 << 10_u32 })).unwrap();
    let locator = index.locator();
    assert_eq!(locator[0], 1);
    method.checkpoint().unwrap();
    let again = method.reopen(&locator, Layout::default()).unwrap();
    for (v, r) in &entries {
        assert!(again.rows(&key(v)).unwrap().contains(r));
    }
    let mut wrong = locator;
    wrong[0] = 2;
    assert_eq!(method.reopen(&wrong, Layout::default()).err().unwrap().code, Code::Corrupt);
}

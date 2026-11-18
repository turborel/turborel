//! Unit tests of the json index: versions counted, postings written once
//! and taken back at the last count, counts kept, searches against the
//! documents themselves, bulk loads equal to inserts, a projection's rows
//! (RULES.md R-TEST-1, R-TEST-2).

use std::collections::BTreeSet;
use std::ops::Bound;

use super::keys::count_term;
use super::*;
use crate::engine::memory_file;
use crate::engine::tree::Direction;
use crate::functions::{JsonContains, ScalarRef};
use crate::memory::MemoryBudget;
use crate::value::Value;

/// An index of every path and value, in memory, of `shape`.
fn index_of(shape: Shape) -> Arc<JsonIndex> {
    let (pool, file) = memory_file(&MemoryBudget::new(MEMORY_POOL_BYTES << 4_i32), "a test").unwrap();
    Arc::new(JsonIndex::with(BLinkTree::create(pool, file).unwrap(), shape))
}

/// An index of every path and value, in memory.
fn index() -> Arc<JsonIndex> {
    index_of(whole())
}

/// The key of the document `text`.
fn doc(text: &str) -> Key {
    Key(vec![Value::Char(text.into())])
}

/// Every entry of `index`'s tree, in key order.
fn dump(index: &JsonIndex) -> Vec<(Vec<u8>, Vec<u8>)> {
    index.tree.scan(Bound::Unbounded, Bound::Unbounded, Direction::Forward).collect::<Result<_>>().unwrap()
}

/// The probe of JSON_CONTAINS of the column and `candidate`.
fn contains(candidate: &str) -> Probe {
    let predicate = Box::new(Predicate { function: ScalarRef(Arc::new(JsonContains)), column: 0, argument: None });
    Probe::Predicate { predicate, value: Value::Row(vec![Value::Char(candidate.into())].into()) }
}

/// The rows `probe` finds of `index`, each once.
fn found(index: &Arc<JsonIndex>, probe: &Probe) -> BTreeSet<RowId> {
    Arc::clone(index).search(probe, KeyOrder::Ascending).map(|e| e.map(|(_, row)| row)).collect::<Result<_>>().unwrap()
}

/// Documents of rows 0 to 199, varied by their row.
fn documents() -> Vec<(Key, RowId)> {
    (0..200_u64)
        .map(|n| {
            let text = format!(r#"{{"id":{n},"kind":"k{}","tags":["t{}","t{}"],"deep":{{"x":{}}}}}"#, n % 7, n % 3, n % 5, n % 2 == 0);
            (doc(&text), n)
        })
        .collect()
}

/// Verifies: [EXTENSION] JSON_CONTAINS finds, of the documents, at least
/// every one that contains the candidate, and by its terms far fewer than
/// all; a document no JSON text is found by every probe.
#[test]
fn contains_finds_every_document_containing() {
    let index = index();
    for (key, row) in documents() {
        index.insert(key, row).unwrap();
    }
    index.insert(doc("{ not json"), 500).unwrap();
    let want: BTreeSet<RowId> = (0..200).filter(|n| n % 7 == 3 && (n % 3 == 1 || n % 5 == 1)).collect();
    let rows = found(&index, &contains(r#"{"kind":"k3","tags":["t1"]}"#));
    assert!(rows.is_superset(&want), "every containing document");
    assert!(rows.contains(&500), "the unread document");
    assert!(rows.len() < 60, "a few beyond, found by their terms: {}", rows.len());
    let deep = found(&index, &contains(r#"{"deep":{"x":true}}"#));
    assert!(deep.is_superset(&(0..200).filter(|n| n % 2 == 0).collect()));
    assert!(!deep.contains(&1));
}

/// Verifies: [DESIGN] D34: versions of a row with one document post once,
/// counted; the last taken back takes its postings and counts, leaving
/// the tree empty; a null document posts nothing.
#[test]
fn versions_post_once_and_the_last_takes_back() {
    let index = index();
    let d = doc(r#"{"a":[1,{"b":"x"}]}"#);
    index.insert(d.clone(), 7).unwrap();
    let once = dump(&index);
    index.insert(d.clone(), 7).unwrap();
    assert_eq!(dump(&index).len(), once.len(), "a second version posts nothing");
    index.insert(Key(vec![Value::Null]), 8).unwrap();
    index.remove(&d, 7).unwrap();
    assert_eq!(dump(&index).len(), once.len(), "one version left");
    assert_eq!(index.rows(&d).unwrap(), vec![7]);
    index.remove(&d, 7).unwrap();
    assert!(dump(&index).is_empty(), "nothing left");
    index.remove(&d, 7).unwrap();
    assert!(dump(&index).is_empty(), "a remove of none changes nothing");
}

/// Verifies: [DESIGN] D31: a bulk load, of entries in any order, makes
/// exactly the tree inserting them one by one does, counts included.
#[test]
fn a_load_equals_inserting_one_by_one() {
    let (loaded, inserted) = (index(), index());
    let mut entries = documents();
    entries.push((doc("[1,2"), 300));
    entries.push(entries.first().cloned().unwrap());
    for (key, row) in entries.clone() {
        inserted.insert(key, row).unwrap();
    }
    let work = WorkAreas::new(MemoryBudget::new(usize::MAX));
    loaded.load(&mut entries.into_iter().rev().map(Ok), (&work, Area::new(4_096, 64))).unwrap();
    assert_eq!(dump(&loaded), dump(&inserted));
    assert!(dump(&loaded).iter().any(|(key, _)| count_term(key).is_some()), "counts written");
}

/// Verifies: [DESIGN] D85: the rows of a document are found by its
/// projected key, and by its text; documents apart by trailing spaces
/// project alike.
#[test]
fn rows_of_a_projected_document() {
    let index = index();
    index.insert(doc(r#"{"a":1}"#), 3).unwrap();
    index.insert(doc(r#"{"a":1}  "#), 4).unwrap();
    let projected = JsonMethod::projection().project(&doc(r#"{"a":1}"#)).unwrap();
    let rows: Vec<RowId> = Arc::clone(&index).key_rows(&projected).collect::<Result<_>>().unwrap();
    assert_eq!(rows, vec![3, 4]);
    assert_eq!(index.rows(&doc(r#"{"a":1}"#)).unwrap(), vec![3, 4]);
}

/// Verifies: [EXTENSION] D41: an index posting paths alone finds a
/// document by its paths, a value asked of it by its path too.
#[test]
fn paths_only_finds_by_paths() {
    let index = index_of(Shape { paths_only: true, max_depth: 128 });
    index.insert(doc(r#"{"a":1}"#), 1).unwrap();
    index.insert(doc(r#"{"a":2}"#), 2).unwrap();
    index.insert(doc(r#"{"b":1}"#), 3).unwrap();
    assert_eq!(found(&index, &contains(r#"{"a":1}"#)), BTreeSet::from([1, 2]));
}

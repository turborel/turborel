//! Contract tests for `IndexMethod` and `Index` (RULES.md R-SAFE-3), for
//! every built-in index method; a plug-in's can be checked the same way.

use std::sync::Arc;

use turborel::disk::buffer::{BufferPool, Clock};
use turborel::disk::encoding::Pipeline;
use turborel::disk::io::OsFileSystem;
use turborel::disk::pager::Pager;
use turborel::error::Result;
use turborel::fulltext::matches::TextMatches;
use turborel::functions::ScalarRef;
use turborel::index::{
    AdjacencyMethod, BTreeMethod, FullTextMethod, HashMethod, Index, IndexMethod, Key, KeyOrder, Predicate, Probe, RTreeMethod,
};
use turborel::storage::RowId;
use turborel::value::{CompOp, DataType, Spelled, Value, compare};

use crate::common::{Scratch, crc32c, number};

/// The keys and ids the index contract uses: a duplicate key, and gaps.
const ENTRIES: [(i128, RowId); 5] = [(1, 10), (2, 11), (2, 12), (3, 13), (5, 14)];
/// Probe values below, on, between and above the keys.
const PROBES: [i128; 5] = [0, 2, 4, 5, 6];
const OPERATORS: [CompOp; 6] = [CompOp::Eq, CompOp::Ne, CompOp::Lt, CompOp::Gt, CompOp::Le, CompOp::Ge];

fn key(n: i128) -> Key {
    Key(vec![number(n)])
}

/// Every entry `probe` selects in `index`, in ascending key order if its
/// method is ordered: its search read whole.
fn searched(index: &Arc<dyn Index>, probe: &Probe) -> Result<Vec<(Key, RowId)>> {
    Arc::clone(index).search(probe, KeyOrder::Ascending).collect()
}

/// The ids `probe` selects in `index`, its search asked for descending
/// keys.
fn descending(index: &Arc<dyn Index>, probe: &Probe) -> Vec<RowId> {
    Arc::clone(index).search(probe, KeyOrder::Descending).map(|entry| entry.unwrap().1).collect()
}

/// The ids whose key satisfies `key op value`, by the comparison rules.
fn expected(op: CompOp, value: i128) -> Vec<RowId> {
    ENTRIES.iter().filter(|(k, _)| compare(&number(*k), &number(value)).is_some_and(|o| op.holds(o))).map(|(_, id)| *id).collect()
}

/// Checks the `IndexMethod` and `Index` contracts on `method`.
fn index_contract(method: &dyn IndexMethod, types: &[DataType], key: fn(i128) -> Key) {
    let index: Arc<dyn Index> = Arc::from(method.create(types).unwrap());
    assert!(searched(&index, &Probe::All).unwrap().is_empty(), "a new index is empty");
    for (k, id) in ENTRIES {
        index.insert(key(k), id).unwrap();
    }
    let ids = |probe: &Probe| searched(&index, probe).unwrap().into_iter().map(|(_, id)| id).collect::<Vec<_>>();
    let key_of = |id: RowId| ENTRIES.iter().find(|(_, i)| *i == id).map(|(k, _)| *k).unwrap();
    for op in OPERATORS.into_iter().filter(|op| method.supports(*op)) {
        for value in PROBES {
            let probe = Probe::Compare { op, value: number(value) };
            let found = ids(&probe);
            let mut sorted = found.clone();
            sorted.sort_unstable();
            assert_eq!(sorted, expected(op, value), "{} {op:?} {value}", method.name());
            assert!(
                searched(&index, &probe).unwrap().iter().all(|(k, id)| *k == key(key_of(*id))),
                "{} returns each entry's key",
                method.name()
            );
            let estimate = index.estimate(&probe);
            assert!(estimate.is_none_or(|n| n == found.len()), "{} {op:?} {value}: an estimate known cheaply is exact here", method.name());
            if method.ordered() {
                assert!(found.windows(2).all(|w| key_of(w[0]) <= key_of(w[1])), "{} {op:?} {value} in key order", method.name());
                let reversed: Vec<RowId> = found.iter().rev().copied().collect();
                assert_eq!(descending(&index, &probe), reversed, "{} {op:?} {value} descending", method.name());
            }
        }
    }
    let mut all = ids(&Probe::All);
    all.sort_unstable();
    assert_eq!(all, ENTRIES.map(|(_, id)| id).to_vec(), "{}", method.name());
    let rows = |k: i128| {
        let mut r = index.rows(&key(k)).unwrap();
        r.sort_unstable();
        let mut taken: Vec<RowId> = Arc::clone(&index).key_rows(&key(k)).collect::<Result<_>>().unwrap();
        taken.sort_unstable();
        assert_eq!(taken, r, "{}: a key's rows as read are its rows", method.name());
        r
    };
    assert_eq!(rows(2), [11, 12], "{}", method.name());
    // Entries are counted: a pair inserted twice stays until removed twice,
    // and is found once meanwhile.
    index.insert(key(2), 11).unwrap();
    assert_eq!((rows(2), ids(&Probe::Compare { op: CompOp::Eq, value: number(2) }).len()), (vec![11, 12], 2), "{}", method.name());
    index.remove(&key(2), 11).unwrap();
    assert_eq!(rows(2), [11, 12], "{}: one insert left", method.name());
    index.remove(&key(2), 11).unwrap();
    index.remove(&key(2), 99).unwrap(); // absent: no change
    assert_eq!((rows(2), rows(5)), (vec![12], vec![14]), "{}", method.name());
    index.remove(&key(5), 14).unwrap();
    assert_eq!(rows(5), Vec::<RowId>::new(), "{}", method.name());
    // A key it cannot encode: an error given as a row is, and no more.
    let wrong: Vec<Result<RowId>> = Arc::clone(&index).key_rows(&Key(vec![Value::Char("x".into())])).collect();
    assert!(matches!(wrong.as_slice(), [Err(_)]), "{}: {wrong:?}", method.name());
}

/// Checks the `Probe::Prefix` contract on `method`, with an index on two
/// columns: every prefix it supports returns exactly the entries whose key
/// answers it, in key order if the method is ordered.
fn prefix_contract(method: &dyn IndexMethod, data_type: &DataType, value: fn(i128) -> Value) {
    let index: Arc<dyn Index> = Arc::from(method.create(&[data_type.clone(), data_type.clone()]).unwrap());
    let keys: Vec<Key> = (0..4).flat_map(|a| (0..4).map(move |b| Key(vec![value(a), value(b)]))).collect();
    for (id, k) in (1..).zip(&keys) {
        index.insert(k.clone(), id).unwrap();
    }
    let thens = OPERATORS.into_iter().map(Some).chain([None]);
    for (equal, then) in [1, 2].into_iter().flat_map(|e| thens.clone().map(move |t| (e, t))) {
        if !method.supports_prefix(2, equal, then) {
            continue;
        }
        for (a, b) in (0..5).flat_map(|a| (0..5).map(move |b| (a, b))) {
            let values = [number(a), number(b)];
            let probe = Probe::Prefix { equal: values.get(..equal).unwrap().to_vec(), then: then.map(|op| (op, number(b))) };
            let found: Vec<RowId> = searched(&index, &probe).unwrap().into_iter().map(|(_, id)| id).collect();
            let mut sorted = found.clone();
            sorted.sort_unstable();
            let expected: Vec<RowId> = (1..).zip(&keys).filter(|(_, k)| probe.answers(&k.0)).map(|(id, _)| id).collect();
            assert_eq!(sorted, expected, "{} {probe:?}", method.name());
            if method.ordered() {
                assert_eq!(found, expected, "{} {probe:?} in key order", method.name());
            }
        }
    }
}

/// Checks the `Probe::Range` contract on `method`, with an index on two
/// columns: every range it supports, of each pair of bounds, inclusive or
/// not, empty ones among them, after no equal column or one, returns
/// exactly the entries whose key answers it, in key order.
fn range_contract(method: &dyn IndexMethod, data_type: &DataType, value: fn(i128) -> Value) {
    let index: Arc<dyn Index> = Arc::from(method.create(&[data_type.clone(), data_type.clone()]).unwrap());
    let keys: Vec<Key> = (0..4).flat_map(|a| (0..6).map(move |b| Key(vec![value(a), value(b)]))).collect();
    for (id, k) in (1..).zip(&keys) {
        index.insert(k.clone(), id).unwrap();
    }
    for equal in [0, 1] {
        if !method.supports_range(2, equal) {
            continue;
        }
        let bounds = (-1..7).flat_map(|lo| (-1..7).map(move |hi| (lo, hi)));
        for ((lo, hi), (low, high)) in
            bounds.flat_map(|b| [CompOp::Gt, CompOp::Ge].into_iter().flat_map(move |l| [CompOp::Lt, CompOp::Le].map(move |h| (b, (l, h)))))
        {
            let probe =
                Probe::Range { equal: vec![number(2); equal], low: Box::new((low, number(lo))), high: Box::new((high, number(hi))) };
            let found: Vec<RowId> = searched(&index, &probe).unwrap().into_iter().map(|(_, id)| id).collect();
            let expected: Vec<RowId> = (1..).zip(&keys).filter(|(_, k)| probe.answers(&k.0)).map(|(id, _)| id).collect();
            let mut sorted = found.clone();
            sorted.sort_unstable();
            assert_eq!(sorted, expected, "{} {probe:?}", method.name());
            if method.ordered() {
                assert_eq!(found, expected, "{} {probe:?} in key order", method.name());
                let reversed: Vec<RowId> = expected.iter().rev().copied().collect();
                assert_eq!(descending(&index, &probe), reversed, "{} {probe:?} descending", method.name());
            }
        }
    }
}

/// The entries of `keys`, ids from 1, in an index of `method` on columns
/// of `types`: those each of `probes` selects checked against
/// `Probe::answers`, and those of each key against `rows`.
fn answered(method: &dyn IndexMethod, (types, keys): (&[DataType], &[Key]), probes: &[Probe]) {
    let index: Arc<dyn Index> = Arc::from(method.create(types).unwrap());
    for (id, k) in (1..).zip(keys) {
        index.insert(k.clone(), id).unwrap();
    }
    for (id, k) in (1..).zip(keys) {
        assert_eq!(index.rows(k).unwrap(), [id], "{}: the entry of {k:?}", method.name());
    }
    for probe in probes {
        let mut found: Vec<RowId> = searched(&index, probe).unwrap().into_iter().map(|(_, id)| id).collect();
        found.sort_unstable();
        let expected: Vec<RowId> = (1..).zip(keys).filter(|(_, k)| probe.answers(&k.0)).map(|(id, _)| id).collect();
        assert_eq!(found, expected, "{} {probe:?}", method.name());
    }
}

/// Checks the contract of keys with a null on `method`, as an index of a
/// UNIQUE NULLS NOT DISTINCT constraint holds them, on one column and on
/// two: no probe but `Probe::All` selects one, as no comparison with a
/// null is true; `rows` finds a key's entries, a null equal to a null.
fn nulls_contract(method: &dyn IndexMethod) {
    let exact = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric };
    let some = |n: Option<i128>| n.map_or(Value::Null, number);
    let ones: Vec<Key> = [None, Some(1), Some(2)].into_iter().map(|a| Key(vec![some(a)])).collect();
    let compared: Vec<Probe> = OPERATORS
        .into_iter()
        .filter(|op| method.supports(*op))
        .flat_map(|op| (0..4).map(move |v| Probe::Compare { op, value: number(v) }))
        .chain([Probe::All])
        .collect();
    answered(method, (std::slice::from_ref(&exact), &ones), &compared);
    let pairs = [(None, None), (None, Some(1)), (Some(1), None), (Some(1), Some(2)), (Some(2), Some(1)), (Some(2), None)];
    let twos: Vec<Key> = pairs.iter().map(|(a, b)| Key(vec![some(*a), some(*b)])).collect();
    let mut probes = vec![Probe::All];
    for (op, v) in OPERATORS.into_iter().flat_map(|op| (0..4).map(move |v| (op, v))) {
        if method.supports_prefix(2, 1, Some(op)) {
            probes.push(Probe::Prefix { equal: vec![number(1)], then: Some((op, number(v))) });
        }
    }
    if method.supports_prefix(2, 1, None) {
        probes.push(Probe::Prefix { equal: vec![number(1)], then: None });
    }
    if method.supports_range(2, 0) {
        probes.push(Probe::Range { equal: Vec::new(), low: Box::new((CompOp::Ge, number(0))), high: Box::new((CompOp::Le, number(1))) });
    }
    answered(method, (&[exact.clone(), exact], &twos), &probes);
}

/// Verifies: [EXTENSION] `IndexMethod` and `Index` contracts, for the built-in
/// btree (in memory and in a pager file) and hash methods: every supported
/// probe, of one column, a prefix of two, or a range (RULES.md D34), returns
/// exactly the matching entries, with their keys, in the key order asked,
/// ascending or descending, if the method is ordered; an estimate known
/// cheaply is exact; entries are counted, a pair inserted twice staying
/// until removed twice; a key's rows read as taken (`Index::key_rows`)
/// are those `Index::rows` gives, each once. [ISO/IEC 9075-2:2023 §11.7]
/// Of keys with a null, held for UNIQUE NULLS NOT DISTINCT, a comparison
/// selects none, a `<` with no lower bound among them.
#[test]
fn built_in_index_methods_keep_the_index_contract() {
    let scratch = Scratch::new("contract-btree");
    let pager = Pager::create(&OsFileSystem, &scratch.path("index"), crc32c(), Pipeline::none(), 4096).unwrap();
    let pool = Arc::new(BufferPool::new(16, Box::new(Clock::new(16))).unwrap());
    let file = pool.add_pager(Arc::new(pager));
    for method in [&BTreeMethod::in_memory() as &dyn IndexMethod, &BTreeMethod::new(pool, file), &HashMethod::in_memory()] {
        assert!(method.supports(CompOp::Eq), "{} answers equality", method.name());
        let exact = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric };
        index_contract(method, std::slice::from_ref(&exact), key);
        assert!(method.supports_prefix(2, 2, None), "{} answers a whole key's equality", method.name());
        prefix_contract(method, &exact, number);
        assert_eq!(method.supports_range(2, 0), method.ordered(), "{}: an ordered method answers a range", method.name());
        range_contract(method, &exact, number);
        nulls_contract(method);
    }
}

/// Verifies: [EXTENSION] The rtree method passes the same comparisons,
/// streaming, key reconstruction, counted-reference, prefix and range
/// contracts as the other built-ins, with its two-coordinate key. Range
/// support does not imply SQL key order. Both memory and file pagers run.
#[test]
fn spatial_method_keeps_the_index_contract() {
    let scratch = Scratch::new("contract-rtree");
    let pager = Pager::create(&OsFileSystem, &scratch.path("index"), crc32c(), Pipeline::none(), 4096).unwrap();
    let pool = Arc::new(BufferPool::new(16, Box::new(Clock::new(16))).unwrap());
    let file = pool.add_pager(Arc::new(pager));
    let approximate = DataType::Approximate { precision: 53, spelled: Spelled::DoublePrecision };
    let value = |n| Value::Approx(f64::from(i32::try_from(n).unwrap()));
    let point = |n| Key(vec![Value::Approx(f64::from(i32::try_from(n).unwrap())), Value::Approx(0.0_f64)]);
    for method in [RTreeMethod::in_memory(), RTreeMethod::new(pool, file)] {
        assert!(!method.ordered());
        index_contract(&method, &[approximate.clone(), approximate.clone()], point);
        prefix_contract(&method, &approximate, value);
        range_contract(&method, &approximate, value);
    }
}

/// Verifies: [EXTENSION] RULES.md D141: the adjacency
/// method passes the comparison, streaming, key, counted-entry and prefix
/// contracts of the other built-ins, of a key of a source column and a
/// destination column, in memory and in a pager file; it answers
/// equality alone, and is not ordered.
#[test]
fn adjacency_method_keeps_the_index_contract() {
    let scratch = Scratch::new("contract-adjacency");
    let pager = Pager::create(&OsFileSystem, &scratch.path("index"), crc32c(), Pipeline::none(), 4096).unwrap();
    let pool = Arc::new(BufferPool::new(16, Box::new(Clock::new(16))).unwrap());
    let file = pool.add_pager(Arc::new(pager));
    let exact = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric };
    let edge = |n| Key(vec![number(n), number(n)]);
    for method in [AdjacencyMethod::in_memory(), AdjacencyMethod::new(pool, file)] {
        assert!(!method.ordered() && method.supports(CompOp::Eq) && !method.supports(CompOp::Lt));
        index_contract(&method, &[exact.clone(), exact.clone()], edge);
        assert!(method.supports_prefix(2, 2, None) && !method.supports_range(2, 0));
        prefix_contract(&method, &exact, number);
    }
}

/// The documents of the full-text contract, ids from 1: a duplicate, and
/// one of two words a search of both finds.
const DOCUMENTS: [&str; 4] = ["the quick fox", "a lazy dog", "the quick fox", "quick brown dogs"];

/// A document as a key of one column.
fn document(text: &str) -> Key {
    Key(vec![Value::Char(text.into())])
}

/// The ids a TEXT_MATCHES lookup of `words` selects in `index`, sorted.
fn matched(index: &Arc<dyn Index>, words: &str) -> Vec<RowId> {
    let predicate = Box::new(Predicate { function: ScalarRef(Arc::new(TextMatches)), column: 0, argument: Some(0) });
    let probe = Probe::Predicate { predicate, value: Value::Char(words.into()) };
    let mut ids: Vec<RowId> = searched(index, &probe).unwrap().into_iter().map(|(_, id)| id).collect();
    ids.sort_unstable();
    ids
}

/// Verifies: [EXTENSION] RULES.md D34, D86: the fulltext method keeps the
/// index contract in memory and in a pager file. It answers no comparison
/// and only a TEXT_MATCHES lookup of its one character string column. A
/// new index is empty; `Probe::All` gives every entry, each with the key
/// its projection makes of its document, one for documents alike; a
/// lookup gives the rows holding every word; entries are counted, a pair
/// inserted twice staying until removed twice; a key's rows read as taken
/// are those `Index::rows` gives; a key it cannot read is an error.
#[test]
fn full_text_method_keeps_the_index_contract() {
    let scratch = Scratch::new("contract-fulltext");
    let pager = Pager::create(&OsFileSystem, &scratch.path("index"), crc32c(), Pipeline::none(), 4096).unwrap();
    let pool = Arc::new(BufferPool::new(64, Box::new(Clock::new(64))).unwrap());
    let file = pool.add_pager(Arc::new(pager));
    let text = DataType::varying(100);
    for method in [FullTextMethod::in_memory(), FullTextMethod::new(pool, file)] {
        assert!(OPERATORS.iter().all(|op| !method.supports(*op)) && !method.ordered(), "it compares nothing");
        assert!(
            method.create(&[DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric }]).is_err()
                && method.create(&[text.clone(), text.clone()]).is_err()
        );
        let index: Arc<dyn Index> = Arc::from(method.create(std::slice::from_ref(&text)).unwrap());
        assert!(searched(&index, &Probe::All).unwrap().is_empty(), "a new index is empty");
        for (id, d) in (1..).zip(DOCUMENTS) {
            index.insert(document(d), id).unwrap();
        }
        let all = searched(&index, &Probe::All).unwrap();
        let key_of = |id: RowId| all.iter().find(|(_, i)| *i == id).map(|(k, _)| k.clone()).unwrap();
        assert_eq!(all.len(), 4);
        assert_eq!(key_of(1), key_of(3), "documents alike, keys alike");
        assert_ne!(key_of(1), key_of(2));
        assert_eq!(matched(&index, "quick"), [1, 3, 4]);
        assert_eq!(matched(&index, "quick dogs"), [4], "every word");
        assert_eq!(matched(&index, "Quick DOG"), Vec::<RowId>::new(), "words normalized, not stemmed");
        assert_eq!(matched(&index, "cat"), Vec::<RowId>::new());
        let rows = |d: &str| {
            let mut r = index.rows(&document(d)).unwrap();
            r.sort_unstable();
            let mut taken: Vec<RowId> = Arc::clone(&index).key_rows(&document(d)).collect::<Result<_>>().unwrap();
            taken.sort_unstable();
            assert_eq!(taken, r, "a key's rows as read are its rows");
            r
        };
        assert_eq!(rows(DOCUMENTS[0]), [1, 3]);
        index.insert(document(DOCUMENTS[1]), 2).unwrap();
        index.remove(&document(DOCUMENTS[1]), 2).unwrap();
        assert_eq!((rows(DOCUMENTS[1]), matched(&index, "lazy")), (vec![2], vec![2]), "one insert left");
        index.remove(&document(DOCUMENTS[1]), 2).unwrap();
        index.remove(&document(DOCUMENTS[1]), 99).unwrap(); // absent: no change
        assert_eq!((rows(DOCUMENTS[1]), matched(&index, "lazy")), (Vec::new(), Vec::new()));
        let wrong: Vec<Result<RowId>> = Arc::clone(&index).key_rows(&key(1)).collect();
        assert!(matches!(wrong.as_slice(), [Err(_)]), "{wrong:?}");
    }
}

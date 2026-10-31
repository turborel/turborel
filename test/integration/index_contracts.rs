//! Contract tests for `IndexMethod` and `Index` (RULES.md R-SAFE-3), for
//! every built-in index method; a plug-in's can be checked the same way.
//! Split from contracts.rs (RULES.md R-SIZE-2).

use std::sync::Arc;

use turborel::disk::buffer::{BufferPool, Clock};
use turborel::disk::encoding::Pipeline;
use turborel::disk::io::OsFileSystem;
use turborel::disk::pager::Pager;
use turborel::error::Result;
use turborel::index::{BTreeMethod, HashMethod, Index, IndexMethod, Key, KeyOrder, Probe};
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
fn index_contract(method: &dyn IndexMethod) {
    let index: Arc<dyn Index> = Arc::from(method.create(&[DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric }]).unwrap());
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
    let wrong: Vec<Result<RowId>> = Arc::clone(&index).key_rows(&Key(vec![Value::Char("x".to_owned())])).collect();
    assert!(matches!(wrong.as_slice(), [Err(_)]), "{}: {wrong:?}", method.name());
}

/// Checks the `Probe::Prefix` contract on `method`, with an index on two
/// columns: every prefix it supports returns exactly the entries whose key
/// answers it, in key order if the method is ordered.
fn prefix_contract(method: &dyn IndexMethod) {
    let exact = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric };
    let index: Arc<dyn Index> = Arc::from(method.create(&[exact, exact]).unwrap());
    let keys: Vec<Key> = (0..4).flat_map(|a| (0..4).map(move |b| Key(vec![number(a), number(b)]))).collect();
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
fn range_contract(method: &dyn IndexMethod) {
    let exact = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric };
    let index: Arc<dyn Index> = Arc::from(method.create(&[exact, exact]).unwrap());
    let keys: Vec<Key> = (0..4).flat_map(|a| (0..6).map(move |b| Key(vec![number(a), number(b)]))).collect();
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
            assert_eq!(found, expected, "{} {probe:?}", method.name());
            let reversed: Vec<RowId> = expected.iter().rev().copied().collect();
            assert_eq!(descending(&index, &probe), reversed, "{} {probe:?} descending", method.name());
        }
    }
}

/// Verifies: [EXTENSION] `IndexMethod` and `Index` contracts, for the built-in
/// btree (in memory and in a pager file) and hash methods: every supported
/// probe, of one column, a prefix of two, or a range (RULES.md D34), returns
/// exactly the matching entries, with their keys, in the key order asked,
/// ascending or descending, if the method is ordered; an estimate known
/// cheaply is exact; entries are counted, a pair inserted twice staying
/// until removed twice; a key's rows read as taken (`Index::key_rows`)
/// are those `Index::rows` gives, each once.
#[test]
fn built_in_index_methods_keep_the_index_contract() {
    let scratch = Scratch::new("contract-btree");
    let pager = Pager::create(&OsFileSystem, &scratch.path("index"), crc32c(), Pipeline::none(), 4096).unwrap();
    let pool = Arc::new(BufferPool::new(16, Box::new(Clock::new(16))).unwrap());
    let file = pool.add_pager(Arc::new(pager));
    for method in [&BTreeMethod::in_memory() as &dyn IndexMethod, &BTreeMethod::new(pool, file), &HashMethod::in_memory()] {
        assert!(method.supports(CompOp::Eq), "{} answers equality", method.name());
        index_contract(method);
        assert!(method.supports_prefix(2, 2, None), "{} answers a whole key's equality", method.name());
        prefix_contract(method);
        assert_eq!(method.supports_range(2, 0), method.ordered(), "{}: an ordered method answers a range", method.name());
        range_contract(method);
    }
}

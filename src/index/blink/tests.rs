//! Unit tests of the B-link index: probes of every operator with values of
//! the column's type and not, many entries in key order, estimates, counts
//! changed by threads at once, entries too long, and malformed entries
//! (RULES.md R-TEST-1, R-TEST-2). Split from blink.rs (RULES.md R-SIZE-4).

use std::thread;

use super::*;
use crate::engine::memory_file;
use crate::index::searched;
use crate::value::Spelled;
use crate::value::{Decimal, Value, compare};

const MONEY: DataType = DataType::Exact { precision: 6, scale: 2, spelled: Spelled::Numeric };

fn exact(units: i128, scale: u32) -> Value {
    Value::Exact(Decimal::new(units, scale))
}

/// An index on columns of `types`, in memory.
fn index(types: &[DataType]) -> Arc<BLinkIndex> {
    let (pool, file) = memory_file(&MemoryBudget::new(MEMORY_POOL_BYTES << 4_i32), "a test").unwrap();
    Arc::new(BLinkIndex::with(BLinkTree::create(pool, file).unwrap(), types, KeyCounts::new()))
}

/// The rows `probe` finds in `index`, in the order found.
fn found(index: &Arc<BLinkIndex>, op: CompOp, value: Value) -> Vec<RowId> {
    searched(index, &Probe::Compare { op, value }).unwrap().into_iter().map(|(_, row)| row).collect()
}

/// Verifies: [X3.135-1986 §5.11 GR2, GR3]; [DESIGN] RULES.md D31, probes:
/// on a DECIMAL(6,2) column, a value of a finer scale (10.555), one beyond
/// the column's range, and an approximate one find exactly the keys SQL
/// compares as asked, in key order; `<>` finds every key on either side;
/// a comparison with NULL finds none, whatever the operator.
#[test]
fn probes_find_what_sql_compares() {
    let index = index(&[MONEY]);
    let keys = [-1_000, 1_055, 1_056, 1_060, 9_999];
    for (row, units) in (1..).zip(keys) {
        index.insert(Key(vec![exact(units, 2)]), row).unwrap();
    }
    let probes =
        [exact(10_555, 3), exact(1_056, 2), exact(-99_999_999, 0), exact(99_999_999, 0), Value::Approx(10.56), Value::Approx(-20.0)];
    for value in probes {
        for op in [CompOp::Eq, CompOp::Ne, CompOp::Lt, CompOp::Le, CompOp::Gt, CompOp::Ge] {
            let want: Vec<RowId> =
                (1..).zip(keys).filter(|(_, u)| compare(&exact(*u, 2), &value).is_some_and(|o| op.holds(o))).map(|(r, _)| r).collect();
            assert_eq!(found(&index, op, value.clone()), want, "{op:?} {value:?}");
            assert_eq!(index.estimate(&Probe::Compare { op, value: value.clone() }), Some(want.len()));
        }
    }
    for op in [CompOp::Eq, CompOp::Ne, CompOp::Lt, CompOp::Ge] {
        assert_eq!(found(&index, op, Value::Null), Vec::<RowId>::new(), "{op:?} NULL");
    }
}

/// Verifies: [X3.135-1986 §5.11 GR5]; [DESIGN] RULES.md D31, probes: on a
/// CHAR(3) column, a string longer than the column is compared padded, so
/// 'ab ' equals 'ab' and 'abcd' is above 'abc'; keys come back padded.
#[test]
fn strings_longer_than_the_column_are_compared_padded() {
    let index = index(&[DataType::Character { length: 3 }]);
    for (row, s) in (1..).zip(["ab", "abc", "abd"]) {
        index.insert(Key(vec![Value::Char(s.into())]), row).unwrap();
    }
    let chars = |s: &str| Value::Char(s.into());
    assert_eq!(found(&index, CompOp::Eq, chars("ab    ")), [1]);
    assert_eq!(found(&index, CompOp::Gt, chars("abcd")), [3]);
    assert_eq!(found(&index, CompOp::Le, chars("abcd")), [1, 2]);
    assert_eq!(found(&index, CompOp::Eq, chars("abcd")), Vec::<RowId>::new());
    let (key, _) = searched(&index, &Probe::All).unwrap().remove(0);
    assert_eq!(format!("{:?}", key.0), "[Char(\"ab \")]");
}

/// Verifies: [DESIGN] RULES.md D31, the index as a tree: 20,000 entries,
/// many under one key, split it into many nodes and come back in key order,
/// a key's rows in row order; an estimate is exact up to 1,000 entries and
/// not known past them; entries removed are gone.
#[test]
fn many_entries_stay_in_key_order() {
    let index = index(&[MONEY]);
    for row in 0..20_000_u64 {
        let units = i128::from(row % 500);
        index.insert(Key(vec![exact(units, 0)]), row).unwrap();
    }
    assert!(index.tree.height().unwrap() > 1);
    let all = searched(&index, &Probe::All).unwrap();
    assert_eq!(all.len(), 20_000);
    assert!(all.windows(2).all(|w| w[0].0 < w[1].0 || (w[0].0 == w[1].0 && w[0].1 < w[1].1)));
    assert_eq!(index.rows(&Key(vec![exact(7, 0)])).unwrap(), (0..40).map(|i| 7 + 500 * i).collect::<Vec<_>>());
    assert_eq!(index.estimate(&Probe::Compare { op: CompOp::Lt, value: exact(10, 0) }), Some(400));
    assert_eq!(index.estimate(&Probe::Compare { op: CompOp::Gt, value: exact(10, 0) }), None);
    assert_eq!(index.estimate(&Probe::All), None);
    for row in (0..20_000_u64).filter(|r| r % 500 == 7) {
        index.remove(&Key(vec![exact(7, 0)]), row).unwrap();
    }
    assert_eq!(index.rows(&Key(vec![exact(7, 0)])).unwrap(), Vec::<RowId>::new());
}

/// Verifies: [DESIGN] RULES.md D34, `Index::search`: a search reads a leaf
/// at a time as its entries are taken, forward or backward, so a key
/// inserted at the far end of a tree of many leaves after its first entry
/// is taken is found as it reaches that leaf; the search holds its index,
/// outliving the reference it was asked through. Regression test
/// (R-TEST-7): a search read every entry before giving the first.
#[test]
fn a_search_reads_as_its_entries_are_taken() {
    let index = index(&[MONEY]);
    for row in 1..3_000_u64 {
        index.insert(Key(vec![exact(i128::from(row), 0)]), row).unwrap();
    }
    assert!(index.tree.height().unwrap() > 1, "many leaves");
    let mut up = Arc::clone(&index).search(&Probe::All, KeyOrder::Ascending);
    let mut down = Arc::clone(&index).search(&Probe::All, KeyOrder::Descending);
    assert_eq!((up.next().unwrap().unwrap().1, down.next().unwrap().unwrap().1), (1, 2_999));
    index.insert(Key(vec![exact(5_000, 0)]), 5_000).unwrap();
    index.insert(Key(vec![exact(0, 0)]), 0).unwrap();
    drop(index);
    assert_eq!(up.map(|e| e.unwrap().1).last(), Some(5_000), "the far end's new key, ascending");
    assert_eq!(down.map(|e| e.unwrap().1).last(), Some(0), "and descending");
}

/// Verifies: [DESIGN] RULES.md D31, counts: threads inserting and removing
/// the same entry at once lose no change; an entry inserted 300 times
/// stays until removed 300 times.
#[test]
fn counts_survive_threads() {
    let index = index(&[MONEY]);
    let key = || Key(vec![exact(1, 0)]);
    thread::scope(|s| {
        for _ in 0..4_u32 {
            s.spawn(|| {
                for _ in 0..100_u32 {
                    index.insert(key(), 9).unwrap();
                }
                for _ in 0..25_u32 {
                    index.remove(&key(), 9).unwrap();
                }
            });
        }
    });
    let left = |of: &BLinkIndex| of.tree.get(&of.entry(&key(), 9).unwrap()).unwrap().map(|v| count_of(&v).unwrap());
    assert_eq!(left(&index), Some(300));
    for n in (0..300).rev() {
        index.remove(&key(), 9).unwrap();
        assert_eq!(left(&index), (n > 0).then_some(n));
    }
}

/// Verifies: [DESIGN] RULES.md D31, refusals: an entry longer than a node
/// holds is refused with SQLCODE -900 and nothing is added; an entry whose
/// key is not a key of the index's columns, or whose count is malformed,
/// is reported with SQLCODE -916.
#[test]
fn long_and_malformed_entries_are_refused() {
    let most = index(&[MONEY]).tree.max_entry();
    let length = u32::try_from(most).unwrap();
    let index = index(&[DataType::Character { length }]);
    let long = Key(vec![Value::Char("x".repeat(most))]);
    assert_eq!(index.insert(long, 1).err().map(|e| e.code), Some(Code::Unsupported));
    assert_eq!(searched(&index, &Probe::All).unwrap().len(), 0);
    index.tree.insert(&[1, b'a'], &[]).unwrap();
    assert_eq!(searched(&index, &Probe::All).err().map(|e| e.code), Some(Code::Corrupt), "a key shorter than a row id");
    assert_eq!(index.estimate(&Probe::All), None, "an estimate that fails is not known");
    let numbers = self::index(&[MONEY]);
    let entry = numbers.entry(&Key(vec![exact(1, 0)]), 5).unwrap();
    numbers.tree.insert(&entry, &[1, 2]).unwrap();
    assert_eq!(numbers.insert(Key(vec![exact(1, 0)]), 5).err().map(|e| e.code), Some(Code::Corrupt), "a count of two bytes");
    assert_eq!(numbers.remove(&Key(vec![exact(1, 0)]), 5).err().map(|e| e.code), Some(Code::Corrupt));
    assert_eq!(numbers.rows(&Key(vec![Value::Char("a".into())])).err().map(|e| e.code), Some(Code::Api), "a key of another type");
}

/// Verifies: [EXTENSION] the method: "btree", ordered, every operator but
/// `<>`; indexes made in memory share one file, made once.
#[test]
fn the_method_makes_indexes() {
    let method = BTreeMethod::in_memory();
    assert_eq!((method.name(), method.ordered(), method.supports(CompOp::Ne), method.supports(CompOp::Le)), ("btree", true, false, true));
    let (a, b): (Arc<dyn Index>, Arc<dyn Index>) =
        (Arc::from(method.create(&[MONEY]).unwrap()), Arc::from(method.create(&[MONEY]).unwrap()));
    a.insert(Key(vec![exact(1, 0)]), 1).unwrap();
    assert_eq!(searched(&b, &Probe::All).unwrap().len(), 0, "each index its own tree");
    assert_eq!(searched(&a, &Probe::All).unwrap().len(), 1);
}

/// Verifies: [EXTENSION] RULES.md D34, a dropped index: its pages, every
/// level's and its meta page, are freed when the last reference to it goes,
/// not when it is discarded, and a new index reuses them; an index not
/// discarded keeps its pages when it goes, as when the database closes.
#[test]
fn a_discarded_index_frees_its_pages_when_it_goes() {
    let fs = crate::disk::memory::MemoryFileSystem::new();
    let hash: Arc<dyn crate::hash::HashFunction> = Arc::new(crate::hash::crc::Crc::new(crate::hash::crc::CRC32_ISCSI).unwrap());
    let pager = Arc::new(
        crate::disk::pager::Pager::create(
            &fs,
            std::path::Path::new("/p"),
            hash,
            crate::disk::encoding::Pipeline::none(),
            crate::disk::pager::DEFAULT_PAGE_SIZE,
        )
        .unwrap(),
    );
    let pool = Arc::new(BufferPool::new(64, Box::new(crate::disk::buffer::Clock::new(64))).unwrap());
    let file = pool.add_pager(Arc::clone(&pager));
    let insert_rows = |index: &BLinkIndex| {
        for row in 0..3_000 {
            index.insert(Key(vec![exact(i128::from(row), 2)]), row).unwrap();
        }
    };
    let kept = BLinkIndex::with(BLinkTree::create(Arc::clone(&pool), file).unwrap(), &[MONEY], KeyCounts::new());
    insert_rows(&kept);
    let in_use = pager.page_count() - pager.free_pages();
    drop(kept);
    assert_eq!(pager.page_count() - pager.free_pages(), in_use, "not discarded: kept");
    let dropped = Arc::new(BLinkIndex::with(BLinkTree::create(Arc::clone(&pool), file).unwrap(), &[MONEY], KeyCounts::new()));
    insert_rows(&dropped);
    assert!(dropped.tree.height().unwrap() > 1, "a tree of several levels");
    let reader = Arc::clone(&dropped);
    let (free, size) = (pager.free_pages(), pager.page_count());
    dropped.discard();
    drop(dropped);
    assert_eq!(pager.free_pages(), free, "a reader still holds it");
    assert_eq!(searched(&reader, &Probe::Compare { op: CompOp::Eq, value: exact(42, 2) }).unwrap().len(), 1);
    drop(reader);
    let freed = pager.free_pages() - free;
    assert_eq!(size - free - freed, in_use, "every page of the dropped tree freed");
    let again = BLinkIndex::with(BLinkTree::create(Arc::clone(&pool), file).unwrap(), &[MONEY], KeyCounts::new());
    insert_rows(&again);
    assert_eq!(pager.page_count(), size, "its pages reused, the file not grown");
}

/// Verifies: [X3.135-1986 §5.11 GR2, GR4, GR5]; [EXTENSION] RULES.md D34,
/// prefix probes of an index on two columns: the keys whose first value
/// equals the probe's, and whose second then compares by each operator,
/// exactly those SQL compares so and in key order, also for values not of
/// their column's type (10.555 against a scale of 2, a string padded past
/// its column); none with a NULL value.
#[test]
fn prefix_probes_find_what_sql_compares() {
    const CODE: DataType = DataType::Character { length: 3 };
    let index = index(&[MONEY, CODE]);
    let mut keys = Vec::new();
    for units in [1_000, 1_055, 1_056] {
        for code in ["A", "AB", "B", "BB"] {
            keys.push(Key(vec![exact(units, 2), Value::Char(code.into())]));
        }
    }
    for (row, key) in (1..).zip(&keys) {
        index.insert(key.clone(), row).unwrap();
    }
    let firsts = [exact(1_055, 2), exact(10_550, 3), exact(10_555, 3), Value::Approx(10.56)];
    let seconds = [Value::Char("AB".into()), Value::Char("AB    ".into()), Value::Char("ABC ".into()), Value::Char(String::new())];
    let ops = [CompOp::Eq, CompOp::Lt, CompOp::Le, CompOp::Gt, CompOp::Ge, CompOp::Ne];
    let mut probes = vec![Probe::Prefix { equal: vec![Value::Null], then: None }];
    for first in &firsts {
        probes.push(Probe::Prefix { equal: vec![first.clone()], then: None });
        for (op, second) in ops.iter().flat_map(|op| seconds.iter().map(move |s| (*op, s))) {
            probes.push(Probe::Prefix { equal: vec![first.clone()], then: Some((op, second.clone())) });
        }
        probes.push(Probe::Prefix { equal: vec![first.clone()], then: Some((CompOp::Lt, Value::Null)) });
        probes.push(Probe::Prefix { equal: vec![first.clone(), Value::Char("B".into())], then: None });
    }
    for probe in probes {
        let expected: Vec<RowId> = (1..).zip(&keys).filter(|(_, k)| probe.answers(&k.0)).map(|(r, _)| r).collect();
        let got: Vec<RowId> = searched(&index, &probe).unwrap().into_iter().map(|(_, row)| row).collect();
        assert_eq!(got, expected, "{probe:?}");
        assert_eq!(index.estimate(&probe), Some(expected.len()), "{probe:?}");
    }
}

/// Verifies: [DESIGN] RULES.md D34, `Index::insert_alone`: inserting
/// entries of keys, many repeated, in an order that spreads them over many
/// leaves, the index never says an entry is its key's only one when an
/// entry of another row holds the key; it says so of most that are; an
/// entry of the same row again, another version's, leaves it alone; and
/// the method's default, of `insert` and `rows`, says what the truth is.
#[test]
fn an_entry_alone_is_its_keys_only_one() {
    let text = DataType::Varying { length: 40 };
    let index = index(&[text]);
    let mut held: std::collections::HashMap<u64, Vec<RowId>> = std::collections::HashMap::new();
    let (mut alone, mut truly) = (0_u32, 0_u32);
    for row in 1..=6000_u64 {
        // Keys scattered, a third of them held by an earlier row too.
        let k = row.checked_mul(7919).and_then(|n| n.checked_rem(4000)).unwrap();
        let key = Key(vec![Value::Char(format!("key {k:05} and a long tail to fill leaves"))]);
        let others = held.get(&k).is_some_and(|rows| !rows.is_empty());
        let said = index.insert_alone(key, row).unwrap();
        assert!(!(said && others), "row {row}, key {k}: an entry of another row holds it");
        truly = truly.saturating_add(u32::from(!others));
        alone = alone.saturating_add(u32::from(said));
        held.entry(k).or_default().push(row);
    }
    assert!(alone.saturating_mul(2) > truly && alone <= truly, "{alone} of {truly} alone");
    let again = Key(vec![Value::Char("again".into())]);
    assert!(!index.insert_alone(again.clone(), 9001).unwrap() || index.rows(&again).unwrap() == vec![9001]);
    let repeated = index.insert_alone(again.clone(), 9001).unwrap();
    assert!(!repeated || index.rows(&again).unwrap() == vec![9001], "the same row's other version");
    assert!(!index.insert_alone(again, 9002).unwrap(), "another row's");
}

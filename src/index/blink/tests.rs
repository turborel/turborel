//! Unit tests of the B-link index: probes of every operator with values of
//! the column's type and not, many entries in key order, estimates, counts
//! changed by threads at once, entries too long, and malformed entries,
//! and the pages its probes miss (RULES.md R-TEST-1, R-TEST-2).

use std::path::Path;
use std::thread;

use super::*;
use crate::disk::buffer::Clock;
use crate::disk::encoding::Pipeline;
use crate::disk::memory::MemoryFileSystem;
use crate::disk::pager::Pager;
use crate::engine::memory_file;
use crate::engine::tree::hints;
use crate::hash::crc::{CRC32_ISCSI, Crc};
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
    let index = index(&[DataType::character(3)]);
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
/// a key's rows in row order; an estimate of a range past 128 entries is
/// its share of the tree, near its count; entries removed are gone.
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
    let near = |got: Option<usize>, want: f64| got.is_some_and(|n| (f64::from(u32::try_from(n).unwrap()) / want - 1.0_f64).abs() < 0.5_f64);
    assert!(near(index.estimate(&Probe::Compare { op: CompOp::Lt, value: exact(10, 0) }), 400.0_f64));
    assert!(near(index.estimate(&Probe::Compare { op: CompOp::Gt, value: exact(10, 0) }), 19_560.0_f64));
    assert_eq!(index.estimate(&Probe::Compare { op: CompOp::Lt, value: exact(3, 0) }), Some(120), "counted");
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
    let most = index(&[MONEY]).tree.max_key();
    let length = u32::try_from(most).unwrap();
    let index = index(&[DataType::character(length)]);
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

/// Verifies: [DESIGN] CONFORMANCE.md, Limits: an entry an index takes
/// fits its tree, and one past the longest it takes is SQLCODE -900, the
/// tree counting a value as 8 bytes never refusing what the index took.
#[test]
fn the_longest_entry_fits_its_tree() {
    let most = index(&[MONEY]).tree.max_key();
    let (mut taken, mut refused) = (false, false);
    for length in most.saturating_sub(16)..most {
        let index = index(&[DataType::character(u32::try_from(length).unwrap())]);
        match index.insert(Key(vec![Value::Char("x".into())]), 1) {
            Ok(()) => taken = true,
            Err(e) => {
                assert_eq!(e.code, Code::Unsupported, "of {length} characters");
                refused = true;
            }
        }
    }
    assert!(taken && refused);
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
    let fs = MemoryFileSystem::new();
    let hash: Arc<dyn crate::hash::HashFunction> = Arc::new(Crc::new(CRC32_ISCSI).unwrap());
    let pager = Arc::new(Pager::create(&fs, Path::new("/p"), hash, Pipeline::none(), crate::disk::pager::DEFAULT_PAGE_SIZE).unwrap());
    let pool = Arc::new(BufferPool::new(64, Box::new(Clock::new(64))).unwrap());
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
    const CODE: DataType = DataType::character(3);
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
    let text = DataType::varying(40);
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

/// Verifies: [DESIGN] RULES.md D35, `Index::maintenance_due`,
/// `Index::maintain`: an index opened by a locator without key counts has
/// no distinct keys, and asking makes its count due, with no pass; its
/// maintenance then counts them, once. Regression test (R-TEST-7): the
/// first to ask counted them in a pass over every entry, inside planning.
#[test]
fn an_index_opened_without_counts_counts_its_keys_in_maintenance() {
    let method = BTreeMethod::in_memory();
    let index = Arc::<dyn Index>::from(method.create(&[MONEY]).unwrap());
    for row in 0..300 {
        index.insert(Key(vec![exact(i128::from(row % 3), 0)]), row).unwrap();
    }
    assert!(!index.maintenance_due(), "counted as written");
    assert_eq!(index.maintain().ok(), Some(false));
    let full = index.locator();
    let again = method.open(&[MONEY], &full[..8]).unwrap().unwrap();
    assert!(!again.maintenance_due(), "not yet asked for");
    assert_eq!(again.distinct_keys(), None);
    assert!(again.maintenance_due());
    assert_eq!(again.maintain().ok(), Some(true));
    assert!(!again.maintenance_due());
    assert!(again.distinct_keys().is_some_and(|d| (d - 3.0_f64).abs() < 0.5_f64), "{:?}", again.distinct_keys());
    assert_eq!(again.maintain().ok(), Some(false), "once");
}

/// Verifies: [DESIGN] RULES.md D35, `Index::probe_missed`: an index tells
/// the pages its probes miss: before its tree has counted `LEAST_LOOKUPS`
/// lookups, its leaf, as the pool has no frames for its file's pages, none
/// of a pool that holds it and some of one of 16 frames; then as its
/// searches missed, a scattered key at a time.
#[test]
fn an_index_tells_the_pages_its_probes_miss() {
    let held = index(&[MONEY]);
    let crc = Arc::new(Crc::new(CRC32_ISCSI).unwrap());
    let pager = Pager::create(&MemoryFileSystem::new(), Path::new("/ix"), crc, Pipeline::none(), 4096).unwrap();
    let pool = Arc::new(BufferPool::new(16, Box::new(Clock::new(16))).unwrap());
    let file = pool.add_pager(Arc::new(pager));
    let small = BLinkIndex::with(BLinkTree::create(pool, file).unwrap(), &[MONEY], KeyCounts::new());
    for row in 0..20_000_u64 {
        let key = Key(vec![exact(i128::from(row), 0)]);
        held.insert(key.clone(), row).unwrap();
        small.insert(key, row).unwrap();
    }
    assert_eq!(held.probe_missed(), Some(0.0_f64), "held: none past the pool");
    let unheld = small.probe_missed().unwrap();
    assert!(unheld > 0.5_f64 && unheld < 1.0_f64, "{unheld}");
    for n in 0..LEAST_LOOKUPS {
        let units = i128::from(n.wrapping_mul(7919_u64) % 20_000_u64);
        assert_eq!(small.rows(&Key(vec![exact(units, 0)])).unwrap().len(), 1_usize);
    }
    let missed = small.probe_missed().unwrap();
    assert!(missed > 0.0_f64 && (missed - unheld).abs() > 1e-9_f64, "as its searches missed: {missed}");
}

/// Verifies: [DESIGN] RULES.md D500, an entry's hint: an entry takes the
/// leaf noted for its row as it is written; a second version's entry
/// counts it and takes the newer leaf, and a removal keeps it; a read
/// notes each entry's hint for the row's read; a leaf past what a hint
/// holds is kept as none; and a load gives each entry the leaf noted as
/// its row was read, its versions counted as one entry.
#[test]
fn entries_carry_the_leaf_noted_for_their_row() {
    let index = index(&[MONEY]);
    let key = || Key(vec![exact(5, 0)]);
    let value = |of: &BLinkIndex, row| of.tree.get(&of.entry(&key(), row).unwrap()).unwrap().unwrap();
    hints::note(41, 1_234);
    index.insert(key(), 41).unwrap();
    assert_eq!((hint_of(&value(&index, 41)).unwrap(), count_of(&value(&index, 41)).unwrap()), (1_234, 1));
    hints::note(41, 99);
    index.insert(key(), 41).unwrap();
    assert_eq!((hint_of(&value(&index, 41)).unwrap(), count_of(&value(&index, 41)).unwrap()), (99, 2));
    index.remove(&key(), 41).unwrap();
    assert_eq!((hint_of(&value(&index, 41)).unwrap(), count_of(&value(&index, 41)).unwrap()), (99, 1));
    hints::note(41, 5);
    assert_eq!(searched(&index, &Probe::All).unwrap().len(), 1);
    assert_eq!(hints::leaf_of(41), Some(99), "a read notes the entry's hint");
    hints::note(42, u64::from(u32::MAX) + 1);
    index.insert(key(), 42).unwrap();
    assert_eq!(hint_of(&value(&index, 42)).unwrap(), hints::NO_LEAF);
    let loaded = self::index(&[MONEY]);
    let mut rows = [3_u64, 1, 2, 2].into_iter().map(|row| {
        hints::note(row, 500 + row);
        Ok((key(), row))
    });
    let areas = WorkAreas::new(MemoryBudget::new(usize::MAX));
    loaded.load(&mut rows, (&areas, Area { bytes: 4 << 10_u32, batch: 1 << 10_u32 })).unwrap();
    let held: Vec<(u64, u32)> =
        (1..=3).map(|row| (hint_of(&value(&loaded, row)).unwrap(), count_of(&value(&loaded, row)).unwrap())).collect();
    assert_eq!(held, [(501, 1), (502, 2), (503, 1)]);
}

/// Verifies: [DESIGN] RULES.md D501, D786: a search decodes a key once
/// for the entries of one key past its second, and again for each other
/// key, each decoded whole; a key is kept only once a second entry has its
/// bytes, so a key whose entry is alone is never copied.
///
/// Decisions: the bytes are the last decoded's or not; a key is kept or
/// not. Each changes the outcome alone: a new key keeps none; its second
/// entry keeps it; its third takes the one kept; another key drops it.
#[test]
fn a_key_is_decoded_once_for_its_entries() {
    let types = [MONEY, DataType::character(4)];
    let index = index(&types);
    let key = |units| Key(vec![exact(units, 2), Value::Char("ab  ".into())]);
    let mut last = Decoded::default();
    let (one, two) = (index.prefix(&key(100)).unwrap(), index.prefix(&key(250)).unwrap());
    assert_eq!(last.of(&one, &types).unwrap(), key(100));
    assert_eq!((last.bytes.as_deref(), last.key.clone()), (Some(one.as_slice()), None));
    assert_eq!(last.of(&one, &types).unwrap(), key(100));
    assert_eq!(last.key, Some(key(100)));
    assert_eq!(last.of(&one, &types).unwrap(), key(100));
    assert_eq!(last.of(&two, &types).unwrap(), key(250));
    assert_eq!((last.bytes.as_deref(), last.key.clone()), (Some(two.as_slice()), None));
    for row in 1..=5 {
        index.insert(key(100), row).unwrap();
    }
    assert_eq!(found(&index, CompOp::Eq, exact(100, 2)), [1, 2, 3, 4, 5]);
}

/// Verifies: [DESIGN] RULES.md D821: an encoded search gives each entry
/// `search` gives, in key order and reversed, its key the bytes of its
/// values over the index's types; a probe each key of which is checked,
/// by a value of a finer scale than its column's, is not searched so.
#[test]
fn an_encoded_search_gives_the_keys_search_finds() {
    let index = index(&[MONEY]);
    for (row, units) in (1..).zip([-1_000, 1_055, 1_056, 1_060, 9_999]) {
        index.insert(Key(vec![exact(units, 2)]), row).unwrap();
    }
    let (low, high) = (Box::new((CompOp::Ge, exact(1_055, 2))), Box::new((CompOp::Lt, exact(9_999, 2))));
    let probes = [Probe::All, Probe::Compare { op: CompOp::Eq, value: exact(1_056, 2) }, Probe::Range { equal: Vec::new(), low, high }];
    for probe in probes {
        for order in [KeyOrder::Ascending, KeyOrder::Descending] {
            let Encoded { entries, types } = Arc::clone(&index).search_encoded(&probe, order).unwrap();
            let decoded = |(bytes, row): (Vec<u8>, RowId)| Ok((Key(decode_key(&bytes, &types)?), row));
            let encoded: Vec<(Key, RowId)> = entries.map(|e| e.and_then(decoded)).collect::<Result<_>>().unwrap();
            let keyed: Vec<(Key, RowId)> = Arc::clone(&index).search(&probe, order).collect::<Result<_>>().unwrap();
            assert!(!keyed.is_empty() && encoded == keyed, "{probe:?} {order:?}");
        }
    }
    let finer = Probe::Compare { op: CompOp::Eq, value: exact(10_555, 3) };
    assert!(Arc::clone(&index).search_encoded(&finer, KeyOrder::Ascending).is_none());
}

/// Verifies: [DESIGN] RULES.md D381, range estimates: of a tree of
/// 100,000 entries inserted in key order and in an order shuffled from a
/// fixed seed, a range of any size past 128 entries is estimated by its
/// share of the tree within 1.5 times the entries it holds, counted; one
/// of fewer is counted exactly; the estimate is never fewer than the
/// entries counted before it stopped (D302). MC/DC of the choice: a range
/// within `SMALL_COUNT` (counted) against one past it (its share).
#[test]
fn a_range_is_estimated_by_its_share_of_the_tree() {
    for shuffled in [false, true] {
        let index = index(&[MONEY]);
        let mut keys: Vec<u64> = (0..100_000_u64).collect();
        let mut random = 11_u64;
        for i in (1..keys.len()).rev().filter(|_| shuffled) {
            let j = usize::try_from(
                crate::stats::parts::xorshift(&mut random).checked_rem(u64::try_from(i).unwrap().saturating_add(1)).unwrap(),
            )
            .unwrap();
            keys.swap(i, j);
        }
        for (row, &k) in (0_u64..).zip(&keys) {
            index.insert(Key(vec![exact(i128::from(k), 2)]), row).unwrap();
        }
        let between = |low: i128, high: i128| Probe::Range {
            equal: Vec::new(),
            low: Box::new((CompOp::Ge, exact(low, 2))),
            high: Box::new((CompOp::Lt, exact(high, 2))),
        };
        for (low, high) in [(0_i128, 100_i128), (1_000, 1_129), (5_000, 6_000), (20_000, 70_000), (0, 100_000), (99_000, 100_000)] {
            let truth = f64::from(u32::try_from(high - low).unwrap());
            let got = index.estimate(&between(low, high)).unwrap();
            let ratio = f64::from(u32::try_from(got).unwrap()) / truth;
            assert!(ratio < 1.5_f64 && ratio > 1.0_f64 / 1.5_f64, "{low}..{high}: {got}, shuffled {shuffled}");
            if high - low <= 128 {
                assert_eq!(got, usize::try_from(high - low).unwrap(), "counted");
            } else {
                assert!(got > SMALL_COUNT, "no fewer than counted");
            }
        }
    }
}

/// Verifies: [DESIGN] RULES.md D383: statistics read with a table's
/// indexes give, of columns exactly an index's key, its distinct keys,
/// correlated columns counted once, in any order the columns are asked;
/// of a column alone in a key, its index's count; of columns holding a
/// unique key's, the rows; of other columns, none.
#[test]
fn an_indexs_columns_are_as_distinct_as_its_keys() {
    use crate::stats::StatsBuilder;
    use crate::stats::keyed::{KeyIndex, Keyed};
    let pair = index(&[MONEY, MONEY]);
    let one = index(&[MONEY]);
    let mut builder = StatsBuilder::new(3);
    for row in 0..8_000_u64 {
        let (x, y) = (i128::from(row % 200), i128::from(row % 200 % 20));
        pair.insert(Key(vec![exact(x, 0), exact(y, 0)]), row).unwrap();
        one.insert(Key(vec![exact(i128::from(row), 0)]), row).unwrap();
        builder.add(&vec![exact(x, 0), exact(y, 0), exact(i128::from(row), 0)], 9);
    }
    let pair: Arc<dyn Index> = pair;
    let one: Arc<dyn Index> = one;
    let keys = vec![KeyIndex::new(&[0, 1], false, pair), KeyIndex::new(&[2], true, one)];
    let stats = Keyed::of(Arc::new(builder.finish()), keys);
    let near = |got: Option<f64>, want: f64| got.is_some_and(|g| (g / want - 1.0_f64).abs() < 0.05_f64);
    assert!(near(stats.group_distinct(&[1, 0]), 200.0_f64), "{:?}", stats.group_distinct(&[1, 0]));
    assert!(near(stats.distinct(2), 8_000.0_f64), "a column alone in a key");
    assert_eq!(stats.group_distinct(&[0, 2]), Some(8_000.0_f64), "holding a unique key");
    assert_eq!(stats.group_distinct(&[0, 1, 2]), Some(8_000.0_f64));
    assert!(near(stats.group_distinct(&[1]), 20.0_f64), "one column");
    let product = stats.distinct(0).unwrap() * stats.distinct(1).unwrap();
    assert!(product > 3_000.0_f64, "the product counts every pair: {product}");
}

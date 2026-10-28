//! The B-link tree (RULES.md D31; Lehman and Yao 1981): entries added,
//! replaced, removed and found; trees of several levels scanned in both
//! directions within bounds; nodes filled in order kept full; a tree
//! larger than its buffer pool; a tree found again after its file is
//! reopened; malformed pages refused; and threads reading and writing at
//! once.

use std::collections::BTreeMap;
use std::ops::Bound;
use std::sync::Arc;
use std::thread;

use turborel::disk::buffer::{BufferPool, Clock, FileId};
use turborel::disk::encoding::Pipeline;
use turborel::disk::io::{Durability, OsFileSystem};
use turborel::disk::pager::Pager;
use turborel::engine::tree::{BLinkTree, Direction};
use turborel::error::Code;

use crate::common::{Scratch, crc32c};

/// A pool of `frames` frames over a new pager file of 4 KiB pages.
fn setup(scratch: &Scratch, frames: usize) -> (Arc<BufferPool>, FileId, Arc<Pager>) {
    let pager = Arc::new(Pager::create(&OsFileSystem, &scratch.path("tree"), crc32c(), Pipeline::none(), 4096).unwrap());
    let pool = Arc::new(BufferPool::new(frames, Box::new(Clock::new(frames))).unwrap());
    let file = pool.add_pager(Arc::clone(&pager));
    (pool, file, pager)
}

fn key(n: u64) -> Vec<u8> {
    n.to_be_bytes().to_vec()
}

fn value(n: u64) -> Vec<u8> {
    format!("value {n} {}", "x".repeat(usize::try_from(n % 40).unwrap())).into_bytes()
}

/// `0..n` in a scattered order: 7919 is prime, so coprime to `n` unless a
/// multiple.
fn scattered(n: u64) -> impl Iterator<Item = u64> {
    (0..n).map(move |i| (i * 7919) % n)
}

fn all(tree: &BLinkTree, lower: Bound<&[u8]>, upper: Bound<&[u8]>, direction: Direction) -> Vec<(Vec<u8>, Vec<u8>)> {
    tree.scan(lower, upper, direction).collect::<Result<_, _>>().unwrap()
}

/// Verifies: [DESIGN] RULES.md D31, B-link tree: an empty tree finds
/// nothing; an entry added is found; adding a key already present, or
/// replacing or removing one absent, changes nothing and says so; a value
/// replaced is found as replaced, and the replace returns the old value; a
/// key removed is gone, and the delete returns its value; `replace_if` and
/// `delete_if` change an entry only if it holds the value expected. An entry longer
/// than `max_entry` (a value under 8 bytes counting as 8) is refused with
/// SQLCODE -910.
#[test]
fn entries_are_added_replaced_and_removed() {
    let scratch = Scratch::new("tree-basic");
    let (pool, file, _) = setup(&scratch, 16);
    let tree = BLinkTree::create(pool, file).unwrap();
    assert_eq!((tree.get(b"a").unwrap(), tree.height().unwrap()), (None, 1));
    assert_eq!(all(&tree, Bound::Unbounded, Bound::Unbounded, Direction::Forward), []);
    assert!(tree.insert(b"a", b"1").unwrap());
    assert!(!tree.insert(b"a", b"2").unwrap(), "already present");
    assert_eq!(tree.get(b"a").unwrap(), Some(b"1".to_vec()));
    assert!(tree.replace(b"b", b"2").unwrap().is_none() && tree.delete(b"b").unwrap().is_none(), "absent");
    assert_eq!(tree.replace(b"a", b"longer").unwrap(), Some(b"1".to_vec()), "the value replaced");
    assert_eq!(tree.get(b"a").unwrap(), Some(b"longer".to_vec()));
    assert!(tree.insert(b"", b"empty key").unwrap());
    assert!(!tree.replace_if(b"", b"other", b"x").unwrap() && !tree.delete_if(b"", b"other").unwrap(), "not the value expected");
    assert!(!tree.replace_if(b"none", b"", b"x").unwrap() && !tree.delete_if(b"none", b"").unwrap(), "absent");
    assert!(tree.replace_if(b"", b"empty key", b"swapped").unwrap());
    assert_eq!(tree.get(b"").unwrap(), Some(b"swapped".to_vec()));
    assert!(tree.delete_if(b"", b"swapped").unwrap());
    assert!(tree.insert(b"", b"empty key").unwrap());
    assert_eq!(tree.get(b"").unwrap(), Some(b"empty key".to_vec()));
    assert_eq!(tree.delete(b"a").unwrap(), Some(b"longer".to_vec()), "the value removed");
    assert_eq!(tree.get(b"a").unwrap(), None);
    let max = tree.max_entry();
    assert!(tree.insert(&vec![1; max - 8], b"").unwrap(), "a short value counts as 8 bytes");
    let refused = |k: &[u8], v: &[u8]| tree.insert(k, v).err().map(|e| e.code);
    assert_eq!(refused(&vec![2; max - 7], b""), Some(Code::Api));
    assert_eq!(refused(b"k", &vec![0; max]), Some(Code::Api));
    assert!(tree.replace(b"", &vec![0; max]).unwrap().is_some(), "exactly the most");
    assert_eq!(tree.replace(b"", &vec![0; max + 1]).err().map(|e| e.code), Some(Code::Api));
}

/// Verifies: [DESIGN] RULES.md D31, B-link tree (Lehman and Yao §4):
/// entries added in a scattered order split leaves and inner nodes until
/// the tree has at least three levels, each root split adding one; every
/// entry is found, and a scan returns them all in key order forward and in
/// reverse backward.
#[test]
fn trees_grow_levels_and_keep_order() {
    let scratch = Scratch::new("tree-levels");
    let (pool, file, _) = setup(&scratch, 512);
    let tree = BLinkTree::create(pool, file).unwrap();
    let n = 20_000;
    for i in scattered(n) {
        assert!(tree.insert(&key(i), &value(i)).unwrap());
    }
    let height = tree.height().unwrap();
    let counts = tree.counts();
    assert!(height >= 3, "{height} levels");
    assert_eq!(counts.root_splits, u64::from(height) - 1);
    assert!(counts.splits > counts.root_splits);
    for i in scattered(n) {
        assert_eq!(tree.get(&key(i)).unwrap(), Some(value(i)), "key {i}");
    }
    assert_eq!(tree.get(&key(n)).unwrap(), None);
    let expected: Vec<_> = (0..n).map(|i| (key(i), value(i))).collect();
    assert_eq!(all(&tree, Bound::Unbounded, Bound::Unbounded, Direction::Forward), expected);
    let mut reversed = expected;
    reversed.reverse();
    assert_eq!(all(&tree, Bound::Unbounded, Bound::Unbounded, Direction::Backward), reversed);
}

/// Verifies: [DESIGN] RULES.md D31, B-link scans: within bounds included,
/// excluded or absent at either end, at keys present and absent, in the
/// middle of leaves and at the ends of the tree, both directions return
/// exactly the keys a sorted map returns, in order.
#[test]
fn scans_honour_bounds_in_both_directions() {
    let scratch = Scratch::new("tree-bounds");
    let (pool, file, _) = setup(&scratch, 256);
    let tree = BLinkTree::create(pool, file).unwrap();
    // Even keys only, so odd bounds fall between keys.
    let model: BTreeMap<Vec<u8>, Vec<u8>> = (0..3000).map(|i| (key(i * 2), value(i))).collect();
    for (k, v) in &model {
        tree.insert(k, v).unwrap();
    }
    let points = [0, 1, 2, 777, 1000, 1001, 3001, 5998, 5999, 6000, 9999];
    for &low in &points {
        for &high in &points {
            let (lk, hk) = (key(low), key(high));
            let bounds = [
                (Bound::Included(&lk[..]), Bound::Included(&hk[..])),
                (Bound::Excluded(&lk[..]), Bound::Excluded(&hk[..])),
                (Bound::Included(&lk[..]), Bound::Unbounded),
                (Bound::Unbounded, Bound::Excluded(&hk[..])),
            ];
            for (lower, upper) in bounds {
                // A sorted map refuses crossed bounds, and equal excluded
                // ones; the tree's answer to those is checked below.
                let bounded = lower != Bound::Unbounded && upper != Bound::Unbounded;
                if bounded && (low > high || (low == high && matches!(lower, Bound::Excluded(_)))) {
                    continue;
                }
                let expected: Vec<_> = model.range::<[u8], _>((lower, upper)).map(|(k, v)| (k.clone(), v.clone())).collect();
                assert_eq!(all(&tree, lower, upper, Direction::Forward), expected, "{lower:?}..{upper:?} forward");
                let mut reversed = expected;
                reversed.reverse();
                assert_eq!(all(&tree, lower, upper, Direction::Backward), reversed, "{lower:?}..{upper:?} backward");
            }
        }
    }
    let (from, to) = (key(5), key(3));
    assert_eq!(all(&tree, Bound::Included(&from), Bound::Included(&to), Direction::Forward), [], "bounds crossed");
    assert_eq!(all(&tree, Bound::Included(&from), Bound::Included(&to), Direction::Backward), []);
    let four = key(4);
    for direction in [Direction::Forward, Direction::Backward] {
        assert_eq!(all(&tree, Bound::Excluded(&four), Bound::Excluded(&four), direction), [], "equal excluded bounds");
        assert_eq!(all(&tree, Bound::Included(&four), Bound::Included(&four), direction), [(four.clone(), value(2))]);
    }
}

/// Verifies: [DESIGN] RULES.md D31, B-link split: keys added in increasing
/// order, as row ids are, leave every node but the last of each level
/// full, so the tree takes few more pages than its entries' bytes fill;
/// the same keys in a scattered order, split evenly, take more.
#[test]
fn keys_added_in_order_fill_their_nodes() {
    let pages = |order: &mut dyn Iterator<Item = u64>| {
        let scratch = Scratch::new("tree-fill");
        let (pool, file, pager) = setup(&scratch, 1024);
        let tree = BLinkTree::create(pool, file).unwrap();
        for i in order {
            tree.insert(&key(i), &[7; 92]).unwrap();
        }
        pager.page_count()
    };
    let n = 20_000_u64;
    // An entry takes 106 bytes (8-byte key, 92-byte value, 6 of overhead):
    // 38 fit a 4 KiB page's 4,056 bytes after the node's header.
    let leaves = n.div_ceil(38);
    let in_order = pages(&mut (0..n));
    assert!(in_order * 20 <= leaves * 21, "{in_order} pages for {leaves} full leaves");
    let scattered_pages = pages(&mut scattered(n));
    assert!(scattered_pages * 5 > in_order * 6, "{scattered_pages} pages scattered, {in_order} in order");
}

/// Verifies: [DESIGN] RULES.md D31, B-link delete (Lehman and Yao §7;
/// Lanin and Shasha 1986): removing most entries empties whole leaves,
/// which are taken out of the tree, their pages freed once no operation
/// can hold them; the rest are found and scanned in order both ways; the
/// keys removed can be added again. Replacing values with longer ones
/// splits leaves as inserting does.
#[test]
fn emptied_leaves_are_taken_out() {
    let scratch = Scratch::new("tree-delete");
    let (pool, file, pager) = setup(&scratch, 256);
    let tree = BLinkTree::create(pool, file).unwrap();
    let n = 5000;
    for i in 0..n {
        tree.insert(&key(i), &value(i)).unwrap();
    }
    let kept = |i: u64| i % 1000 == 999 || i < 3;
    for i in scattered(n).filter(|&i| !kept(i)) {
        assert!(tree.delete(&key(i)).unwrap().is_some());
    }
    assert!(tree.counts().unlinked > 20, "{:?}", tree.counts());
    assert!(pager.free_pages() > 10, "their pages freed: {}", pager.free_pages());
    let expected: Vec<_> = (0..n).filter(|&i| kept(i)).map(|i| (key(i), value(i))).collect();
    assert_eq!(all(&tree, Bound::Unbounded, Bound::Unbounded, Direction::Forward), expected);
    let mut reversed = expected.clone();
    reversed.reverse();
    assert_eq!(all(&tree, Bound::Unbounded, Bound::Unbounded, Direction::Backward), reversed);
    assert_eq!(tree.get(&key(500)).unwrap(), None);
    let splits = tree.counts().splits;
    for i in 0..n {
        let long = vec![b'v'; 200];
        if kept(i) {
            assert!(tree.replace(&key(i), &long).unwrap().is_some());
        } else {
            assert!(tree.insert(&key(i), &long).unwrap());
        }
    }
    assert!(tree.counts().splits > splits, "longer values split leaves again");
    assert_eq!(all(&tree, Bound::Unbounded, Bound::Unbounded, Direction::Forward).len(), usize::try_from(n).unwrap());
    assert!(tree.scan(Bound::Unbounded, Bound::Unbounded, Direction::Forward).all(|e| e.unwrap().1.len() == 200));
}

/// Verifies: [DESIGN] RULES.md D31, B-link replace: values replaced with
/// longer ones overfill their leaves, which split as inserts do; values
/// replaced with shorter ones, and with values of the same length, change
/// in place. Every key keeps its latest value, in order.
#[test]
fn replaced_values_split_leaves_when_they_grow() {
    let scratch = Scratch::new("tree-replace");
    let (pool, file, _) = setup(&scratch, 256);
    let tree = BLinkTree::create(pool, file).unwrap();
    let n = 3000;
    for i in 0..n {
        tree.insert(&key(i), b"short").unwrap();
    }
    let splits = tree.counts().splits;
    for i in scattered(n) {
        assert_eq!(tree.replace(&key(i), &vec![b'L'; 300]).unwrap(), Some(b"short".to_vec()));
    }
    let grown = tree.counts().splits;
    assert!(grown > splits * 4, "{grown} splits, {splits} before");
    for i in 0..n {
        assert!(tree.replace(&key(i), if i % 2 == 0 { b"tiny" } else { b"TINY" }).unwrap().is_some());
    }
    assert_eq!(tree.counts().splits, grown, "shorter values fit");
    let expected: Vec<_> = (0..n).map(|i| (key(i), if i % 2 == 0 { b"tiny".to_vec() } else { b"TINY".to_vec() })).collect();
    assert_eq!(all(&tree, Bound::Unbounded, Bound::Unbounded, Direction::Forward), expected);
}

/// Verifies: [DESIGN] RULES.md D31, B-link tree over the buffer pool: a
/// tree of many more pages than the pool's frames works, its nodes evicted
/// and read back; synced, and its file reopened in a new pool, it is found
/// again by its meta page, whole. A page that is not a tree's meta page is
/// refused with SQLCODE -916.
#[test]
fn trees_outgrow_the_pool_and_are_found_again() {
    let scratch = Scratch::new("tree-reopen");
    let (pool, file, _) = setup(&scratch, 8);
    let tree = BLinkTree::create(Arc::clone(&pool), file).unwrap();
    let n = 6000;
    for i in scattered(n) {
        tree.insert(&key(i), &value(i)).unwrap();
    }
    assert!(pool.counts().evictions > 0);
    let meta = tree.meta();
    drop(tree);
    pool.sync(file, Durability::Full).unwrap();
    drop(pool);
    let (pager, _) = Pager::open(&OsFileSystem, &scratch.path("tree"), &[crc32c()], &[]).unwrap();
    let pool = Arc::new(BufferPool::new(8, Box::new(Clock::new(8))).unwrap());
    let file = pool.add_pager(Arc::new(pager));
    let tree = BLinkTree::open(Arc::clone(&pool), file, meta).unwrap();
    for i in 0..n {
        assert_eq!(tree.get(&key(i)).unwrap(), Some(value(i)));
    }
    assert_eq!(all(&tree, Bound::Unbounded, Bound::Unbounded, Direction::Backward).len(), usize::try_from(n).unwrap());
    let (other, mut page) = pool.new_page(file).unwrap();
    page.extend_from_slice(b"not a meta page");
    drop(page);
    assert_eq!(BLinkTree::open(Arc::clone(&pool), file, other).err().map(|e| e.code), Some(Code::Corrupt));
}

/// Verifies: [DESIGN] RULES.md D31, B-link concurrency (Lehman and Yao §5,
/// §6): writers adding and removing their own keys while readers scan
/// never deadlock; each writer finds its keys as soon as it added them;
/// every scan sees keys strictly in order, never one twice; at the end the
/// tree holds exactly the keys added and not removed.
#[test]
fn threads_read_and_write_at_once() {
    let scratch = Scratch::new("tree-threads");
    let (pool, file, _) = setup(&scratch, 1024);
    let tree = Arc::new(BLinkTree::create(pool, file).unwrap());
    let (writers, per) = (4_u64, 6000_u64);
    let mut handles = Vec::new();
    for w in 0..writers {
        let tree = Arc::clone(&tree);
        handles.push(thread::spawn(move || {
            // Writer `w` owns the keys equal to `w` modulo `writers`, added
            // in a scattered order, every third removed again.
            for i in scattered(per) {
                let k = key(i * writers + w);
                assert!(tree.insert(&k, &value(i)).unwrap());
                assert_eq!(tree.get(&k).unwrap(), Some(value(i)));
                if i % 3 == 0 {
                    assert_eq!(tree.delete(&k).unwrap(), Some(value(i)));
                }
            }
        }));
    }
    for direction in [Direction::Forward, Direction::Backward] {
        let tree = Arc::clone(&tree);
        handles.push(thread::spawn(move || {
            for _ in 0..20 {
                let keys: Vec<Vec<u8>> = tree.scan(Bound::Unbounded, Bound::Unbounded, direction).map(|e| e.unwrap().0).collect();
                let ordered = keys.windows(2).all(|w| if direction == Direction::Forward { w[0] < w[1] } else { w[0] > w[1] });
                assert!(ordered, "a scan saw keys out of order or twice");
            }
        }));
    }
    for handle in handles {
        handle.join().unwrap();
    }
    let expected: Vec<Vec<u8>> = (0..writers * per).filter(|k| (k / writers) % 3 != 0).map(key).collect();
    let found: Vec<Vec<u8>> = tree.scan(Bound::Unbounded, Bound::Unbounded, Direction::Forward).map(|e| e.unwrap().0).collect();
    assert_eq!(found, expected);
}

/// Verifies: [DESIGN] RULES.md D31, taking out emptied leaves (Lanin and
/// Shasha 1986) while others read and write: writers empty whole runs of
/// leaves and fill them again, their leaves taken out and their ranges
/// given to their left neighbours, while readers look up keys that never
/// change, and always find them, and scan, seeing keys strictly in order,
/// never one twice; at the end the tree holds exactly what is left.
#[test]
fn leaves_are_taken_out_while_others_read_and_write() {
    let scratch = Scratch::new("tree-unlink-threads");
    let (pool, file, _) = setup(&scratch, 1024);
    let tree = Arc::new(BLinkTree::create(pool, file).unwrap());
    let (writers, per) = (4_u64, 3000_u64);
    // Each writer's block begins with a key that stays.
    for w in 0..writers {
        assert!(tree.insert(&key(w * per), &value(0)).unwrap());
    }
    let mut handles = Vec::new();
    for w in 0..writers {
        let tree = Arc::clone(&tree);
        handles.push(thread::spawn(move || {
            for round in 0..3 {
                for i in 1..per {
                    assert!(tree.insert(&key(w * per + i), &value(i)).unwrap());
                }
                let kept = |i: u64| round == 2 && i.is_multiple_of(7);
                for i in (1..per).filter(|&i| !kept(i)) {
                    assert_eq!(tree.delete(&key(w * per + i)).unwrap(), Some(value(i)));
                }
            }
        }));
    }
    for reader in 0..3_u64 {
        let tree = Arc::clone(&tree);
        handles.push(thread::spawn(move || {
            for pass in 0..40_u64 {
                let w = (pass + reader) % writers;
                assert_eq!(tree.get(&key(w * per)).unwrap(), Some(value(0)), "a key that stays");
                let direction = if (pass + reader) % 2 == 0 { Direction::Forward } else { Direction::Backward };
                let keys: Vec<Vec<u8>> = tree.scan(Bound::Unbounded, Bound::Unbounded, direction).map(|e| e.unwrap().0).collect();
                let ordered = keys.windows(2).all(|p| if direction == Direction::Forward { p[0] < p[1] } else { p[0] > p[1] });
                assert!(ordered, "a scan saw keys out of order or twice");
                assert!(keys.len() >= usize::try_from(writers).unwrap(), "the keys that stay");
            }
        }));
    }
    for handle in handles {
        handle.join().unwrap();
    }
    assert!(tree.counts().unlinked > 0);
    let expected: Vec<Vec<u8>> = (0..writers)
        .flat_map(|w| std::iter::once(w * per).chain((1..per).filter(|i| i % 7 == 0).map(move |i| w * per + i)))
        .map(key)
        .collect();
    let found: Vec<Vec<u8>> = tree.scan(Bound::Unbounded, Bound::Unbounded, Direction::Forward).map(|e| e.unwrap().0).collect();
    assert_eq!(found, expected);
}

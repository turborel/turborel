//! Unit tests of sparse leaves merged into their right neighbour (RULES.md
//! D814, `merge.rs`): a leaf thinned beside a thin neighbour merges, one
//! beside a full neighbour stays, a copy of a leaf merged since is not
//! read, a count and a backward scan across a merge take its keys once,
//! and scans and reads running while threads delete see every key that
//! stays, in order, none twice.

use std::sync::Barrier;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::*;

/// The leaf of `tree` whose range holds key `k`.
fn leaf_of(tree: &BLinkTree, k: u64) -> PageId {
    tree.descend(Target::Key(&key(k)), 0, &mut Path::default(), |_| Ok(())).unwrap().0
}

/// The keys of `1..=n` in the leaf that holds key `k`.
fn in_leaf(tree: &BLinkTree, n: u64, k: u64) -> Vec<u64> {
    let leaf = leaf_of(tree, k);
    (1..=n).filter(|&i| leaf_of(tree, i) == leaf).collect()
}

/// The number key `k` is.
fn number(k: &[u8]) -> u64 {
    u64::from_be_bytes(<[u8; 8]>::try_from(k).unwrap())
}

/// Deletes each of `keys` but every fourth.
fn thin(tree: &BLinkTree, keys: &[u64]) {
    for &k in keys.iter().filter(|k| !k.is_multiple_of(4)) {
        assert!(tree.delete(&key(k)).unwrap().is_some(), "key {k}");
    }
}

/// Verifies: [DESIGN] RULES.md D814: a leaf thinned to a quarter beside a
/// full right neighbour stays. Its left neighbour thinned so beside it
/// merges into it once sparse, as the two fit in three quarters of a page:
/// one leaf holds the keys of both, the one merged is marked deleted, and
/// every key left is found and scanned both ways, in order, none twice.
#[test]
fn a_sparse_leaf_merges_into_a_sparse_right_neighbour() {
    let (tree, _scratch) = tree("merge-sparse");
    fill(&tree, 400);
    let left = in_leaf(&tree, 400, 200);
    let right = in_leaf(&tree, 400, left.last().unwrap().checked_add(1).unwrap());
    assert!(left.len() > 16 && right.len() > 16, "full leaves of {} and {}", left.len(), right.len());
    thin(&tree, &right);
    assert_eq!(tree.counts().merged, 0, "its right neighbour is full");
    let merged_away = leaf_of(&tree, left[0]);
    thin(&tree, &left);
    assert_eq!(tree.counts().merged, 1);
    assert_eq!(leaf_of(&tree, left[3]), leaf_of(&tree, right[3]), "one leaf holds both");
    let page = tree.pool.read(tree.file, merged_away.0).unwrap();
    assert!(Node::parse(&page).unwrap().deleted, "the leaf merged is taken out");
    drop(page);
    let thinned = |k: &u64| (left.contains(k) || right.contains(k)) && !k.is_multiple_of(4);
    let expected: Vec<Vec<u8>> = (1..=400).filter(|k| !thinned(k)).map(|k| key(k).to_vec()).collect();
    assert_eq!(keys(&tree, Direction::Forward), expected);
    assert_eq!(keys(&tree, Direction::Backward), expected.iter().rev().cloned().collect::<Vec<_>>());
    for k in 1..=400 {
        assert_eq!(tree.get(&key(k)).unwrap().is_some(), !thinned(&k), "key {k}");
    }
}

/// Verifies: [DESIGN] RULES.md D814, D806: a copy kept of a leaf merged
/// into its right neighbour since is not read. The leaf's keys left are
/// found where they went, and those deleted are not.
#[test]
fn a_copy_of_a_leaf_merged_since_is_not_read() {
    let (tree, _scratch) = tree("merge-finger");
    fill(&tree, 400);
    let left = in_leaf(&tree, 400, 200);
    let right = in_leaf(&tree, 400, left.last().unwrap().checked_add(1).unwrap());
    thin(&tree, &right);
    let mut near = Near::default();
    for &k in left.iter().take(3) {
        assert!(tree.get_near(&key(k), &mut near).unwrap());
    }
    thin(&tree, &left);
    assert_eq!(tree.counts().merged, 1);
    for &k in &left {
        assert_eq!(tree.get_near(&key(k), &mut near).unwrap(), k.is_multiple_of(4), "key {k}");
    }
}

/// Verifies: [DESIGN] RULES.md D814: a count of a range across a leaf
/// merged into the next after it was counted counts its keys once, as the
/// count goes on past the leaf's high key.
#[test]
fn a_count_across_a_leaf_merged_since_counts_its_keys_once() {
    let (tree, _scratch) = tree("merge-count");
    fill(&tree, 400);
    let left = in_leaf(&tree, 400, 200);
    let last = *left.last().unwrap();
    let right = in_leaf(&tree, 400, last.checked_add(1).unwrap());
    thin(&tree, &right);
    let mut scan = tree.scan(Bound::Unbounded, Bound::Unbounded, Direction::Forward);
    let mut counted = 0_usize;
    let through = usize::try_from(last).unwrap();
    while counted < through {
        counted = counted.checked_add(scan.count_leaf().unwrap().unwrap()).unwrap();
    }
    assert_eq!(counted, through, "counted through the leaf");
    thin(&tree, &left);
    assert_eq!(tree.counts().merged, 1, "merged into the leaf counted next");
    while let Some(found) = scan.count_leaf().unwrap() {
        counted = counted.checked_add(found).unwrap();
    }
    let thinned = right.iter().filter(|k| !k.is_multiple_of(4)).count();
    assert_eq!(counted, 400_usize.checked_sub(thinned).unwrap(), "the leaf's keys counted once, as they were");
}

/// Verifies: [DESIGN] RULES.md D814: a backward scan's next bound is the
/// low of its leaf's range, unless it took keys that low or lower, from a
/// leaf merged into it since its parent was read: then just below the
/// lowest it took, so none is taken twice.
#[test]
fn a_backward_scan_goes_below_keys_merged_in() {
    let low = key(200).to_vec();
    assert_eq!(scan::below_taken(Some(&key(201)), low.clone()), Bound::Included(low.clone()), "the leaf's own keys");
    assert_eq!(scan::below_taken(None, low.clone()), Bound::Included(low.clone()), "none taken");
    assert_eq!(scan::below_taken(Some(&key(150)), low.clone()), Bound::Excluded(key(150).to_vec()), "keys merged in");
    assert_eq!(scan::below_taken(Some(&key(200)), low), Bound::Excluded(key(200).to_vec()), "the low key itself");
}

/// Keys of the concurrent test, and those of them never deleted.
const KEYS: u64 = 3_000;
const DELETERS: u64 = 2;
fn pinned(k: u64) -> bool {
    k.is_multiple_of(5)
}

/// Checks a scan of `tree` in `direction` while others delete: its keys in
/// order, none twice, every pinned key among them.
fn scan_checked(tree: &BLinkTree, direction: Direction) {
    let mut found: Vec<u64> = keys(tree, direction).iter().map(|k| number(k)).collect();
    if direction == Direction::Backward {
        found.reverse();
    }
    assert!(found.windows(2).all(|w| w[0] < w[1]), "{direction:?}: in order, none twice");
    let kept: Vec<u64> = found.into_iter().filter(|&k| pinned(k)).collect();
    assert_eq!(kept, (1..=KEYS).filter(|&k| pinned(k)).collect::<Vec<_>>(), "{direction:?}: every key kept");
}

/// Deletes the keys of deleter `t` that are not pinned, scattered: a
/// stride prime to `KEYS` visits each key once.
fn delete_own(tree: &BLinkTree, t: u64) {
    for i in 0..KEYS {
        let k = i.checked_mul(1_009).unwrap().checked_rem(KEYS).unwrap().checked_add(1).unwrap();
        if !pinned(k) && k.checked_rem(DELETERS) == Some(t) {
            assert!(tree.delete(&key(k)).unwrap().is_some(), "key {k}");
        }
    }
}

/// Scanner threads of the concurrent test besides the reader, each
/// scanning backward and forward, and rounds of deletes they scan through.
const SCANNERS: usize = 3;
const ROUNDS: usize = 8;

/// One round of the concurrent test: the deleters' keys deleted while
/// `SCANNERS` threads scan and this one reads every pinned key, twice at
/// least and until the deletes are done.
fn round_of_deletes(tree: &BLinkTree) {
    let (done, finished, start) = (AtomicBool::new(false), AtomicU64::new(0), Barrier::new(SCANNERS + 3));
    std::thread::scope(|s| {
        for t in 0..DELETERS {
            let (start, finished) = (&start, &finished);
            s.spawn(move || {
                start.wait();
                delete_own(tree, t);
                finished.fetch_add(1, Ordering::SeqCst);
            });
        }
        for _ in 0..SCANNERS {
            let (start, done) = (&start, &done);
            s.spawn(move || {
                start.wait();
                while !done.load(Ordering::SeqCst) {
                    scan_checked(tree, Direction::Backward);
                    scan_checked(tree, Direction::Forward);
                }
            });
        }
        start.wait();
        let mut near = Near::default();
        let mut reads = 0_u32;
        while reads < 2 || finished.load(Ordering::SeqCst) < DELETERS {
            for k in (5..=KEYS).step_by(5) {
                assert!(tree.get_near(&key(k), &mut near).unwrap(), "key {k}");
                assert!(tree.get(&key(k)).unwrap().is_some(), "key {k}");
            }
            reads = reads.saturating_add(1);
        }
        done.store(true, Ordering::SeqCst);
    });
}

/// Verifies: [DESIGN] RULES.md D814, D31: while threads delete four keys
/// in five in a fixed scattered order, so leaves merge under them, scans
/// forward and backward return keys in order and none twice, every key
/// never deleted among them, and reads find each such key, from a copy of
/// its leaf or not. Each round ends with the tree holding exactly the keys
/// kept, in order both ways, and leaves merged; the keys deleted are then
/// inserted again for the next.
#[test]
fn scans_and_reads_across_merges_miss_nothing_and_repeat_nothing() {
    let (tree, _scratch) = tree("merge-concurrent");
    fill(&tree, KEYS);
    let kept: Vec<Vec<u8>> = (1..=KEYS).filter(|&k| pinned(k)).map(|k| key(k).to_vec()).collect();
    for round in 0..ROUNDS {
        let merged = tree.counts().merged;
        round_of_deletes(&tree);
        assert_eq!(keys(&tree, Direction::Forward), kept, "round {round}");
        assert_eq!(keys(&tree, Direction::Backward), kept.iter().rev().cloned().collect::<Vec<_>>(), "round {round}");
        assert!(tree.counts().merged > merged.saturating_add(10), "round {round}: {:?}", tree.counts());
        for k in (1..=KEYS).filter(|&k| !pinned(k)) {
            assert!(tree.insert(&key(k), &VALUE).unwrap(), "key {k}");
        }
    }
}

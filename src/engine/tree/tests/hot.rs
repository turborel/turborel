//! Unit tests of writers on hot nodes (RULES.md D503, `edge.rs`): keys in
//! order written into the rightmost leaf with no descent, a remembered
//! leaf that is no longer the rightmost forgotten, and a leaf writers kept
//! waiting for split before it is full.

use std::sync::atomic::AtomicU64;
use std::thread;

use super::*;

/// Verifies: [DESIGN] RULES.md D503: keys written in order after the
/// first leaf go into the rightmost leaf with no descent, splits of it
/// included; a key below that leaf descends and the leaf stays remembered;
/// and the tree holds every key, in order.
#[test]
fn keys_in_order_go_to_the_rightmost_leaf_with_no_descent() {
    let (tree, _scratch) = tree("edge-in-order");
    fill(&tree, 2000);
    let counts = tree.counts();
    assert!(counts.splits > 40, "{counts:?}");
    assert!(counts.edge_writes >= 1998, "{counts:?}");
    let rightmost = tree.descend(Target::Key(&key(u64::MAX)), 0, &mut Path::default(), |_| Ok(())).unwrap().0;
    assert_eq!(tree.edge.load(Ordering::Relaxed), rightmost.0);
    assert!(tree.insert(&key(0), &VALUE).unwrap());
    assert_eq!(tree.counts().edge_writes, counts.edge_writes, "a key below it descends");
    assert_eq!(tree.edge.load(Ordering::Relaxed), rightmost.0, "and leaves it remembered");
    assert!(tree.insert(&key(2001), &VALUE).unwrap());
    assert_eq!(tree.counts().edge_writes, counts.edge_writes + 1);
    let want: Vec<Vec<u8>> = (0..=2001_u64).map(|k| key(k).to_vec()).collect();
    assert_eq!(keys(&tree, Direction::Forward), want);
}

/// Verifies: [DESIGN] RULES.md D503: a remembered leaf that is not the
/// rightmost leaf of the tree, an inner node, a page no frame holds, or
/// a leaf with a right neighbour, is forgotten, and the write descends.
#[test]
fn a_leaf_no_longer_the_rightmost_is_forgotten() {
    let (tree, _scratch) = tree("edge-stale");
    fill(&tree, 400);
    let first = tree.descend(Target::Key(&key(1)), 0, &mut Path::default(), |_| Ok(())).unwrap().0;
    let root = tree.root.load(Ordering::Acquire);
    for (k, stale) in [(401_u64, first.0), (402, root), (403, 1_u64 << 40_u32)] {
        tree.edge.store(stale, Ordering::Relaxed);
        let before = tree.counts().edge_writes;
        assert!(tree.insert(&key(k), &VALUE).unwrap());
        assert_eq!(tree.counts().edge_writes, before, "key {k}");
        assert_ne!(tree.edge.load(Ordering::Relaxed), stale, "key {k}");
        assert_eq!(tree.get(&key(k)).unwrap(), Some(VALUE.to_vec()));
    }
    let want: Vec<Vec<u8>> = (1..=403_u64).map(|k| key(k).to_vec()).collect();
    assert_eq!(keys(&tree, Direction::Forward), want);
}

/// Verifies: [DESIGN] RULES.md D503: a leaf mostly full that writers
/// waited for 32 times is split by the next entry written into its
/// middle, though the entry fits; the count of waits starts again; and
/// each key stays found.
#[test]
fn a_leaf_writers_wait_for_is_split_early() {
    let (tree, _scratch) = tree("edge-contended");
    fill(&tree, 400);
    let k = 200_u64;
    let leaf = tree.descend(Target::Key(&key(k)), 0, &mut Path::default(), |_| Ok(())).unwrap().0;
    let in_leaf = |n: u64| tree.descend(Target::Key(&key(n)), 0, &mut Path::default(), |_| Ok(())).unwrap().0 == leaf;
    assert!(in_leaf(k + 1) && in_leaf(k + 2), "the leaf holds a few keys after {k}");
    // Room for one entry: one removed, the leaf left mostly full.
    assert!(tree.delete(&key(k + 1)).unwrap().is_some());
    for round in 1..=edge::CONTENDED {
        let held = tree.lock(leaf).unwrap();
        thread::scope(|s| {
            s.spawn(|| drop(tree.lock(leaf).unwrap()));
            while held.contended() < round {
                thread::yield_now();
            }
            drop(held);
        });
    }
    let splits = tree.counts().splits;
    assert!(tree.insert(&key(k + 1), &VALUE).unwrap());
    let counts = tree.counts();
    assert_eq!((counts.splits, counts.contention_splits), (splits + 1, 1), "{counts:?}");
    assert_eq!(tree.lock(leaf).unwrap().contended(), 0);
    let want: Vec<Vec<u8>> = (1..=400_u64).map(|n| key(n).to_vec()).collect();
    assert_eq!(keys(&tree, Direction::Forward), want);
}

/// Verifies: [DESIGN] RULES.md D503, D31: threads writing keys in order
/// at once, each taking the next key, and threads reading them back as
/// they go, meet at the rightmost leaf; each key written is found, the
/// tree ends holding every key in order, and most went in with no
/// descent.
#[test]
fn threads_writing_keys_in_order_keep_every_key() {
    let (tree, _scratch) = tree("edge-threads");
    let next = AtomicU64::new(1);
    let (threads, each) = (8_u64, 1500_u64);
    thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                for _ in 0..each {
                    let k = next.fetch_add(1, Ordering::Relaxed);
                    assert!(tree.insert(&key(k), &VALUE).unwrap());
                    assert_eq!(tree.get(&key(k)).unwrap(), Some(VALUE.to_vec()), "key {k}");
                }
            });
        }
    });
    let want: Vec<Vec<u8>> = (1..=threads * each).map(|k| key(k).to_vec()).collect();
    assert_eq!(keys(&tree, Direction::Forward), want);
    let counts = tree.counts();
    assert!(counts.edge_writes.saturating_mul(2) > threads * each, "{counts:?}");
}

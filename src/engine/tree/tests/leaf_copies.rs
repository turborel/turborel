//! Unit tests of the copies of read-mostly leaves a thread keeps (RULES.md
//! D858, `leaves.rs`): when a leaf is copied, that a copy is never read
//! once its leaf has changed, and that readers beside a writer see whole
//! values, never older than the last write finished before they read.

use std::sync::atomic::{AtomicU8, Ordering as Order};

use super::*;

/// The leaf of `tree` whose range holds key `k`, read with no copy made.
fn leaf_of(tree: &BLinkTree, k: u64) -> PageId {
    let mut path = Path { above_leaf: true, ..Path::default() };
    let found = tree.descend(Target::Key(&key(k)), 0, &mut path, |node| node.child(&key(k)).map(|(child, _)| child));
    found.unwrap().1
}

/// Whether the thread keeps a copy of `leaf` of `tree`.
fn copied(tree: &BLinkTree, leaf: PageId) -> bool {
    leaves::copied().contains(&(tree.file, leaf.0))
}

/// Verifies: [DESIGN] RULES.md D858: a leaf is copied at the third read
/// that finds it unchanged, and read from the copy after; a write of it
/// ends the copy, the next read giving the value written, and the leaf
/// then needs twice the reads in a row to be copied again; a leaf written
/// between every two reads is never copied.
#[test]
fn a_leaf_is_copied_once_read_unchanged_and_never_read_stale() {
    let (tree, _scratch) = tree("leaves-rule");
    fill(&tree, 300);
    let leaf = leaf_of(&tree, 150);
    let read = |n: usize| (0..n).for_each(|_| assert!(tree.get(&key(150)).unwrap().is_some()));
    read(2);
    assert!(!copied(&tree, leaf), "two reads");
    read(1);
    assert!(copied(&tree, leaf), "the third");
    let hits = tree.pool.counts().hits;
    read(5);
    assert_eq!(tree.pool.counts().hits, hits, "read from the copies, no page held");
    let mut newer = VALUE;
    newer[0] = 8;
    assert!(tree.replace(&key(150), &newer).unwrap().is_some());
    assert_eq!(tree.get(&key(150)).unwrap(), Some(newer.to_vec()), "the value written, not the copy's");
    assert!(!copied(&tree, leaf), "ended by the write");
    read(3);
    assert!(!copied(&tree, leaf), "a strike against it: four in a row now");
    read(1);
    assert!(copied(&tree, leaf));
    assert_eq!(tree.get(&key(150)).unwrap(), Some(newer.to_vec()));
    let busy = leaf_of(&tree, 20);
    assert_ne!(busy, leaf);
    for round in 0..40_u8 {
        newer[1] = round;
        assert!(tree.replace(&key(20), &newer).unwrap().is_some());
        assert_eq!(tree.get(&key(20)).unwrap(), Some(newer.to_vec()));
        assert!(!copied(&tree, busy), "written as often as read");
    }
}

/// Verifies: [DESIGN] RULES.md D858, D502: readers of a leaf beside a
/// writer of it, each reading through its own copies, see every value
/// whole, all its bytes of one write, and none older than the last write
/// that had finished before the read began.
#[test]
fn readers_beside_a_writer_see_whole_values_none_stale() {
    let (tree, _scratch) = tree("leaves-threads");
    fill(&tree, 60);
    let done = AtomicU8::new(7);
    std::thread::scope(|threads| {
        for _ in 0..3_u8 {
            threads.spawn(|| {
                let mut last = 7_u8;
                while last < 200 {
                    let floor = done.load(Order::SeqCst);
                    let value = tree.get(&key(30)).unwrap().unwrap();
                    let first = value[0];
                    assert!(value.iter().all(|&byte| byte == first), "torn");
                    assert!(first >= floor && first >= last, "stale: {first} after {floor}, {last}");
                    last = first;
                }
            });
        }
        for n in 8..=200_u8 {
            assert!(tree.replace(&key(30), &[n; 92]).unwrap().is_some());
            done.store(n, Order::SeqCst);
            // Read between writes, so the readers' copies are made and ended.
            (0..40_u8).for_each(|_| drop(tree.get(&key(31)).unwrap()));
        }
    });
}

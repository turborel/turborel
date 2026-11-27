//! Unit tests of reads by key that find their leaf through the thread's
//! copies of the inner nodes (RULES.md D955, `inner.rs`, `near.rs`,
//! `probe.rs`): they find what a descent finds, and descend where a copy
//! is missing or no longer its node's content.

use std::sync::Barrier;

use super::*;

/// The leaf the thread's copies lead to for key `k`, asked in the tree's
/// epoch, as a read asks (RULES.md D1270).
fn by_copies(tree: &BLinkTree, k: u64) -> Option<u64> {
    let op = tree.epochs.guard();
    tree.leaf_by_copies(op.within(), &key(k))
}

/// The first key a probe from `k` shows, if any.
fn first(tree: &BLinkTree, k: u64) -> Option<Vec<u8>> {
    let mut seen = None;
    tree.first_from(&key(k), |found, _| {
        seen = Some(found.to_vec());
        Ok(Visit::Stop)
    })
    .unwrap();
    seen
}

/// A tree of two levels, its root an inner node, its keys the even
/// numbers to 40,000.
fn two_levels(test: &str) -> (BLinkTree, Scratch) {
    let (tree, scratch) = tree(test);
    for k in (2..=40_000).step_by(2) {
        assert!(tree.insert(&key(k), &VALUE).unwrap());
    }
    (tree, scratch)
}

/// Verifies: [DESIGN] RULES.md D955: with the thread's copies of the
/// inner nodes current, the leaf found through them for each key, held
/// or not, below all and above all, is the leaf a descent reaches; a
/// read by key through them finds each key held and no other, and a
/// probe shows the key a scan from it gives first. MC/DC of the walk:
/// (a copy at each level, current, holding the key) a leaf found,
/// against the same reads by a thread that keeps no copies, which reach
/// the root and descend, finding the same.
#[test]
fn a_read_through_the_copies_finds_what_a_descent_finds() {
    let (tree, _scratch) = two_levels("by-copies");
    for k in 0..=40_002 {
        let leaf = leaf_of(&tree, k).0;
        assert_eq!(by_copies(&tree, k), Some(leaf), "key {k}");
    }
    let reads = || {
        for k in 0..=40_002_u64 {
            let mut near = Near::default();
            assert_eq!(tree.get_near(&key(k), &mut near).unwrap(), k > 0 && k <= 40_000 && k.is_multiple_of(2), "key {k}");
            let next = k.max(1).checked_add(k.max(1) % 2).filter(|&n| n <= 40_000);
            assert_eq!(first(&tree, k), next.map(|n| key(n).to_vec()), "a probe from {k}");
        }
    };
    reads();
    std::thread::scope(|s| {
        s.spawn(|| {
            let root = tree.root.load(Ordering::Acquire);
            assert_eq!(by_copies(&tree, 10), Some(root), "no copies: the root, to be read for itself");
            reads();
        });
    });
}

/// Verifies: [DESIGN] RULES.md D955, Lehman and Yao §5: a thread's copy
/// of an inner node changed since by another, as a split posts a new
/// child to it, is not followed. The walk gives no leaf below it, and the
/// read descends, finding every key. Reads made as leaves split find every
/// key, and the walk never gives a leaf a descent would not reach. MC/DC
/// of the copy's check: current, the leaf given, against changed, the
/// root given to be read for itself.
#[test]
fn a_copy_changed_since_is_not_followed() {
    let (tree, _scratch) = two_levels("by-copies-stale");
    let (warm, split) = (Barrier::new(2), Barrier::new(2));
    let mut k = 1_u64;
    std::thread::scope(|s| {
        s.spawn(|| {
            let leaf = leaf_of(&tree, 1).0;
            assert_eq!(by_copies(&tree, 1), Some(leaf), "its copies kept by the descent");
            warm.wait();
            split.wait();
            let root = tree.root.load(Ordering::Acquire);
            assert_eq!(by_copies(&tree, 1), Some(root), "the root changed by the split: no leaf below it");
            let mut near = Near::default();
            assert!(tree.get_near(&key(1), &mut near).unwrap(), "found by a descent");
        });
        warm.wait();
        let splits = tree.counts().splits;
        while tree.counts().splits == splits {
            assert!(tree.insert(&key(k), &VALUE).unwrap());
            k = k.checked_add(2).unwrap();
        }
        split.wait();
    });
    for odd in (1..k).step_by(2) {
        let mut near = Near::default();
        assert!(tree.get_near(&key(odd), &mut near).unwrap(), "key {odd}");
        if let Some(leaf) = by_copies(&tree, odd) {
            assert_eq!(leaf, leaf_of(&tree, odd).0, "key {odd}");
        }
    }
    for even in (2..=4_000).step_by(2) {
        assert_eq!(first(&tree, even), Some(key(even).to_vec()), "key {even}");
    }
}

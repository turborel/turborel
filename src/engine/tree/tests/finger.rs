//! Unit tests of reads by key from a copy of the leaf the reads before
//! found (RULES.md D806, `near.rs`): a run of keys of one leaf read from
//! the copy with no page read, a change to the leaf ending the copy, and
//! a split moving keys right out of it.

use super::*;

/// The leaf of `tree` whose range holds key `k`.
fn leaf_of(tree: &BLinkTree, k: u64) -> PageId {
    tree.descend(Target::Key(&key(k)), 0, &mut Path::default(), |_| Ok(())).unwrap().0
}

/// The keys of `1..=n` in the leaf that holds key 1.
fn first_leaf(tree: &BLinkTree, n: u64) -> Vec<u64> {
    let leaf = leaf_of(tree, 1);
    (1..=n).filter(|&k| leaf_of(tree, k) == leaf).collect()
}

/// Verifies: [DESIGN] RULES.md D806, D502: keys of one leaf read one
/// after another read the leaf from the pool twice, then from the copy
/// kept, with no page read; a value replaced in the leaf moves its frame's
/// version, so the next read takes the leaf again and finds the new value.
#[test]
fn a_run_of_keys_of_one_leaf_is_read_from_its_copy() {
    let (tree, _scratch) = tree("finger-run");
    fill(&tree, 300);
    let run = first_leaf(&tree, 300);
    assert!(run.len() > 8, "a leaf of {} keys", run.len());
    let mut near = Near::default();
    let hits = tree.pool.counts().hits;
    for &k in run.iter().take(3) {
        assert!(tree.get_near(&key(k), &mut near).unwrap());
    }
    assert!(tree.pool.counts().hits > hits, "the leaf read from the pool");
    let copied = tree.pool.counts().hits;
    for &k in run.iter().skip(3) {
        assert!(tree.get_near(&key(k), &mut near).unwrap());
        assert_eq!(near.bytes, VALUE, "key {k}");
    }
    assert_eq!(tree.pool.counts().hits, copied, "the rest from the copy");
    let last = *run.last().unwrap();
    let mut newer = VALUE;
    newer[0] = 9;
    assert!(tree.replace(&key(last), &newer).unwrap().is_some());
    assert!(tree.get_near(&key(last), &mut near).unwrap());
    assert_eq!(near.bytes, newer, "the change seen, the copy dropped");
}

/// Verifies: [DESIGN] RULES.md D806, Lehman and Yao §5: a copy kept of a
/// leaf that splits since is not read: keys that moved right are found
/// in the right half, and keys inserted are found.
#[test]
fn a_copy_of_a_leaf_split_since_is_not_read() {
    let (tree, _scratch) = tree("finger-split");
    for k in (2..=200).step_by(2) {
        assert!(tree.insert(&key(k), &VALUE).unwrap());
    }
    let run: Vec<u64> = first_leaf(&tree, 200).into_iter().filter(|k| k.is_multiple_of(2)).collect();
    let mut near = Near::default();
    for &k in run.iter().take(3) {
        assert!(tree.get_near(&key(k), &mut near).unwrap());
    }
    let splits = tree.counts().splits;
    let end = *run.last().unwrap();
    for k in (1..end).step_by(2) {
        assert!(tree.insert(&key(k), &VALUE).unwrap());
    }
    assert!(tree.counts().splits > splits, "the leaf split");
    for k in 1..=end {
        assert!(tree.get_near(&key(k), &mut near).unwrap(), "key {k}");
    }
    assert!(!tree.get_near(&key(end.checked_add(1).unwrap()), &mut near).unwrap(), "odd keys past it are not in");
}

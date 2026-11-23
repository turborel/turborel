//! Unit tests of reads from the leaf noted for a row (RULES.md D500,
//! `near.rs`, `hints.rs`): a hint at the row's leaf, at a leaf split since,
//! taken out, freed, or given to another tree, and at another leaf of the
//! tree; each finds what a descent finds, and only a leaf of the tree that
//! holds the key's range answers.

use super::*;

/// The leaf of `tree` whose range holds key `k`.
fn leaf_of(tree: &BLinkTree, k: u64) -> PageId {
    tree.descend(Target::Key(&key(k)), 0, &mut Path::default(), |_| Ok(())).unwrap().0
}

/// A table's tree, keyed by row id, of keys `1..=n`.
fn rows(test: &str, n: u64) -> (BLinkTree, Scratch) {
    let (tree, scratch) = tree(test);
    tree.note_rows();
    fill(&tree, n);
    (tree, scratch)
}

/// Whether key `k` is found by a read with no leaf to start from, given
/// the note that it is in `leaf`; that what it found is what a descent
/// finds; and whether the noted leaf answered.
fn read_noted(tree: &BLinkTree, k: u64, leaf: PageId) -> (bool, bool) {
    hints::note(k, leaf.0);
    let before = tree.counts().hinted;
    let mut near = Near::default();
    let found = tree.get_near(&key(k), &mut near).unwrap();
    let want = tree.get(&key(k)).unwrap();
    assert_eq!(found.then_some(near.bytes), want, "key {k}");
    (found, tree.counts().hinted > before)
}

/// Verifies: [DESIGN] RULES.md D500: a table's tree notes the leaf each
/// row is written into, the right half of a split if it went there, and
/// a read with no leaf to start from takes it
/// from the note, with no descent; a tree that is not a table's ignores
/// notes, and a row noted with no leaf descends.
#[test]
fn a_row_read_starts_at_the_leaf_noted_for_it() {
    let (tree, _scratch) = rows("hinted-rows", 0);
    for k in 1..=300 {
        assert!(tree.insert(&key(k), &VALUE).unwrap());
        assert_eq!(hints::leaf_of(k), Some(leaf_of(&tree, k).0), "the write of {k} noted its leaf, split or not");
    }
    assert!(tree.counts().splits > 5);
    for k in 1..=300 {
        assert_eq!(read_noted(&tree, k, leaf_of(&tree, k)), (true, true), "key {k}");
    }
    assert_eq!(read_noted(&tree, 301, leaf_of(&tree, 301)), (false, true), "absent, in the leaf's range");
    hints::note(7, hints::NO_LEAF);
    assert!(tree.get_near(&key(7), &mut Near::default()).unwrap());
    let (plain, _other) = super::tree("hinted-plain");
    fill(&plain, 300);
    assert_eq!(read_noted(&plain, 5, leaf_of(&plain, 5)), (true, false), "not a table's tree");
}

/// Verifies: [DESIGN] RULES.md D500, Lehman and Yao §4: a leaf noted
/// before it split leads right to the key's leaf within four links; past
/// them, or noted at a leaf right of the key's, whose first key is above
/// it, the read descends.
#[test]
fn a_noted_leaf_split_since_leads_right() {
    let (tree, _scratch) = rows("hinted-split", 40);
    let old = leaf_of(&tree, 40);
    fill_from(&tree, 41, 400);
    let moves = tree.counts().right_moves;
    let near_keys: Vec<u64> = (41..=400).filter(|&k| leaf_hops(&tree, old, k) <= 4).collect();
    let far_keys: Vec<u64> = (41..=400).filter(|&k| leaf_hops(&tree, old, k) > 4).collect();
    assert!(!near_keys.is_empty() && !far_keys.is_empty());
    for &k in &near_keys {
        assert_eq!(read_noted(&tree, k, old), (true, true), "key {k}");
    }
    assert!(tree.counts().right_moves > moves, "moved right");
    for &k in &far_keys {
        assert_eq!(read_noted(&tree, k, old), (true, false), "key {k}, too far right");
    }
    assert_eq!(read_noted(&tree, 1, leaf_of(&tree, 400)), (true, false), "a leaf right of the key's");
}

/// Inserts keys `from..=to` into `tree`.
fn fill_from(tree: &BLinkTree, from: u64, to: u64) {
    for i in from..=to {
        assert!(tree.insert(&key(i), &VALUE).unwrap());
    }
}

/// The right links from `leaf` to the leaf of key `k`.
fn leaf_hops(tree: &BLinkTree, leaf: PageId, k: u64) -> usize {
    let target = leaf_of(tree, k);
    let next = |at: &PageId| Node::parse(&tree.pool.read(tree.file, at.0).unwrap()).unwrap().right;
    std::iter::successors(Some(leaf), next).take_while(|&at| at != target).count()
}

/// Verifies: [DESIGN] RULES.md D500, `reclaim.rs`: a noted leaf taken out
/// of the tree is not trusted; once its page is freed, it is not read; once
/// the page is given to another tree, holding the same keys, it names that
/// tree and is not trusted. Each read finds what a descent finds.
#[test]
fn a_noted_leaf_gone_is_not_trusted() {
    let (tree, _scratch) = rows("hinted-gone", 300);
    let gone = leaf_of(&tree, 150);
    let range: Vec<u64> = (1..=300).filter(|&k| leaf_of(&tree, k) == gone).collect();
    for &k in &range {
        assert!(tree.delete(&key(k)).unwrap().is_some());
    }
    assert!(tree.counts().unlinked > 0);
    assert_eq!(read_noted(&tree, 150, gone), (false, false), "taken out");
    let other = BLinkTree::create(Arc::clone(&tree.pool), tree.file).unwrap();
    other.note_rows();
    tree.free_retired().unwrap();
    assert!(tree.pool.read_resident(tree.file, gone.0).unwrap().is_none(), "freed, its frame emptied");
    assert_eq!(read_noted(&tree, 150, gone), (false, false), "freed");
    fill(&other, 300);
    let page = tree.pool.read(tree.file, gone.0).unwrap();
    let reused = Node::parse(&page).unwrap();
    assert!(reused.level == 0 && reused.tree == other.meta.0 && reused.len() > 0, "a leaf of the other tree");
    let first = u64::from_be_bytes(reused.key(0).unwrap().to_vec().try_into().unwrap());
    fill_from(&tree, 150, 150);
    for k in [1, 150, 300, first] {
        assert!(!read_noted(&tree, k, gone).1, "key {k}: the page is another tree's or none");
    }
}

/// Verifies: [DESIGN] RULES.md D502: a descent keeps copies of the inner
/// nodes it reads and reads them again from the copies, finding what it
/// found through their latches; a split of a leaf changes its parent, so
/// the copy taken before is not current, and a read gives the node as its
/// latch does, the keys moved found.
#[test]
fn descents_read_inner_nodes_from_current_copies() {
    let (tree, _scratch) = rows("inner-copies", 2_000);
    assert!(tree.height().unwrap() > 1);
    let root = tree.root.load(Ordering::Acquire);
    let read_root = || inner::read(&tree.pool, (tree.file, root), None, true).unwrap();
    {
        let got = read_root();
        let parsed = got.node().unwrap().parsed();
        inner::keep(&got, (tree.file, root), parsed);
    }
    assert!(matches!(read_root(), inner::Read::Copied(..)), "kept, and current");
    let before = tree.pool.read_stamped(tree.file, root, None).unwrap().1;
    for k in (1..=2_000).step_by(97) {
        assert_eq!(tree.get(&key(k)).unwrap(), Some(VALUE.to_vec()), "key {k}");
    }
    assert!(tree.pool.unchanged(tree.file, root, before), "reads change nothing");
    fill_from(&tree, 2_001, 4_000);
    assert!(!tree.pool.unchanged(tree.file, root, before), "the root changed as leaves split");
    assert_eq!(&read_root()[..], &tree.pool.read(tree.file, root).unwrap()[..]);
    for k in (1..=4_000).step_by(89) {
        assert_eq!(tree.get(&key(k)).unwrap(), Some(VALUE.to_vec()), "key {k}");
    }
}

/// Verifies: [DESIGN] RULES.md D500, D446: a replace from the leaf a
/// `get_near` left starts there, with no descent: from the leaf of the key
/// read before, the write moves right to the key's own leaf; it compares
/// the value under the leaf's lock; and with no leaf left, or a leaf
/// right of the key's, it descends.
#[test]
fn a_replace_starts_at_the_leaf_a_read_left() {
    let (tree, _scratch) = rows("near-replace", 300);
    let (first, last) = (key(1), key(300));
    assert_ne!(leaf_of(&tree, 1), leaf_of(&tree, 300));
    let mut near = Near::default();
    assert!(tree.get_near(&first, &mut near).unwrap());
    let moves = tree.counts().right_moves;
    let mut newer = VALUE;
    newer[0] = 8;
    assert!(tree.replace_if_near(&first, (&VALUE, &newer), &near).unwrap());
    assert_eq!(tree.counts().right_moves, moves, "its own leaf, no move");
    assert!(!tree.replace_if_near(&first, (&VALUE, &newer), &near).unwrap(), "the value compared");
    // Left at the leaf of key 1, the write of the last key moves right.
    assert!(tree.replace_if_near(&last, (&VALUE, &newer), &near).unwrap());
    assert!(tree.counts().right_moves > moves, "moved right");
    assert_eq!(tree.get(&last).unwrap(), Some(newer.to_vec()));
    // Left right of the key's leaf, or by no read, it descends.
    let mut right = Near::default();
    assert!(tree.get_near(&last, &mut right).unwrap());
    assert!(tree.replace_if_near(&key(2), (&VALUE, &newer), &right).unwrap());
    assert!(tree.replace_if_near(&key(3), (&VALUE, &newer), &Near::default()).unwrap());
    for k in [2, 3] {
        assert_eq!(tree.get(&key(k)).unwrap(), Some(newer.to_vec()), "key {k}");
    }
}

/// Verifies: [DESIGN] RULES.md D853: a read of a table's row for a write
/// takes it from the leaf noted for the row, with no descent and no path,
/// and the write over it starts at that leaf; a row absent from the range
/// of the leaf noted is none; a leaf noted that split since leads right to
/// the row, and the write moves right too; a row noted with no leaf is
/// read by a descent, which keeps its path.
#[test]
fn a_read_for_a_write_starts_at_the_leaf_noted() {
    let (tree, _scratch) = rows("noted-write", 40);
    let old = leaf_of(&tree, 5);
    hints::note(5, old.0);
    let hinted = tree.counts().hinted;
    let (value, found) = tree.get_for_write(&key(5)).unwrap().unwrap();
    assert_eq!((value.as_slice(), found.leaf), (&VALUE[..], old));
    assert_eq!(tree.counts().hinted, hinted + 1, "from the leaf noted");
    assert!(found.path.stack.last().is_none(), "no descent, so no path");
    let mut newer = VALUE;
    newer[0] = 8;
    assert!(tree.replace_found(&found, &key(5), (&VALUE, &newer)).unwrap());
    assert_eq!(tree.get(&key(5)).unwrap(), Some(newer.to_vec()));
    hints::note(41, leaf_of(&tree, 41).0);
    assert!(tree.get_for_write(&key(41)).unwrap().is_none(), "absent, in the range of the leaf noted");
    fill_from(&tree, 41, 400);
    let k = (41..=400).find(|&k| (1..=4).contains(&leaf_hops(&tree, old, k))).unwrap();
    hints::note(k, old.0);
    let (_, right) = tree.get_for_write(&key(k)).unwrap().unwrap();
    assert_eq!(right.leaf, leaf_of(&tree, k), "moved right from the leaf noted");
    assert!(tree.replace_found(&right, &key(k), (&VALUE, &newer)).unwrap());
    assert_eq!(tree.get(&key(k)).unwrap(), Some(newer.to_vec()));
    hints::note(7, hints::NO_LEAF);
    let hinted = tree.counts().hinted;
    let (_, descended) = tree.get_for_write(&key(7)).unwrap().unwrap();
    assert_eq!(tree.counts().hinted, hinted, "no leaf noted");
    assert!(descended.path.stack.last().is_some(), "a descent, its path kept");
}

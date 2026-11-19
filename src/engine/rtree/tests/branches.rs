//! Branch reclamation: branches left sparse taken out or merged a level
//! up, ancestors' entries narrowed, and the root giving way to its only
//! child, while scans neither miss nor repeat an entry.

use super::*;

/// The levels of `tree`, its root's included.
fn height(tree: &RTree) -> usize {
    tree.shape().unwrap().len()
}

/// The nodes a search of `window` reads.
fn read_by(tree: &RTree, window: Bounds) -> usize {
    let mut walk = scan::Walk::new(tree.clone(), Some(window));
    std::iter::from_fn(|| walk.node().unwrap()).count()
}

/// Verifies: [DESIGN] D139: removing every row empties leaves, then the
/// branches above them, until the root is left with one child and gives
/// way to it, level by level, down to a single leaf; the tree then grows
/// again as rows return, and holds each once.
#[test]
fn the_tree_grows_shorter_as_it_empties() {
    let tree = tree();
    fill(&tree, 0..20_000);
    let tall = height(&tree);
    assert!(tall >= 3, "{tall}");
    for i in 0..19_990_u32 {
        tree.remove(point(i), u64::from(i)).unwrap();
    }
    assert!(height(&tree) < tall);
    for i in 19_990..20_000_u32 {
        tree.remove(point(i), u64::from(i)).unwrap();
    }
    assert_eq!(height(&tree), 1);
    assert!(tree.reclaimed().0 > 0);
    assert_eq!(ids(&tree, None), Vec::<u64>::new());
    fill(&tree, 0..20_000);
    assert_eq!(height(&tree), tall);
    assert_eq!(ids(&tree, None), (0..20_000).collect::<Vec<_>>());
}

/// Verifies: [DESIGN] D139: after deletes empty the left of the space,
/// the entries above each changed parent are narrowed to what lies below
/// them, so every entry for a branch is exactly its envelope and a search
/// of the emptied region reads a tenth of the nodes it read before; every
/// entry for a leaf still covers it.
#[test]
fn ancestors_are_narrowed_after_reclamation() {
    let tree = tree();
    fill(&tree, 0..20_000);
    let window = Bounds::new([0.0_f64, 0.0_f64], [500.0_f64, 991.0_f64]).unwrap();
    let before = read_by(&tree, window);
    for i in (0..20_000_u32).filter(|&i| point(i).lower()[0] < 700.0_f64) {
        tree.remove(point(i), u64::from(i)).unwrap();
    }
    assert!(tree.reclaimed().1 > 0);
    let after = read_by(&tree, window);
    assert!(after.saturating_mul(10) < before, "{after} nodes read after, {before} before");
    let mut walk = scan::Walk::new(tree.clone(), None);
    while let Some((_, node)) = walk.node().unwrap() {
        for entry in node.entries.iter().filter(|_| node.level > 0) {
            let child = tree.read(PageId(entry.id)).unwrap().bounds().unwrap();
            assert!(entry.bounds.contains(child), "{entry:?} misses {child:?}");
            assert!(node.level == 1 || entry.bounds == child, "{entry:?} is wider than {child:?}");
        }
    }
}

/// Verifies: [DESIGN] D139: a sparse node whose parent was retired by a
/// merge a level up is left alone, since the parent's live copy still
/// names it; changing the retired parent would strand it there, retired.
#[test]
fn a_retired_parent_is_never_changed() {
    let tree = tree();
    fill(&tree, 0..2_000);
    let root = tree.root().unwrap();
    let mut parent = tree.read(root.id).unwrap();
    assert_eq!(parent.level, 1);
    let leaf = PageId(parent.entries.first().unwrap().id);
    let mut sparse = tree.read(leaf).unwrap();
    sparse.entries.clear();
    tree.store(leaf, &sparse).unwrap();
    parent.retired = true;
    tree.store(root.id, &parent).unwrap();
    assert!(!tree.take_out_sparse(leaf, root.id).unwrap());
    parent.retired = false;
    tree.store(root.id, &parent).unwrap();
    assert!(tree.take_out_sparse(leaf, root.id).unwrap());
}

/// Removes every unstable row of `writer`, reinserts them, and removes
/// them again, so whole regions empty and branches merge.
fn sweep(tree: &RTree, writer: u32) {
    let mine = (0..20_000_u32).filter(|i| i % 7 != 0 && i % 3 == writer);
    mine.clone().for_each(|i| tree.remove(point(i), u64::from(i)).unwrap());
    mine.clone().for_each(|i| tree.insert(point(i), u64::from(i)).unwrap());
    mine.for_each(|i| tree.remove(point(i), u64::from(i)).unwrap());
}

/// Verifies: [DESIGN] D139: scans on other threads, while writers empty
/// the tree around a sparse set of stable rows so that branches merge,
/// ancestors narrow and the root gives way, return every stable row
/// exactly once each time, however the threads are scheduled (R-TEST-5),
/// as the check holds for every interleaving.
#[test]
fn concurrent_scans_neither_miss_nor_repeat_across_branch_reclamation() {
    let tree = tree();
    fill(&tree, 0..20_000);
    let (tall, branches) = (height(&tree), tree.shape().unwrap()[1].nodes);
    let stable: BTreeSet<u64> = (0..20_000).filter(|id| id % 7 == 0).collect();
    std::thread::scope(|scope| {
        for writer in 0_u32..3 {
            let tree = tree.clone();
            std::thread::Builder::new().name(format!("rtree-sweeper-{writer}")).spawn_scoped(scope, move || sweep(&tree, writer)).unwrap();
        }
        for reader in 0_u32..2 {
            let (tree, stable) = (tree.clone(), &stable);
            std::thread::Builder::new()
                .name(format!("rtree-branch-scanner-{reader}"))
                .spawn_scoped(scope, move || {
                    for _ in 0..30_u32 {
                        let seen: Vec<u64> = tree.scan(None).map(|e| e.unwrap().1).filter(|id| stable.contains(id)).collect();
                        let unique: BTreeSet<u64> = seen.iter().copied().collect();
                        assert_eq!(seen.len(), stable.len());
                        assert_eq!(&unique, stable);
                    }
                })
                .unwrap();
        }
    });
    let shape = tree.shape().unwrap();
    assert!(shape.len() < tall || shape[1].nodes < branches, "{branches} {shape:?}");
    assert_eq!(ids(&tree, None), stable.into_iter().collect::<Vec<_>>());
}

/// Verifies: [DESIGN] D139: a writer that holds a child checks that the
/// parent's entry for it still covers what it adds, since a narrowing may
/// have come between enlarging the entry and locking the child; outside
/// the entry it must descend again.
#[test]
fn a_writer_sees_a_narrowed_entry() {
    let tree = tree();
    fill(&tree, 0..2_000);
    let root = tree.root().unwrap();
    let entry = *tree.read(root.id).unwrap().entries.first().unwrap();
    let inside = Bounds::point(entry.bounds.lower()).unwrap();
    let outside = Bounds::point(entry.bounds.upper().map(|v| v + 1.0_f64)).unwrap();
    assert!(tree.covers(Some(&root), PageId(entry.id), inside).unwrap());
    assert!(!tree.covers(Some(&root), PageId(entry.id), outside).unwrap());
    assert!(tree.covers(None, root.id, outside).unwrap());
}

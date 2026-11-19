//! Reclamation: emptied and under-full leaves taken out or merged, their
//! pages freed and reused, while scans neither miss nor repeat an entry.

use super::*;

/// Removes every row of `range` whose point lies left of `x`, so whole
/// leaves empty and others fall under minimum fill.
fn remove_left(tree: &RTree, range: std::ops::Range<u32>, x: f64) -> BTreeSet<u64> {
    let mut removed = BTreeSet::new();
    for i in range {
        if point(i).lower()[0] < x {
            tree.remove(point(i), u64::from(i)).unwrap();
            removed.insert(u64::from(i));
        }
    }
    removed
}

/// Verifies: [DESIGN] D139: deletes that empty leaves or leave them under
/// two fifths full take them out or merge them; the leaf level then holds
/// fewer nodes, every remaining entry is found once, and windows match an
/// exhaustive search.
#[test]
fn emptied_and_sparse_leaves_are_taken_out() {
    let tree = tree();
    fill(&tree, 0..20_000);
    let before = tree.shape().unwrap()[0].nodes;
    let removed = remove_left(&tree, 0..20_000, 700.0_f64);
    for i in (0..20_000_u32).step_by(3) {
        if point(i).lower()[0] >= 700.0_f64 {
            tree.remove(point(i), u64::from(i)).unwrap();
        }
    }
    assert!(tree.reclaimed().0 > 0);
    let after = tree.shape().unwrap();
    assert!(after[0].nodes * 2 < before, "{before} {after:?}");
    let kept: Vec<u64> = (0..20_000_u32).filter(|&i| !removed.contains(&u64::from(i)) && i % 3 != 0).map(u64::from).collect();
    assert_eq!(ids(&tree, None), kept);
    let window = Bounds::new([650.0_f64, 100.0_f64], [900.0_f64, 400.0_f64]).unwrap();
    let expected: Vec<u64> = kept.iter().copied().filter(|&id| window.intersects(point(u32::try_from(id).unwrap()))).collect();
    assert_eq!(ids(&tree, Some(window)), expected);
}

/// Verifies: [DESIGN] D139: pages of reclaimed leaves are freed once no
/// operation holds them, and inserts afterwards reuse them, so refilling the
/// tree allocates no more pages than it first took.
#[test]
fn reclaimed_pages_are_reused() {
    let tree = tree();
    fill(&tree, 0..20_000);
    let full = tree.inner.pool.pages_in_use(tree.inner.file).unwrap();
    for round in 0..3_u32 {
        for i in 0..20_000_u32 {
            tree.remove(point(i), u64::from(i)).unwrap();
        }
        assert!(tree.shape().unwrap().iter().map(|l| l.nodes).sum::<u64>() < 20, "round {round}");
        fill(&tree, 0..20_000);
        let used = tree.inner.pool.pages_in_use(tree.inner.file).unwrap();
        assert!(used * 5 <= full * 6, "round {round}: {used} pages after {full}");
    }
    assert_eq!(ids(&tree, None), (0..20_000).collect::<Vec<_>>());
}

/// Verifies: [DESIGN] D139: a scan paused partway while deletes empty and
/// merge leaves both behind and ahead of it returns every entry held
/// throughout exactly once, and no entry twice. Its pin keeps the retired
/// leaves' pages until it is dropped; freeing them at once would hand the
/// scan another node's page.
#[test]
fn a_paused_scan_survives_reclamation() {
    for (pause, window) in [(1_usize, None), (900, None), (40, Some(Bounds::new([0.0_f64, 0.0_f64], [600.0_f64, 991.0_f64]).unwrap()))] {
        let tree = tree();
        fill(&tree, 0..12_000);
        let mut scan = tree.scan(window);
        let mut seen: Vec<u64> = scan.by_ref().take(pause).map(|e| e.unwrap().1).collect();
        let removed = remove_left(&tree, 0..12_000, 500.0_f64);
        for i in (0..12_000_u32).filter(|i| i % 2 == 0) {
            if !removed.contains(&u64::from(i)) {
                tree.remove(point(i), u64::from(i)).unwrap();
            }
        }
        assert!(tree.reclaimed().0 > 0);
        assert!(tree.inner.retired.load(Ordering::Relaxed) > 0);
        seen.extend(scan.map(|e| e.unwrap().1));
        let unique: BTreeSet<u64> = seen.iter().copied().collect();
        assert_eq!(unique.len(), seen.len(), "an entry was returned twice");
        let held = (0..12_000_u32)
            .filter(|&i| i % 2 == 1 && point(i).lower()[0] >= 500.0_f64)
            .filter(|&i| window.is_none_or(|w: Bounds| w.intersects(point(i))))
            .map(u64::from);
        for id in held {
            assert!(unique.contains(&id), "held entry {id} was missed");
        }
    }
}

/// Removes and reinserts a third of the unstable rows, three times, the
/// last time leaving them out.
fn churn(tree: &RTree, writer: u32) {
    for round in 0..3_u32 {
        let mine = (0..8_000_u32).filter(|i| i % 4 != 0 && i % 3 == writer);
        mine.clone().for_each(|i| tree.remove(point(i), u64::from(i)).unwrap());
        if round < 2 {
            mine.for_each(|i| tree.insert(point(i), u64::from(i)).unwrap());
        }
    }
}

/// Verifies: [DESIGN] D139: scans on other threads, while writers remove
/// and reinsert rows so leaves empty, merge and split again, return every
/// row never removed exactly once each time; rows changed meanwhile are
/// optional to a scan, however the threads are
/// scheduled (R-TEST-5), as the check holds for every interleaving.
#[test]
fn concurrent_scans_neither_miss_nor_repeat_during_reclamation() {
    let tree = tree();
    fill(&tree, 0..8_000);
    let stable: BTreeSet<u64> = (0..8_000).filter(|id| id % 4 == 0).collect();
    std::thread::scope(|scope| {
        for writer in 0_u32..3 {
            let tree = tree.clone();
            std::thread::Builder::new()
                .name(format!("rtree-reclaimer-{writer}"))
                .spawn_scoped(scope, move || churn(&tree, writer))
                .unwrap();
        }
        for reader in 0_u32..2 {
            let (tree, stable) = (tree.clone(), &stable);
            std::thread::Builder::new()
                .name(format!("rtree-scanner-{reader}"))
                .spawn_scoped(scope, move || {
                    for _ in 0..40_u32 {
                        let seen: Vec<u64> = tree.scan(None).map(|e| e.unwrap().1).filter(|id| stable.contains(id)).collect();
                        let unique: BTreeSet<u64> = seen.iter().copied().collect();
                        assert_eq!(seen.len(), stable.len());
                        assert_eq!(&unique, stable);
                    }
                })
                .unwrap();
        }
    });
    assert!(tree.reclaimed().0 > 0);
    assert_eq!(ids(&tree, None), stable.into_iter().collect::<Vec<_>>());
}

/// Verifies: [DESIGN] D139: a leaf retired by a merge refuses writers.
/// an insert or remove that reaches it starts again from the root and
/// changes the merged leaf, so no change lands in an unreachable page.
#[test]
fn writers_never_change_a_retired_leaf() {
    let tree = tree();
    fill(&tree, 0..2_000);
    let removed = remove_left(&tree, 0..2_000, 600.0_f64);
    assert!(tree.reclaimed().0 > 0);
    for i in (0..2_000_u32).filter(|i| removed.contains(&u64::from(*i))) {
        tree.insert(point(i), u64::from(i)).unwrap();
    }
    let mut walk = scan::Walk::new(tree.clone(), None);
    while let Some((_, node)) = walk.node().unwrap() {
        assert!(!node.retired);
    }
    assert_eq!(ids(&tree, None), (0..2_000).collect::<Vec<_>>());
}

/// Verifies: [DESIGN] D139: split stamps come from the meta page's
/// counter, so after splits, merges and freed pages allocated again no two
/// live nodes share a stamp, and a tree opened again gives stamps past
/// every one it holds. A stamp repeated would let a scan stop before
/// entries a split moved right.
#[test]
fn split_stamps_never_repeat() {
    let tree = tree();
    fill(&tree, 0..6_000);
    remove_left(&tree, 0..6_000, 500.0_f64);
    fill(&tree, 6_000..9_000);
    let mut stamps = BTreeSet::new();
    let mut walk = scan::Walk::new(tree.clone(), None);
    while let Some((_, node)) = walk.node().unwrap() {
        assert!(stamps.insert(node.stamp), "stamp {} repeats", node.stamp);
    }
    drop(walk);
    let reopened = RTree::open(Arc::clone(&tree.inner.pool), tree.inner.file, tree.meta()).unwrap();
    assert!(reopened.fresh_stamp().unwrap() > *stamps.last().unwrap());
}

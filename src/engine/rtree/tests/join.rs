//! Intersection joins checked against Cartesian products and paused
//! across splits of both inputs; resources and malformed links included.

use super::*;

fn pairs(left: &RTree, right: &RTree) -> Vec<(u64, u64)> {
    let mut pairs: Vec<_> = left
        .join(right, &MemoryBudget::new(usize::MAX))
        .unwrap()
        .map(|e| {
            let (a, b) = e.unwrap();
            (a.1, b.1)
        })
        .collect();
    pairs.sort_unstable();
    pairs
}

fn exhaustive(left: &RTree, right: &RTree) -> Vec<(u64, u64)> {
    let a: Vec<_> = left.scan(None).map(Result::unwrap).collect();
    let b: Vec<_> = right.scan(None).map(Result::unwrap).collect();
    let mut expected = Vec::new();
    for (bounds, id) in a {
        for &(other, row) in &b {
            if bounds.intersects(other) {
                expected.push((id, row));
            }
        }
    }
    expected.sort_unstable();
    expected
}

fn windows(tree: &RTree, count: u32) {
    for n in 0..count {
        let lower = point(n).lower();
        let bounds = Bounds::new(lower, lower.map(|x| x + 50.0_f64)).unwrap();
        tree.insert(bounds, u64::from(n)).unwrap();
    }
}

/// Verifies: [DESIGN] Dual-tree joins equal exhaustive intersection,
/// with either side taller, empty inputs, points, and inclusive edges.
/// Left/right level mismatches independently require descent on that side.
#[test]
fn intersection_pairs_match_cartesian_filter() {
    let left = tree();
    let right = tree();
    assert_eq!(pairs(&left, &right), []);
    fill(&left, 0..8_000);
    assert_eq!(pairs(&left, &right), []);
    windows(&right, 300);
    right.insert(point(13), 999).unwrap();
    assert!(left.root().unwrap().level > right.root().unwrap().level);
    assert_eq!(pairs(&left, &right), exhaustive(&left, &right));
    assert_eq!(pairs(&right, &left), exhaustive(&right, &left));
    let zero = Bounds::point([0.0_f64, 0.0_f64]).unwrap();
    left.insert(zero, 10_000).unwrap();
    right.insert(Bounds::new([-1.0_f64, -1.0_f64], [0.0_f64, 0.0_f64]).unwrap(), 10_000).unwrap();
    assert!(pairs(&left, &right).contains(&(10_000, 10_000)));
}

/// Verifies: [DESIGN] A self-join retains both orientations and self
/// matches, while MVCC reference counts never multiply a matching pair.
#[test]
fn self_join_and_reference_counts_preserve_pair_multiplicity() {
    let tree = tree();
    windows(&tree, 400);
    for _ in 0_u32..3 {
        tree.insert(point(0), 999).unwrap();
    }
    assert_eq!(pairs(&tree, &tree), exhaustive(&tree, &tree));
}

/// Verifies: [DESIGN] An old root pair follows splits on both sides.
/// Each right/right product appears once, even after multiple root grows.
#[test]
fn old_roots_partition_split_pairs_without_duplicates() {
    let left = tree();
    let right = tree();
    fill(&left, 0..40);
    windows(&right, 40);
    let join = left.join(&right, &MemoryBudget::new(usize::MAX)).unwrap();
    fill(&left, 40..8_000);
    windows(&right, 300);
    let mut seen = BTreeSet::new();
    let mut found: Vec<_> = join
        .map(|e| {
            let (a, b) = e.unwrap();
            assert!(seen.insert((a.1, b.1)), "a split product was visited twice");
            (a.1, b.1)
        })
        .collect();
    found.sort_unstable();
    assert_eq!(found, exhaustive(&left, &right));
}

/// Verifies: [DESIGN] Pairs held throughout a paused join remain once
/// after both inputs split, including pairs in the copied active leaves.
#[test]
fn paused_join_preserves_stable_pairs_across_both_splits() {
    let left = tree();
    let right = tree();
    fill(&left, 0..300);
    windows(&right, 200);
    let expected = exhaustive(&left, &right);
    let mut join = left.join(&right, &MemoryBudget::new(usize::MAX)).unwrap();
    let first = join.next().unwrap().unwrap();
    fill(&left, 300..8_000);
    windows(&right, 1_000);
    let mut found: Vec<_> =
        std::iter::once(first).chain(join.map(Result::unwrap)).map(|(a, b)| (a.1, b.1)).filter(|&(a, b)| a < 300 && b < 200).collect();
    found.sort_unstable();
    assert_eq!(found, expected);
}

/// Verifies: [DESIGN] Join page copies and pending pairs reserve memory,
/// and dropping a partially consumed join releases it. Exhaustion
/// reports SQLCODE -918, both at construction and while expanding nodes.
#[test]
fn join_workspace_is_budgeted_and_released() {
    let left = tree();
    windows(&left, 8_000);
    let budget = MemoryBudget::new(0);
    assert_eq!(left.join(&left, &budget).err().unwrap().code, Code::Resources);
    budget.set_limit(usize::MAX);
    let mut join = left.join(&left, &budget).unwrap();
    let initial = budget.reserved();
    assert!(initial > 0);
    budget.set_limit(initial);
    assert_eq!(join.by_ref().find_map(Result::err).unwrap().code, Code::Resources);
    assert!(join.next().is_none());
    drop(join);
    assert_eq!(budget.reserved(), 0);
    budget.set_limit(usize::MAX);
    let mut unlimited = left.join(&left, &budget).unwrap();
    assert!(unlimited.next().unwrap().is_ok());
    drop(unlimited);
    assert_eq!(budget.reserved(), 0);
}

/// Verifies: [DESIGN] Each side independently refuses a split without
/// its right link and a child at the wrong level, SQLCODE -916 once.
#[test]
fn join_refuses_malformed_splits_and_levels() {
    for side in [false, true] {
        for level in [false, true] {
            let left = tree();
            let right = tree();
            fill(&left, 0..3);
            fill(&right, 0..3);
            let mut join = left.join(&right, &MemoryBudget::new(usize::MAX)).unwrap();
            let damaged = if side { &left } else { &right };
            let root = damaged.root().unwrap();
            let mut node = damaged.read(root.id).unwrap();
            if level {
                node.level += 1;
            } else {
                node.stamp += 1;
            }
            damaged.store(root.id, &node).unwrap();
            assert_eq!(join.next().unwrap().unwrap_err().code, Code::Corrupt);
            assert!(join.next().is_none());
        }
    }
}

/// Verifies: [DESIGN] A right-link cycle on either join input is bounded
/// by the file's page count and yields SQLCODE -916 once, then stops.
#[test]
fn join_refuses_cycles_on_either_input() {
    for side in [false, true] {
        let left = tree();
        let right = tree();
        let damaged = if side { &left } else { &right };
        fill(damaged, 0..u32::try_from(damaged.inner.capacity).unwrap());
        let root = damaged.root().unwrap();
        let node = damaged.split_node(root.id, damaged.read(root.id).unwrap()).unwrap();
        let sibling = node.right.unwrap();
        let mut linked = damaged.read(sibling).unwrap();
        linked.stamp = node.stamp + 1;
        linked.right = Some(root.id);
        damaged.store(sibling, &linked).unwrap();
        let mut join = left.join(&right, &MemoryBudget::new(usize::MAX)).unwrap();
        assert_eq!(join.by_ref().find_map(Result::err).unwrap().code, Code::Corrupt);
        assert!(join.next().is_none());
    }
}

//! Nearest-neighbour requirements: exact rectangle distances, ties,
//! bounded frontier memory and scans crossing structural changes.

use super::*;
use crate::engine::rtree::node::Entry;
use crate::spatial::DIMENSIONS;

fn ordered(tree: &RTree, query: Bounds) -> Vec<(f64, u64)> {
    tree.nearest(query, &MemoryBudget::new(usize::MAX)).unwrap().map(|e| e.map(|(d, _, id)| (d, id)).unwrap()).collect()
}

/// Verifies: [DESIGN] Best-first traversal returns the same nearest
/// points as exhaustive distance calculation and sorting, including
/// k=1, 10, 100 and the complete input; ties are broken by row identifier.
#[test]
fn nearest_points_match_exhaustive_sorting() {
    let tree = tree();
    fill(&tree, 0..2_000);
    for n in 0..20 {
        let query = point(n * 133);
        let mut expected: Vec<_> = (0..2_000).map(|i| (query.distance(point(i)), u64::from(i))).collect();
        expected.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let found = ordered(&tree, query);
        assert_eq!(found, expected);
        for k in [1, 10, 100] {
            let near: Vec<_> = tree.nearest(query, &MemoryBudget::new(usize::MAX)).unwrap().take(k).map(|e| e.unwrap().2).collect();
            assert_eq!(near, expected[..k].iter().map(|e| e.1).collect::<Vec<_>>());
        }
    }
}

/// Verifies: [DESIGN] A rectangle containing the query has zero distance
/// even when its centre is far away. Equal distances across different
/// leaves return in row-id order, not page or insertion order.
#[test]
fn rectangle_distances_and_cross_leaf_ties_are_exact() {
    let tree = tree();
    for n in (0_u32..300).rev() {
        let wide = Bounds::new([-f64::from(n), -1.0_f64], [f64::from(n), 1.0_f64]).unwrap();
        tree.insert(wide, u64::from(n)).unwrap();
    }
    let query = Bounds::point([0.0_f64, 0.0_f64]).unwrap();
    let found = ordered(&tree, query);
    assert_eq!(found.iter().map(|e| e.1).collect::<Vec<_>>(), (0..300).collect::<Vec<_>>());
    assert!(found.iter().all(|e| e.0.to_bits() == 0));
}

/// Verifies: [DESIGN] A nearest-neighbour cursor retains the old root's
/// expected stamp and traverses its split chain after root growth, with
/// the same distances and no duplicate entries as a new cursor.
#[test]
fn nearest_from_an_old_root_survives_growth() {
    let tree = tree();
    fill(&tree, 0..40);
    let query = point(17);
    let near = tree.nearest(query, &MemoryBudget::new(usize::MAX)).unwrap();
    fill(&tree, 40..8_000);
    let found: Vec<_> = near.map(|e| e.map(|(d, _, id)| (d, id)).unwrap()).collect();
    assert_eq!(found, ordered(&tree, query));
    assert_eq!(found.len(), 8_000);
}

/// Verifies: [DESIGN] The frontier is charged to the memory budget and
/// releases its reservation on cancellation. Exhausting the budget while
/// expanding a leaf reports SQLCODE -918 once and stops the cursor.
#[test]
fn nearest_frontier_is_budgeted_and_released() {
    let tree = tree();
    fill(&tree, 0..2_000);
    let budget = MemoryBudget::new(usize::MAX);
    let mut near = tree.nearest(point(0), &budget).unwrap();
    assert!(budget.reserved() > 0);
    assert!(near.next().unwrap().is_ok());
    drop(near);
    assert_eq!(budget.reserved(), 0);
    let mut limited = tree.nearest(point(0), &budget).unwrap();
    budget.set_limit(budget.reserved());
    let error = limited.by_ref().find_map(Result::err).unwrap();
    assert_eq!(error.code, Code::Resources);
    assert!(limited.next().is_none());
    drop(limited);
    assert_eq!(budget.reserved(), 0);
    budget.set_limit(0);
    assert_eq!(tree.nearest(point(0), &budget).err().unwrap().code, Code::Resources);
}

/// Verifies: [DESIGN] Empty trees have no nearest entry. Extreme finite
/// coordinates retain correct distance order even when a distance
/// overflows f64; every overflowing distance ties after finite distances.
#[test]
fn empty_and_extreme_distance_searches_are_defined() {
    let tree = tree();
    let query = Bounds::point([-f64::MAX, 0.0_f64]).unwrap();
    assert_eq!(ordered(&tree, query), []);
    for (id, x) in [(3, f64::MAX), (2, 0.0_f64), (1, -f64::MAX), (0, f64::MAX)] {
        tree.insert(Bounds::point([x, 0.0_f64]).unwrap(), id).unwrap();
    }
    let found = ordered(&tree, query);
    assert_eq!(found.iter().map(|e| e.1).collect::<Vec<_>>(), [1, 2, 0, 3]);
    assert!(found[2].0.is_infinite());
}

/// Verifies: [DESIGN] Enlarging a queued branch may add an entry nearer
/// than a result already returned. That concurrent entry is omitted;
/// stable entries retain distance order and appear exactly once.
#[test]
fn an_entry_inserted_behind_the_distance_cursor_is_omitted() {
    let tree = tree();
    fill(&tree, 0..200);
    let query = Bounds::point([-100.0_f64, -100.0_f64]).unwrap();
    let mut near = tree.nearest(query, &MemoryBudget::new(usize::MAX)).unwrap();
    let first = near.next().unwrap().unwrap();
    let root = tree.root().unwrap();
    assert_eq!(root.level, 1);
    let mut above = tree.read(root.id).unwrap();
    let child = above.entries.iter_mut().find(|e| e.bounds.distance_lower_bound(query) > first.0).unwrap();
    let id = PageId(child.id);
    child.bounds = child.bounds.union(query);
    tree.store(root.id, &above).unwrap();
    let mut leaf = tree.read(id).unwrap();
    assert!(leaf.entries.len() < tree.inner.capacity);
    leaf.entries.push(Entry { bounds: query, id: 9_999, value: 1 });
    tree.store(id, &leaf).unwrap();
    let found: Vec<_> = std::iter::once(first).chain(near.map(Result::unwrap)).collect();
    assert!(found.windows(2).all(|pair| pair[0].0 <= pair[1].0));
    let mut ids: Vec<_> = found.iter().map(|e| e.2).collect();
    ids.sort_unstable();
    assert_eq!(ids, (0..200).collect::<Vec<_>>());
    assert_eq!(ordered(&tree, query)[0].1, 9_999);
}

/// Verifies: [DESIGN] Nearest traversal refuses a changed stamp lacking
/// the required sibling, reporting SQLCODE -916 once and then stopping.
#[test]
fn nearest_refuses_a_missing_split_link() {
    let tree = tree();
    tree.insert(point(0), 0).unwrap();
    let root = tree.root().unwrap();
    let mut node = tree.read(root.id).unwrap();
    node.stamp += 1;
    tree.store(root.id, &node).unwrap();
    let mut near = tree.nearest(point(0), &MemoryBudget::new(usize::MAX)).unwrap();
    assert_eq!(near.next().unwrap().unwrap_err().code, Code::Corrupt);
    assert!(near.next().is_none());
}

/// Verifies: [DESIGN] D85: geometry candidates use monotone lower bounds
/// for leaves as well as branches, in bound/row order, through root growth.
/// Their frontier remains charged until the cursor is dropped.
#[test]
fn candidate_bounds_match_exhaustive_order_through_old_roots() {
    let tree = tree();
    fill(&tree, 0..40);
    let root = tree.root().unwrap();
    fill(&tree, 40..2_000);
    let query = Bounds::point([-3.0_f64, -4.0_f64]).unwrap();
    let domain = Bounds::new([-1000.0_f64; DIMENSIONS], [1000.0_f64; DIMENSIONS]).unwrap();
    let budget = MemoryBudget::new(usize::MAX);
    let mut near = Nearest::within(tree.clone(), root, (query, domain), &budget).unwrap().unwrap();
    assert!(budget.reserved() > 0);
    let actual: Vec<_> = near.by_ref().map(|e| e.map(|(d, _, row)| (d, row)).unwrap()).collect();
    let mut expected: Vec<_> = (0_u32..2_000).map(|n| (point(n).distance_lower_bound(query), u64::from(n))).collect();
    expected.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    assert_eq!(actual, expected);
    drop(near);
    assert_eq!(budget.reserved(), 0);
}

fn split_root() -> (RTree, Visit, Bounds) {
    let tree = tree();
    for n in 0..tree.inner.capacity {
        let x = if n.saturating_add(1) == tree.inner.capacity { 1e8_f64 } else { f64::from(u32::try_from(n).unwrap()) };
        tree.insert(Bounds::point([x, 0.0_f64]).unwrap(), u64::try_from(n).unwrap()).unwrap();
    }
    let root = tree.root().unwrap();
    tree.split_node(root.id, tree.read(root.id).unwrap()).unwrap();
    let domain = Bounds::new([-1000.0_f64; DIMENSIONS], [1000.0_f64; DIMENSIONS]).unwrap();
    let left = tree.read(root.id).unwrap();
    assert!(domain.contains(left.bounds().unwrap()));
    assert!(!domain.contains(tree.read(left.right.unwrap()).unwrap().bounds().unwrap()));
    (tree, root, domain)
}

/// Verifies: [DESIGN] D15, D85: the finite-distance certificate covers
/// every root split sibling, not only the safe left half. Query and data
/// outside the domain independently decline; an empty tree is supported.
#[test]
fn candidate_range_proof_includes_unposted_root_siblings() {
    let (tree, root, domain) = split_root();
    let query = Bounds::point([0.0_f64; DIMENSIONS]).unwrap();
    let budget = MemoryBudget::new(usize::MAX);
    assert!(Nearest::within(tree.clone(), root, (query, domain), &budget).unwrap().is_none());
    let wide = domain.union(Bounds::point([1e8_f64, 0.0_f64]).unwrap());
    let found = Nearest::within(tree.clone(), root, (query, wide), &budget).unwrap().unwrap();
    assert_eq!(found.map(Result::unwrap).count(), tree.inner.capacity);
    let empty = super::tree();
    let outside = Bounds::point([1001.0_f64, 0.0_f64]).unwrap();
    assert!(empty.nearest_candidates(outside, domain, &budget).unwrap().is_none());
    assert_eq!(empty.nearest_candidates(query, domain, &budget).unwrap().unwrap().count(), 0);
    assert_eq!(budget.reserved(), 0);
}

/// Verifies: [DESIGN] D85: root-envelope certification refuses missing
/// siblings, wrong levels and sibling cycles with SQLCODE -916, rather
/// than treating an incomplete envelope as a safe distance domain.
#[test]
fn candidate_range_proof_refuses_damaged_root_chains() {
    let budget = MemoryBudget::new(usize::MAX);
    let query = Bounds::point([0.0_f64; DIMENSIONS]).unwrap();
    for damage in 0_u8..3 {
        let (tree, root, domain) = split_root();
        let mut node = tree.read(root.id).unwrap();
        let right = node.right.unwrap();
        match damage {
            0 => node.right = None,
            1 => node.level += 1,
            _ => {
                let mut sibling = tree.read(right).unwrap();
                sibling.stamp = node.stamp;
                sibling.right = Some(root.id);
                tree.store(right, &sibling).unwrap();
            }
        }
        tree.store(root.id, &node).unwrap();
        let wide = domain.union(Bounds::point([1e8_f64, 0.0_f64]).unwrap());
        let error = Nearest::within(tree, root, (query, wide), &budget).err().unwrap();
        assert_eq!(error.code, Code::Corrupt, "{damage}");
    }
}

/// Verifies: [DESIGN] D135: the farthest-first traversal returns every
/// point, the farthest first, each with a negated
/// bound no greater than its negated distance, nondecreasing, under the
/// finite-distance certificate; a query outside the domain declines.
#[test]
fn farthest_candidates_come_in_decreasing_distance() {
    let tree = tree();
    fill(&tree, 0..500);
    let domain = Bounds::new([-1e9_f64; DIMENSIONS], [1e9_f64; DIMENSIONS]).unwrap();
    let budget = MemoryBudget::new(usize::MAX);
    for n in [0_u32, 77, 250, 499] {
        let query = point(n);
        let found: Vec<_> = tree.farthest_candidates(query, domain, &budget).unwrap().unwrap().map(Result::unwrap).collect();
        assert_eq!(found.len(), 500);
        let mut last = f64::NEG_INFINITY;
        for (bound, bounds, _) in &found {
            let distance = query.distance(*bounds);
            assert!(*bound >= last && *bound <= -distance, "{bound} {distance}");
            last = *bound;
        }
        let exact = (0..500).map(|i| query.distance(point(i))).fold(0.0_f64, f64::max);
        assert_eq!(query.distance(found[0].1).to_bits(), exact.to_bits());
    }
    let outside = Bounds::point([2e9_f64, 0.0_f64]).unwrap();
    assert!(tree.farthest_candidates(outside, domain, &budget).unwrap().is_none());
    assert_eq!(budget.reserved(), 0);
}

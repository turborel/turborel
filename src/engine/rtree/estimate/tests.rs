//! Statistics sample across branches and decline unreliable paths.

use super::*;
use crate::engine::rtree::tests::tree;

fn point(x: u32, y: u32) -> Bounds {
    Bounds::point([f64::from(x), f64::from(y)]).unwrap()
}

fn allocated(tree: &RTree, level: u8, entries: Vec<Entry>, bounds: Bounds) -> Entry {
    let (id, mut page) = tree.inner.pool.new_page(tree.inner.file).unwrap();
    let mut node = Node::empty(id, level);
    node.entries = entries;
    *page = node.bytes().unwrap();
    Entry { bounds, id: id.0, value: node.stamp }
}

fn root(tree: &RTree, level: u8, entries: Vec<Entry>) {
    let id = tree.root().unwrap().id;
    let mut node = Node::empty(id, level);
    node.entries = entries;
    tree.store(id, &node).unwrap();
}

/// Verifies: [DESIGN] D35, D82: estimation visits the whole root range,
/// weighs differently populated leaves and counts candidates once despite
/// their version reference counts. Closed boundaries and disjoint queries
/// have the same meaning as search. Repeated estimates are deterministic.
#[test]
fn estimates_cover_root_branches_and_count_entries_not_references() {
    let tree = tree();
    assert_eq!(tree.estimated_matches(point(0, 0)), Some(0));
    let mut branches = Vec::new();
    for x in 0..64_u32 {
        let entries = (0..=x).map(|y| Entry { bounds: point(x, y), id: u64::from(x * 100 + y), value: u64::from(y + 1) }).collect();
        let bounds = Bounds::new([f64::from(x), 0.0_f64], [f64::from(x), f64::from(x)]).unwrap();
        branches.push(allocated(&tree, 0, entries, bounds));
    }
    root(&tree, 1, branches);
    for query in [
        Bounds::new([0.0_f64, 0.0_f64], [100.0_f64, 100.0_f64]).unwrap(),
        Bounds::new([0.0_f64, 0.0_f64], [31.0_f64, 100.0_f64]).unwrap(),
        point(63, 63),
        point(100, 100),
    ] {
        let exact = tree.scan(Some(query)).map(Result::unwrap).count();
        assert_eq!(tree.estimated_matches(query), Some(exact));
        assert_eq!(tree.estimated_matches(query), Some(exact));
    }
}

/// Verifies: [DESIGN] D35, D82: a sampled path accounts for branching at
/// every level, not just the root. A leaf root is counted exactly and an
/// empty internal branch contributes zero without dividing by zero.
#[test]
fn estimates_weight_every_branch_and_handle_empty_nodes() {
    let tree = tree();
    let bounds = point(0, 0);
    let leaf = || (0..4_u64).map(|id| Entry { bounds, id, value: 1 }).collect();
    root(&tree, 0, leaf());
    assert_eq!(tree.estimated_matches(bounds), Some(4));
    let mut branches = Vec::new();
    for _ in 0..2_usize {
        let leaves = (0..2_usize).map(|_| allocated(&tree, 0, leaf(), bounds)).collect();
        branches.push(allocated(&tree, 1, leaves, bounds));
    }
    root(&tree, 2, branches);
    assert_eq!(tree.estimated_matches(bounds), Some(16));
    let empty = allocated(&tree, 1, Vec::new(), bounds);
    root(&tree, 2, vec![empty]);
    assert_eq!(tree.estimated_matches(bounds), Some(0));
    root(&tree, 1, Vec::new());
    assert_eq!(tree.estimated_matches(bounds), Some(0));
}

/// Verifies: [DESIGN] D35, D82: split stamps, pending links and wrong
/// child levels make statistics decline. A fixed read budget bounds even
/// deep paths; these fallbacks do not change ordinary search.
#[test]
fn estimates_decline_changed_paths_and_exhausted_page_budgets() {
    let tree = tree();
    let bounds = point(0, 0);
    let leaf = allocated(&tree, 0, vec![Entry { bounds, id: 1, value: 1 }], bounds);
    root(&tree, 1, vec![Entry { value: leaf.value + 1, ..leaf }]);
    assert_eq!(tree.estimated_matches(bounds), None);
    root(&tree, 2, vec![leaf]);
    assert_eq!(tree.estimated_matches(bounds), None);
    root(&tree, 1, vec![leaf]);
    let id = tree.root().unwrap().id;
    let mut node = tree.read(id).unwrap();
    node.pending = true;
    node.right = Some(PageId(leaf.id));
    tree.store(id, &node).unwrap();
    assert_eq!(tree.estimated_matches(bounds), None);
    let mut branch = leaf;
    for level in 1..5_u8 {
        branch = allocated(&tree, level, vec![branch], bounds);
    }
    root(&tree, 5, vec![branch]);
    assert_eq!(tree.estimated_matches(bounds), None);
    assert_eq!(tree.scan(Some(bounds)).map(Result::unwrap).count(), 1);
}

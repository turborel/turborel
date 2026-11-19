//! Deterministic writer interleavings: a parent splits before a child is
//! posted, a parent fills first, and an already posted split is retried.

use super::*;
use crate::engine::rtree::node::Entry;

fn branch(capacity: usize) -> (RTree, Visit) {
    let mut tree = tree();
    Arc::get_mut(&mut tree.inner).unwrap().capacity = capacity;
    let root = tree.root().unwrap();
    let mut above = Node::empty(root.stamp, 1);
    for n in 0_u32..4 {
        let (id, mut page) = tree.inner.pool.new_page(tree.inner.file).unwrap();
        let mut node = Node::empty(tree.fresh_stamp().unwrap(), 0);
        for j in 0_u32..2 {
            let bounds = Bounds::point([f64::from(n.wrapping_mul(10).wrapping_add(j)), f64::from(j)]).unwrap();
            node.entries.push(Entry { bounds, id: u64::from(n.wrapping_mul(2).wrapping_add(j)), value: 1 });
        }
        *page = node.bytes().unwrap();
        above.entries.push(Entry { bounds: node.bounds().unwrap(), id: id.0, value: node.stamp });
    }
    tree.store(root.id, &above).unwrap();
    let current = tree.root().unwrap();
    (tree, current)
}

/// Verifies: [DESIGN] A writer posts into the parent containing its
/// original child even when the child entry moved right in a parent
/// split after descent. Posting into a newly chosen geometric parent
/// would lose the old reader's route to the split.
#[test]
fn stale_parent_paths_find_children_moved_right() {
    let (tree, old_root) = branch(4);
    let left = tree.split_node(old_root.id, tree.read(old_root.id).unwrap()).unwrap();
    let right = tree.read(left.right.unwrap()).unwrap();
    let child = PageId(right.entries[0].id);
    tree.split_node(child, tree.read(child).unwrap()).unwrap();
    tree.finish_split(child, vec![old_root]).unwrap();
    assert!(!tree.read(child).unwrap().pending);
    assert_eq!(ids(&tree, None), (0..8).collect::<Vec<_>>());
    tree.insert(point(999), 999).unwrap();
    assert_eq!(ids(&tree, None), (0..8).chain([999]).collect::<Vec<_>>());
}

/// Verifies: [DESIGN] A full parent is split before taking the child's
/// new sibling. The child remains searchable while posting is deferred;
/// a writer with no remaining ancestry finds the original parent again
/// under the new root, and completes the pending split.
#[test]
fn helping_a_full_parent_then_rebuilding_ancestry_preserves_entries() {
    let (tree, old_root) = branch(4);
    let child = PageId(tree.read(old_root.id).unwrap().entries[0].id);
    tree.split_node(child, tree.read(child).unwrap()).unwrap();
    tree.finish_split(child, vec![old_root]).unwrap();
    assert!(tree.root().unwrap().level > old_root.level);
    assert!(tree.read(child).unwrap().pending);
    assert_eq!(ids(&tree, None), (0..8).collect::<Vec<_>>());
    tree.finish_split(child, Vec::new()).unwrap();
    assert!(!tree.read(child).unwrap().pending);
    assert_eq!(ids(&tree, None), (0..8).collect::<Vec<_>>());
}

/// Verifies: [DESIGN] A parent posting completed before clearing the
/// child's pending flag is recognized on retry. Retrying clears the flag
/// without adding the sibling to the parent a second time.
#[test]
fn retrying_a_completed_parent_post_does_not_duplicate_the_sibling() {
    let (tree, root) = branch(8);
    let child = PageId(tree.read(root.id).unwrap().entries[0].id);
    tree.split_node(child, tree.read(child).unwrap()).unwrap();
    tree.finish_split(child, vec![root]).unwrap();
    let count = tree.read(root.id).unwrap().entries.len();
    let mut node = tree.read(child).unwrap();
    node.pending = true;
    tree.store(child, &node).unwrap();
    tree.finish_split(child, vec![root]).unwrap();
    assert!(!tree.read(child).unwrap().pending);
    assert_eq!(tree.read(root.id).unwrap().entries.len(), count);
    assert_eq!(ids(&tree, None), (0..8).collect::<Vec<_>>());
}

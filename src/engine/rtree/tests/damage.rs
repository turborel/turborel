//! Allocation failures and corrupt spatial pages must not become partial
//! inserts, panics, infinite traversals or silent omissions.

use super::*;

/// Verifies: [DESIGN] Exhausting the budget before a split, and after
/// publishing its sibling but before making the new root, both leave the
/// failed insert absent. Existing entries remain searchable. Restoring
/// room lets a later insert finish the split and add exactly one entry.
#[test]
fn failed_split_and_root_allocation_leave_no_insert() {
    for extra in [0, 4096] {
        let budget = MemoryBudget::new(usize::MAX);
        let tree = tree_in(&budget);
        let count = u32::try_from(tree.inner.capacity).unwrap();
        fill(&tree, 0..count);
        budget.set_limit(budget.reserved() + extra);
        assert_eq!(tree.insert(point(count), u64::from(count)).unwrap_err().code, Code::Resources);
        assert_eq!(ids(&tree, None), (0..u64::from(count)).collect::<Vec<_>>());
        assert_eq!(tree.counts().0, u64::from(extra > 0));
        budget.set_limit(usize::MAX);
        tree.insert(point(count), u64::from(count)).unwrap();
        assert_eq!(ids(&tree, None), (0..=u64::from(count)).collect::<Vec<_>>());
    }
}

/// Verifies: [DESIGN] The reference counter cannot wrap to zero and
/// silently delete every version of an entry. Overflow is SQLCODE -910
/// and leaves the old count unchanged.
#[test]
fn reference_count_overflow_changes_nothing() {
    let tree = tree();
    tree.insert(point(0), 0).unwrap();
    let root = tree.root().unwrap();
    let mut node = tree.read(root.id).unwrap();
    node.entries[0].value = u64::MAX;
    tree.store(root.id, &node).unwrap();
    assert_eq!(tree.insert(point(0), 0).unwrap_err().code, Code::Api);
    assert_eq!(tree.read(root.id).unwrap().entries[0].value, u64::MAX);
    assert_eq!(ids(&tree, None), [0]);
}

/// Verifies: [DESIGN] A node's format, bounded level, flags, split
/// link, length, finite coordinates and positive reference counts are
/// independently checked (MC/DC), with SQLCODE -916 for each defect.
#[test]
fn invalid_node_fields_are_refused() {
    let tree = tree();
    tree.insert(point(0), 0).unwrap();
    let root = tree.root().unwrap();
    let bytes = tree.read(root.id).unwrap().bytes().unwrap();
    let mut cases = Vec::new();
    for (offset, byte) in [(0, 99), (1, node::MAX_LEVEL + 1), (2, 4), (2, 1), (19, 2)] {
        let mut damaged = bytes.clone();
        damaged[offset] = byte;
        cases.push(damaged);
    }
    let mut nan = bytes.clone();
    nan[HEADER..HEADER + 8].copy_from_slice(&f64::NAN.to_bits().to_le_bytes());
    cases.push(nan);
    let mut zero = bytes.clone();
    zero[HEADER + 40..HEADER + 48].fill(0);
    cases.push(zero);
    let mut truncated = bytes.clone();
    truncated.pop();
    cases.push(truncated);
    for damaged in cases {
        assert_eq!(Node::parse(&damaged).err().unwrap().code, Code::Corrupt);
    }
    assert!(Node::parse(&bytes).is_ok());
}

/// Verifies: [DESIGN] A corrupt right-link cycle terminates with SQLCODE
/// -916; the iterator gives no more entries after its error.
#[test]
fn corrupt_right_link_cycles_terminate() {
    let tree = tree();
    fill(&tree, 0..u32::try_from(tree.inner.capacity).unwrap());
    let root = tree.root().unwrap();
    let left = tree.split_node(root.id, tree.read(root.id).unwrap()).unwrap();
    let right_id = left.right.unwrap();
    let mut right = tree.read(right_id).unwrap();
    right.stamp = left.stamp + 1;
    right.right = Some(root.id);
    tree.store(right_id, &right).unwrap();
    let mut scan = tree.scan(None);
    let error = scan.by_ref().find_map(Result::err).unwrap();
    assert_eq!(error.code, Code::Corrupt);
    assert!(scan.next().is_none());
}

/// Verifies: [DESIGN] A child at the wrong level cannot silently turn
/// row identifiers into child pointers, and a node linking to itself is
/// rejected before traversal. Both are SQLCODE -916.
#[test]
fn wrong_child_levels_and_self_links_are_refused() {
    let tree = tree();
    fill(&tree, 0..200);
    let root = tree.root().unwrap();
    let mut node = tree.read(root.id).unwrap();
    let child_id = PageId(node.entries[0].id);
    let mut child = tree.read(child_id).unwrap();
    child.level = root.level;
    tree.store(child_id, &child).unwrap();
    assert_eq!(tree.scan(None).find_map(Result::err).unwrap().code, Code::Corrupt);
    node.right = Some(root.id);
    tree.store(root.id, &node).unwrap();
    assert_eq!(tree.read(root.id).err().unwrap().code, Code::Corrupt);
}

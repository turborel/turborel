//! R-link requirements, including deterministic scans across structural
//! changes, unfinished splits and root growth, plus concurrent counts.

use std::collections::BTreeSet;

use super::*;
use crate::disk::encoding::Pipeline;
use crate::disk::io::Durability;
use crate::disk::memory::MemoryFileSystem;
use crate::disk::pager::Pager;
use crate::hash::crc::{CRC32_ISCSI, Crc};
use crate::memory::MemoryBudget;

mod damage;
mod paths;

fn tree() -> RTree {
    tree_in(&MemoryBudget::new(usize::MAX))
}

fn tree_in(budget: &Arc<MemoryBudget>) -> RTree {
    let fs = MemoryFileSystem::within(Arc::clone(budget), "R-tree test pages");
    let pager =
        Pager::create(&fs, std::path::Path::new("/tree"), Arc::new(Crc::new(CRC32_ISCSI).unwrap()), Pipeline::none(), 4096).unwrap();
    let pool = Arc::new(BufferPool::in_budget(budget, (2 << 20_u32, 4096), "R-tree test pool").unwrap());
    let file = pool.add_pager(Arc::new(pager));
    RTree::create(pool, file).unwrap()
}

fn point(n: u32) -> Bounds {
    Bounds::point([f64::from(n.wrapping_mul(17) % 997), f64::from(n.wrapping_mul(71) % 991)]).unwrap()
}

fn fill(tree: &RTree, range: std::ops::Range<u32>) {
    for i in range {
        tree.insert(point(i), u64::from(i)).unwrap();
    }
}

fn ids(tree: &RTree, bounds: Option<Bounds>) -> Vec<u64> {
    let mut rows: Vec<_> = tree.scan(bounds).map(|e| e.unwrap().1).collect();
    rows.sort_unstable();
    rows
}

/// Verifies: [DESIGN] Closed spatial window searches match exhaustive
/// intersection after enough inserts to split leaves and internal nodes.
/// Points on a window's boundary remain candidates, and each row is once.
#[test]
fn windows_match_exhaustive_search_across_levels() {
    let tree = tree();
    fill(&tree, 0..10_000);
    assert!(tree.root().unwrap().level >= 2);
    for i in 0..100 {
        let lower = point(i).lower();
        let upper = lower.map(|v| v + 50.0_f64);
        let window = Bounds::new(lower, upper).unwrap();
        let expected: Vec<_> = (0..10_000).filter(|&n| window.intersects(point(n))).map(u64::from).collect();
        assert_eq!(ids(&tree, Some(window)), expected);
    }
    assert_eq!(ids(&tree, None), (0..10_000).collect::<Vec<_>>());
    assert!(tree.counts().0 > 0);
}

/// Verifies: [DESIGN] Entries held throughout a scan are returned once
/// despite repeated splits and root growth after the scan queued its
/// children. Ignoring the remembered split stamp loses these entries.
#[test]
fn a_scan_survives_splits_without_missing_or_repeating_old_entries() {
    let tree = tree();
    fill(&tree, 0..300);
    let mut scan = tree.scan(None);
    let first = scan.next().unwrap().unwrap().1;
    fill(&tree, 300..10_000);
    let mut old: Vec<_> = std::iter::once(first).chain(scan.map(|e| e.unwrap().1)).filter(|&id| id < 300).collect();
    old.sort_unstable();
    assert_eq!(old, (0..300).collect::<Vec<_>>());
    assert!(tree.counts().1 > 0);
}

/// Verifies: [DESIGN] A cursor that captured a root before any split
/// follows the old root's chain after the tree grows several levels.
#[test]
fn an_old_root_remains_a_complete_starting_point() {
    let tree = tree();
    fill(&tree, 0..40);
    let scan = tree.scan(None);
    fill(&tree, 40..8_000);
    let all: Vec<_> = scan.map(|e| e.unwrap().1).collect();
    let distinct: BTreeSet<_> = all.iter().copied().collect();
    assert_eq!(all.len(), distinct.len());
    assert_eq!(distinct, (0..8_000).collect());
}

/// Verifies: [DESIGN] A split published before its parent update remains
/// searchable; the next insert helps post it. Removing every entry during
/// that pause must not turn a valid empty tree into corruption.
#[test]
fn unfinished_splits_are_searchable_and_helped_even_when_emptied() {
    for empty in [false, true] {
        let tree = tree();
        let count = u32::try_from(tree.inner.capacity).unwrap();
        fill(&tree, 0..count);
        let root = tree.root().unwrap();
        let node = tree.read(root.id).unwrap();
        tree.split_node(root.id, node).unwrap();
        assert_eq!(ids(&tree, None), (0..u64::from(count)).collect::<Vec<_>>());
        if empty {
            for n in 0..count {
                tree.remove(point(n), u64::from(n)).unwrap();
            }
        }
        tree.insert(point(count), u64::from(count)).unwrap();
        let expected = if empty { vec![u64::from(count)] } else { (0..=u64::from(count)).collect() };
        assert_eq!(ids(&tree, None), expected);
        assert!(!tree.read(root.id).unwrap().pending);
    }
}

/// Verifies: [DESIGN] Repeated MVCC keys count references but scan once;
/// removing an absent pair is a no-op and removing one version retains
/// the other. Different bounds for the same row are independent entries.
#[test]
fn reference_counts_preserve_versions_and_absent_removes_do_nothing() {
    let tree = tree();
    for _ in 0_u32..3 {
        tree.insert(point(1), 7).unwrap();
    }
    tree.insert(point(2), 7).unwrap();
    tree.remove(point(3), 7).unwrap();
    tree.remove(point(1), 99).unwrap();
    assert_eq!(ids(&tree, None), [7, 7]);
    for _ in 0_u32..2 {
        tree.remove(point(1), 7).unwrap();
        assert_eq!(ids(&tree, Some(point(1))), [7]);
    }
    tree.remove(point(1), 7).unwrap();
    tree.remove(point(1), 7).unwrap();
    assert_eq!(ids(&tree, None), [7]);
    tree.remove(point(2), 7).unwrap();
    assert_eq!(ids(&tree, None), []);
}

/// Verifies: [DESIGN] Concurrent writers cannot lose counts for the same
/// row or lose distinct entries during splits. Counts are checked after
/// joining, independent of how threads were scheduled (R-TEST-5).
#[test]
fn concurrent_writers_preserve_shared_counts_and_distinct_entries() {
    let tree = tree();
    std::thread::scope(|scope| {
        let mut threads = Vec::new();
        for thread in 0_u32..8 {
            let tree = &tree;
            threads.push(
                std::thread::Builder::new()
                    .name(format!("rtree-writer-{thread}"))
                    .spawn_scoped(scope, move || {
                        for n in 0..400 {
                            let id = thread * 400 + n;
                            tree.insert(point(id), u64::from(id)).unwrap();
                            tree.insert(point(99_999), 99_999).unwrap();
                        }
                    })
                    .unwrap(),
            );
        }
        for thread in threads {
            thread.join().unwrap();
        }
    });
    assert_eq!(ids(&tree, None).len(), 3_201);
    for _ in 0_u32..3_199 {
        tree.remove(point(99_999), 99_999).unwrap();
    }
    assert_eq!(ids(&tree, Some(point(99_999))), [99_999]);
    tree.remove(point(99_999), 99_999).unwrap();
    assert_eq!(ids(&tree, None), (0..3_200).collect::<Vec<_>>());
}

/// Verifies: [DESIGN] The persistent meta page restores every counted
/// entry after checkpoint and reopen; an incompatible format is refused
/// with SQLCODE -916 rather than read as another tree's pages.
#[test]
fn checkpointed_pages_reopen_and_invalid_formats_are_refused() {
    let tree = tree();
    fill(&tree, 0..500);
    tree.inner.pool.sync(tree.inner.file, Durability::Full).unwrap();
    let reopened = RTree::open(Arc::clone(&tree.inner.pool), tree.inner.file, tree.meta()).unwrap();
    assert_eq!(ids(&reopened, None), (0..500).collect::<Vec<_>>());
    drop(reopened);
    *tree.inner.pool.write(tree.inner.file, tree.meta()).unwrap() = vec![META_TAG, META_VERSION + 1];
    assert_eq!(RTree::open(Arc::clone(&tree.inner.pool), tree.inner.file, tree.meta()).err().unwrap().code, Code::Corrupt);
    let mut scan = tree.scan(None);
    assert_eq!(scan.next().unwrap().unwrap_err().code, Code::Corrupt);
    assert!(scan.next().is_none());
}

/// Verifies: [DESIGN] Discard frees every node, including empty leaves,
/// and the meta page, so repeated create/drop does not retain their space.
#[test]
fn discarded_trees_release_all_their_pages() {
    let tree = tree();
    fill(&tree, 0..500);
    for n in 0..500 {
        tree.remove(point(n), u64::from(n)).unwrap();
    }
    tree.free().unwrap();
    assert_eq!(tree.inner.pool.pages_in_use(tree.inner.file).unwrap(), 0);
}

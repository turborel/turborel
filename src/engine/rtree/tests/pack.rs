//! Packed loads: the same answers as inserted trees, counted duplicates,
//! writers' entries kept, spilled sorts, and failures that change nothing.

use super::*;
use crate::spill::testing::{Held, areas};
use crate::spill::{Area, WorkAreas};

/// A work area small enough that thousands of entries spill.
fn spilling() -> (Arc<WorkAreas>, Area) {
    (areas(Some(Arc::new(Held { broken: false })), usize::MAX), Area::new(8_192, 1_024))
}

fn load(tree: &RTree, range: std::ops::Range<u32>) {
    let (work, area) = spilling();
    let mut entries = range.map(|i| Ok((point(i), u64::from(i))));
    tree.load(&mut entries, (&work, area)).unwrap();
}

fn window(i: u32) -> Bounds {
    let lower = point(i).lower();
    Bounds::new(lower, lower.map(|v| v + 40.0_f64)).unwrap()
}

/// Verifies: [DESIGN] D139: a tree packed by Sort-Tile-Recursive through
/// a spilled sort answers every window and the full scan exactly as one
/// built entry by entry, with leaves near nine tenths full, fewer nodes
/// and less leaf overlap than the inserted tree. Tiled from the root
/// down, no two nodes of a level overlap, branches included.
#[test]
fn packed_and_inserted_trees_answer_alike() {
    let (packed, inserted) = (tree(), tree());
    load(&packed, 0..20_000);
    fill(&inserted, 0..20_000);
    for i in 0..200 {
        assert_eq!(ids(&packed, Some(window(i))), ids(&inserted, Some(window(i))));
    }
    assert_eq!(ids(&packed, None), (0..20_000).collect::<Vec<_>>());
    let (p, n) = (packed.shape().unwrap(), inserted.shape().unwrap());
    assert!(p[0].fill > 0.85_f64 && p[0].fill <= 0.9_f64, "{p:?}");
    assert!(p[0].nodes < n[0].nodes && p[0].overlap < n[0].overlap, "{p:?} {n:?}");
    assert!(p.iter().all(|level| level.overlap == 0.0_f64), "{p:?}");
    assert_eq!(p[0].entries, 20_000);
    assert_eq!(p.last().unwrap().nodes, 1);
}

/// Verifies: [DESIGN] D139: identical entries loaded twice become one
/// entry counting two references, returned once; one remove leaves it and
/// a second takes it out, as for inserted references.
#[test]
fn duplicate_loaded_entries_are_counted() {
    let tree = tree();
    let (work, area) = spilling();
    let mut entries = (0..3_000_u32).chain(0..10).map(|i| Ok((point(i), u64::from(i))));
    tree.load(&mut entries, (&work, area)).unwrap();
    assert_eq!(ids(&tree, None), (0..3_000).collect::<Vec<_>>());
    tree.remove(point(3), 3).unwrap();
    assert!(ids(&tree, Some(point(3))).contains(&3));
    tree.remove(point(3), 3).unwrap();
    assert!(!ids(&tree, Some(point(3))).contains(&3));
}

/// Verifies: [DESIGN] D139: a load into a tree that writers already
/// changed keeps their entries, an entry in both counted once per side,
/// and then takes inserts and removes as any tree does. The old nodes'
/// pages are freed once later operations move the epoch on.
#[test]
fn a_load_keeps_the_writers_entries() {
    let tree = tree();
    fill(&tree, 19_000..20_000);
    load(&tree, 0..19_100);
    let mut expected: Vec<u64> = (0..20_000).collect();
    assert_eq!(ids(&tree, None), expected);
    tree.remove(point(19_050), 19_050).unwrap();
    assert!(ids(&tree, Some(point(19_050))).contains(&19_050));
    tree.remove(point(19_050), 19_050).unwrap();
    expected.retain(|&id| id != 19_050);
    fill(&tree, 20_000..21_000);
    expected.extend(20_000..21_000);
    assert_eq!(ids(&tree, None), expected);
    assert_eq!(tree.inner.retired.load(Ordering::Relaxed), 0);
}

/// Verifies: [DESIGN] D139: an entry that fails partway, or a work area
/// that fails, leaves the tree as it was and frees every page the load
/// made; an empty load leaves an empty tree that takes inserts.
#[test]
fn a_failed_or_empty_load_changes_nothing() {
    let tree = tree();
    fill(&tree, 0..500);
    let before = tree.inner.pool.pages_in_use(tree.inner.file).unwrap();
    let (work, area) = spilling();
    let failing = (0..5_000_u32).map(|i| if i == 4_000 { Err(corrupt("told to fail")) } else { Ok((point(i + 500), u64::from(i + 500))) });
    assert_eq!(tree.load(&mut failing.into_iter(), (&work, area)).unwrap_err().code, Code::Corrupt);
    let broken = areas(Some(Arc::new(Held { broken: true })), usize::MAX);
    let mut entries = (500..5_000_u32).map(|i| Ok((point(i), u64::from(i))));
    assert!(tree.load(&mut entries, (&broken, area)).is_err());
    assert_eq!(ids(&tree, None), (0..500).collect::<Vec<_>>());
    assert_eq!(tree.inner.pool.pages_in_use(tree.inner.file).unwrap(), before);
    let empty = super::tree();
    empty.load(&mut std::iter::empty(), (&work, area)).unwrap();
    assert_eq!(ids(&empty, None), Vec::<u64>::new());
    fill(&empty, 0..10);
    assert_eq!(ids(&empty, None), (0..10).collect::<Vec<_>>());
}

/// Verifies: [DESIGN] D139: splits after a packed load take stamps from
/// the meta page's counter past every packed node's, so a scan that
/// started before the inserts still sees each loaded entry once.
#[test]
fn inserts_after_a_load_keep_scans_exact() {
    let tree = tree();
    load(&tree, 0..5_000);
    let mut scan = tree.scan(None);
    let first = scan.next().unwrap().unwrap().1;
    fill(&tree, 5_000..15_000);
    let mut old: Vec<_> = std::iter::once(first).chain(scan.map(|e| e.unwrap().1)).filter(|&id| id < 5_000).collect();
    old.sort_unstable();
    assert_eq!(old, (0..5_000).collect::<Vec<_>>());
}

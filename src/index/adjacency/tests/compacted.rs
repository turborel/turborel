//! The compacted adjacency and its delta against the tree: built by
//! maintenance and by a load, read while written, dropped, and refused by
//! a budget too small (RULES.md D-GRAPH-ADJACENCY, D38).

use std::thread;

use super::*;
use crate::index::adjacency::delta::LEAST_MARKS;
use crate::index::adjacency::keys::Encoded;

/// An index of `method` holding seeded random edges, with its model.
fn filled(method: &AdjacencyMethod, seed: u64, count: usize) -> (Arc<AdjacencyIndex>, Model) {
    let ix = index(method, Kept::Both);
    let (mut model, mut state) = (Model::default(), seed);
    for _ in 0..count {
        model.insert(&ix, random_edge(&mut state, 60, 500));
    }
    (ix, model)
}

/// Whether `vertex`'s list in `direction` is read of the compacted adjacency.
fn compacted(index: &AdjacencyIndex, vertex: i128, direction: Direction) -> bool {
    let bytes = Encoded::of(&edge(vertex, 0, 0), &index.layout).unwrap().source;
    index.delta.csr(direction, &bytes).unwrap().is_some()
}

/// Verifies: [DESIGN] RULES.md D-GRAPH-ADJACENCY: maintenance builds the
/// compacted adjacency, and every vertex's steps and degree, both ways,
/// are then the tree's; written again, the vertices written are read of
/// the tree and the others of the compacted adjacency, still the tree's
/// lists; built again, the delta is empty, and the lists are the tree's.
#[test]
fn compacted_reads_are_the_trees() {
    let method = method(Some(1_usize << 30_u32));
    let (ix, mut model) = filled(&method, 3, 2_000);
    for e in [(5, 6, 0, 1_000), (5, 6, 1, 1_000), (5, 6, 1, 1_000), (5, 6, 2, 1_000)] {
        model.insert(&ix, e);
    }
    assert!(ix.maintenance_due(), "no compacted adjacency yet");
    assert!(ix.maintain().unwrap());
    assert!(!ix.maintenance_due() && ix.delta.current().unwrap().is_some());
    assert!((0..60).all(|v| compacted(&ix, v, Direction::Out) && compacted(&ix, v, Direction::In)));
    agrees(&ix, &model, 61, "compacted");
    assert!(compacted(&ix, 5, Direction::Out));
    assert_eq!(steps(&ix, 5, Direction::Out).iter().filter(|s| s.1 == 1_000).count(), 1, "versions of one edge, one step");
    let mut state = 77_u64;
    for _ in 0_u32..300 {
        let e = random_edge(&mut state, 30, 500);
        if next(&mut state).is_multiple_of(3) { model.remove(&ix, e) } else { model.insert(&ix, e) }
    }
    model.remove(&ix, (59, 58, 0, 999));
    model.insert(&ix, (40, 41, 0, 999));
    assert!(!compacted(&ix, 40, Direction::Out) && !compacted(&ix, 41, Direction::In), "written: read of the tree");
    assert!(compacted(&ix, 41, Direction::Out) && compacted(&ix, 59, Direction::In), "not written");
    agrees(&ix, &model, 61, "compacted and the delta");
    assert!(ix.compact().unwrap());
    assert_eq!(ix.delta.marked(), 0);
    agrees(&ix, &model, 61, "compacted again");
}

/// Verifies: [DESIGN] a load builds the compacted adjacency as it loads
/// the tree, read at once, the tree's lists.
#[test]
fn a_load_builds_the_compacted_adjacency() {
    let method = method(Some(1_usize << 30_u32));
    let ix = index(&method, Kept::Both);
    let (mut model, mut state, mut all) = (Model::default(), 21_u64, Vec::new());
    let other = index(&method, Kept::Both);
    for _ in 0_u32..1_500 {
        let e = random_edge(&mut state, 40, 300);
        model.insert(&other, e);
        all.push(Ok((edge(e.0, e.1, e.2), e.3)));
    }
    let work = WorkAreas::new(MemoryBudget::new(usize::MAX));
    ix.load(&mut all.into_iter(), (&work, Area::new(1 << 16, 1 << 10))).unwrap();
    assert!(ix.delta.current().unwrap().is_some() && compacted(&ix, 0, Direction::Out));
    agrees(&ix, &model, 41, "loaded");
}

/// Verifies: [DESIGN] RULES.md D38: a budget too small for the compacted
/// adjacency leaves none, the tree read alone, still exact, and the
/// build not due again until the index holds twice its entries; a method
/// of no budget builds none, and is never due.
#[test]
fn a_budget_too_small_reads_the_tree() {
    let small = method(Some(4_096));
    let (ix, mut model) = filled(&small, 4, 1_000);
    assert!(ix.maintenance_due());
    assert!(!ix.maintain().unwrap(), "it did not fit");
    assert!(ix.delta.current().unwrap().is_none() && !ix.maintenance_due());
    agrees(&ix, &model, 61, "the tree alone");
    let mut state = 8_u64;
    for _ in 0..(1_000 + LEAST_MARKS) {
        model.insert(&ix, random_edge(&mut state, 60, 500));
    }
    assert!(ix.maintenance_due(), "twice the entries");
    let none = method(None);
    let (alone, _) = filled(&none, 4, 100);
    assert!(!alone.maintenance_due() && !alone.maintain().unwrap());
}

/// Verifies: [DESIGN] marks past the compacted adjacency's vertices and
/// `LEAST_MARKS` drop it, every list read of the tree, and a build due.
#[test]
fn marks_past_the_vertices_drop_it() {
    let method = method(Some(1_usize << 30_u32));
    let ix = index(&method, Kept::Both);
    let mut model = Model::default();
    model.insert(&ix, (0, 1, 0, 1));
    assert!(ix.compact().unwrap());
    let most = i128::try_from(LEAST_MARKS).unwrap();
    for v in 0..=most {
        model.insert(&ix, (v + 10, v + 11, 0, 2));
        assert_eq!(ix.delta.current().unwrap().is_some(), 2 * (v + 1) <= most + 2, "{v}");
    }
    assert!(ix.maintenance_due());
    agrees(&ix, &model, 20, "dropped");
}

/// Verifies: [DESIGN] RULES.md D-GRAPH-ADJACENCY: builds made while
/// writers insert and remove edges leave every list the tree's: a vertex
/// written while a build reads the tree is marked for it.
#[test]
fn builds_while_writing_keep_the_trees_lists() {
    let method = method(Some(1_usize << 30_u32));
    let (ix, model) = filled(&method, 31, 500);
    let written = thread::scope(|s| {
        let writer = s.spawn(|| {
            let (mut model, mut state) = (model.clone(), 99_u64);
            for _ in 0_u32..4_000 {
                let e = random_edge(&mut state, 60, 500);
                if next(&mut state).is_multiple_of(3) { model.remove(&ix, e) } else { model.insert(&ix, e) }
            }
            model
        });
        while !writer.is_finished() {
            ix.compact().unwrap();
        }
        writer.join().unwrap()
    });
    agrees(&ix, &written, 61, "written while built");
    assert!(ix.compact().unwrap());
    agrees(&ix, &written, 61, "built after");
}

//! A frontier's lists read at once (`steps_many`) against each read
//! alone: of the tree, of the compacted adjacency, and of both while the
//! delta marks some vertices, every way the index keeps its lists.

use super::*;
use crate::graph::adjacency::Adjacency;

/// Each of `vertices`' steps in `direction`, read alone.
fn alone(ix: &AdjacencyIndex, vertices: &[Key], direction: Direction) -> Vec<Vec<Step>> {
    vertices
        .iter()
        .map(|v| {
            let mut steps = Vec::new();
            ix.steps_of(v, direction, &mut steps).unwrap();
            steps
        })
        .collect()
}

/// Whether `steps_many` of every vertex below 61, repeated and out of
/// order, gives each its own steps, both ways.
fn agrees_at_once(ix: &AdjacencyIndex) {
    let vertices: Vec<Key> = (0_i128..61).rev().chain([7, 7, 3]).map(|v| Key(vec![n(v)])).collect();
    for direction in [Direction::Out, Direction::In] {
        let mut batch = vec![vec![Step { edge: 0, vertex: Key(Vec::new()) }]];
        Adjacency::steps_many(ix, &vertices, direction, &mut batch).unwrap();
        assert_eq!(&batch[1..], alone(ix, &vertices, direction).as_slice(), "{direction:?}");
    }
}

/// Verifies: [DESIGN] a vectorized Expand reads a frontier's lists at
/// once, each the list read alone: of the tree, of the compacted
/// adjacency, and of both once writes mark vertices in the delta; of an
/// index keeping one way's lists, the other way's too; after what `out`
/// already held.
#[test]
fn lists_read_at_once_are_each_alone() {
    for kept in [Kept::Both, Kept::Out, Kept::In] {
        let method = method(Some(1_usize << 30_u32));
        let ix = index(&method, kept);
        let (mut model, mut state) = (Model::default(), 11_u64);
        for _ in 0_u32..1_500 {
            model.insert(&ix, random_edge(&mut state, 60, 400));
        }
        agrees_at_once(&ix);
        assert!(ix.maintain().unwrap());
        agrees_at_once(&ix);
        for _ in 0_u32..40 {
            model.insert(&ix, random_edge(&mut state, 60, 400));
        }
        agrees_at_once(&ix);
    }
}

//! Unit tests of shortest paths [ISO/IEC 13249-3:2016 (DIS, SC32 N2593)
//! §13.1], on graphs built here.

use super::enumerate::MAX_PATHS;
use super::{Edge, Found, Graph, shortest};

/// The graph of `nodes` nodes and edges `(from, to, weight, id)`, each
/// both ways if `both`.
fn graph(nodes: usize, edges: &[(usize, usize, f64, i32)], both: bool) -> Graph {
    let mut g = Graph { nodes, edges: Vec::new() };
    for &(from, to, weight, id) in edges {
        g.edges.push(Edge { from, to, weight, id });
        if both {
            g.edges.push(Edge { from: to, to: from, weight, id });
        }
    }
    g
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §13.1.1 Description
/// 4, 11] the least total weight, and every path of it, in ascending
/// order; a heavier path left out.
#[test]
fn every_shortest_path_is_found() {
    // A square 0-1-2 and 0-3-2 of weight 2, and a direct 0-2 of 5.
    let g = graph(
        4,
        &[(0, 1, 1.0_f64, 10_i32), (1, 2, 1.0_f64, 11_i32), (0, 3, 1.5_f64, 12_i32), (3, 2, 0.5_f64, 13_i32), (0, 2, 5.0_f64, 14_i32)],
        true,
    );
    let found = shortest(&g, 0, 2).unwrap().unwrap();
    assert_eq!(found, Found { total: 2.0, paths: vec![vec![10_i32, 11_i32], vec![12_i32, 13_i32]] });
    let back = shortest(&g, 2, 0).unwrap().unwrap();
    assert_eq!(back.paths, [vec![11_i32, 10_i32], vec![13_i32, 12_i32]]);
    let one = shortest(&graph(3, &[(0, 1, 1.0_f64, 1_i32), (1, 2, 1.0_f64, 2_i32), (0, 2, 3.0_f64, 3_i32)], true), 0, 2).unwrap().unwrap();
    assert_eq!(one, Found { total: 2.0, paths: vec![vec![1_i32, 2_i32]] });
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §13.1.2 Description
/// 4, 10] a directed edge is walked one way only; where no path leads,
/// there is none; from a node to itself, the path of no edge.
#[test]
fn direction_and_unreachable_ends() {
    let g = graph(3, &[(0, 1, 1.0_f64, 1_i32), (1, 2, 1.0_f64, 2_i32), (2, 0, 1.0_f64, 3_i32)], false);
    assert_eq!(shortest(&g, 0, 2).unwrap().unwrap().paths, [vec![1_i32, 2_i32]]);
    assert_eq!(shortest(&g, 2, 1).unwrap().unwrap(), Found { total: 2.0, paths: vec![vec![3_i32, 1_i32]] });
    let apart = graph(4, &[(0, 1, 1.0_f64, 1_i32), (2, 3, 1.0_f64, 2_i32)], true);
    assert_eq!(shortest(&apart, 0, 3).unwrap(), None);
    assert_eq!(shortest(&apart, 0, 0).unwrap(), Some(Found { total: 0.0, paths: vec![Vec::new()] }));
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §13.1.1 Description
/// 11] a cycle of weight zero adds no path, as a path passes no node
/// twice, and parallel edges of one weight are two paths.
#[test]
fn zero_cycles_and_parallel_edges() {
    let g = graph(
        4,
        &[(0, 1, 1.0_f64, 1_i32), (1, 2, 0.0_f64, 2_i32), (2, 1, 0.0_f64, 3_i32), (2, 3, 1.0_f64, 4_i32), (1, 3, 1.0_f64, 5_i32)],
        false,
    );
    assert_eq!(shortest(&g, 0, 3).unwrap().unwrap().paths, [vec![1_i32, 2_i32, 4_i32], vec![1_i32, 5_i32]]);
    let parallel = graph(2, &[(0, 1, 2.0_f64, 7_i32), (0, 1, 2.0_f64, 8_i32), (0, 1, 3.0_f64, 9_i32)], true);
    assert_eq!(shortest(&parallel, 1, 0).unwrap().unwrap().paths, [vec![7_i32], vec![8_i32]]);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §13.1.1 Description
/// 11] the paths of a grid of equal weights are counted whole, and more
/// than `MAX_PATHS` paths is an error, not some of them.
#[test]
fn many_paths_and_the_limit() {
    // A 3 by 3 grid of nodes: 6 shortest paths across it.
    let mut edges = Vec::new();
    for r in 0..3_usize {
        for c in 0..3_usize {
            let n = r * 3 + c;
            if c < 2 {
                edges.push((n, n + 1, 1.0_f64, i32::try_from(n * 2).unwrap()));
            }
            if r < 2 {
                edges.push((n, n + 3, 1.0_f64, i32::try_from(n * 2 + 1).unwrap()));
            }
        }
    }
    let found = shortest(&graph(9, &edges, false), 0, 8).unwrap().unwrap();
    assert_eq!((found.total, found.paths.len()), (4.0_f64, 6));
    // A chain of 17 diamonds: 2^17 shortest paths.
    let mut chain = Vec::new();
    for k in 0..17_usize {
        let (a, b, c, d) = (k * 3, k * 3 + 1, k * 3 + 2, k * 3 + 3);
        let id = i32::try_from(k * 4).unwrap();
        chain.extend([(a, b, 1.0_f64, id), (b, d, 1.0_f64, id + 1_i32), (a, c, 1.0_f64, id + 2_i32), (c, d, 1.0_f64, id + 3_i32)]);
    }
    let err = shortest(&graph(52, &chain, false), 0, 51).unwrap_err();
    assert!(err.msg.contains(&MAX_PATHS.to_string()), "{}", err.msg);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §13.1.1 Description
/// 4] the least weight first: a node reached first by a heavy edge and
/// then by a lighter path is left by the lighter.
#[test]
fn the_lightest_node_is_settled_first() {
    // 0 to 1 of 1, 0 to 2 of 5, 1 to 2 of 1, 2 to 3 of 1.
    let g = graph(4, &[(0, 1, 1.0_f64, 1_i32), (0, 2, 5.0_f64, 2_i32), (1, 2, 1.0_f64, 3_i32), (2, 3, 1.0_f64, 4_i32)], false);
    assert_eq!(shortest(&g, 0, 3).unwrap(), Some(Found { total: 3.0_f64, paths: vec![vec![1_i32, 3_i32, 4_i32]] }));
}

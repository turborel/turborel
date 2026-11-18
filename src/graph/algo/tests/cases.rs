//! Hand-computed results on small graphs, on one thread and on four.

use super::*;

/// The widths each case runs on.
const WIDTHS: [usize; 2] = [1, 4];

/// Whether `a` and `b` are within 1e-9 of each other.
fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9_f64
}

/// Verifies: [EXTENSION] [GS §18] PAGERANK of a 3-cycle gives each
/// vertex 1/3; of an edge to a dangling vertex, whose rank is shared by
/// all, the fixed point r0 = 1/2.85 and r1 = 1.85/2.85, summing to 1; a
/// damping past 1 and a negative tolerance are refused with SQLCODE -301.
#[test]
fn pagerank_of_small_graphs() {
    let memory = Memory::new(PLENTY);
    let cycle = of(&memory, 3, &[(0, 1), (1, 2), (2, 0)]);
    let dangling = of(&memory, 2, &[(0, 1)]);
    for width in WIDTHS {
        let r = with(width, |c| pagerank(&cycle, &PageRank::default(), c));
        assert!(r.ranks.iter().all(|&x| near(x, 1.0_f64 / 3.0_f64)), "{r:?}");
        assert_eq!(r.iterations, 1, "the ranks do not move, so one iteration ends it");
        let p = PageRank { iterations: 200, tolerance: 0.0_f64, ..PageRank::default() };
        let d = with(width, |c| pagerank(&dangling, &p, c));
        assert!(near(d.ranks[0], 1.0_f64 / 2.85_f64) && near(d.ranks[1], 1.85_f64 / 2.85_f64), "{d:?}");
        assert!(near(d.ranks.iter().sum(), 1.0_f64));
    }
    let control = Control::new(1, &memory, &Never);
    let past = pagerank(&cycle, &PageRank { damping: 1.5_f64, ..PageRank::default() }, &control).unwrap_err();
    assert_eq!(past.sqlcode(), -301_i32);
    assert!(past.msg.contains("damping"), "{}", past.msg);
    let negative = pagerank(&cycle, &PageRank { tolerance: -1.0_f64, ..PageRank::default() }, &control).unwrap_err();
    assert_eq!(negative.sqlcode(), -301_i32);
}

/// Verifies: [EXTENSION] [GS §18] WCC labels each vertex by the least of
/// its weakly connected component, an isolated vertex by itself; SCC by
/// the least of its strongly connected component, a cycle one component
/// and the vertices after it their own.
#[test]
fn components_of_small_graphs() {
    let memory = Memory::new(PLENTY);
    let weak = of(&memory, 6, &[(0, 1), (2, 1), (4, 3)]);
    let strong = of(&memory, 7, &[(0, 1), (1, 2), (2, 0), (2, 3), (3, 4), (4, 3), (4, 6), (6, 6)]);
    for width in WIDTHS {
        assert_eq!(with(width, |c| wcc(&weak, c)), vec![0, 0, 0, 3, 3, 5]);
        assert_eq!(with(width, |c| scc(&strong, c)), vec![0, 0, 0, 3, 3, 5, 6]);
    }
}

/// Verifies: [EXTENSION] [GS §18] BFS_LEVELS gives each vertex its edges
/// from the start, `UNREACHED` of one not reached; SSSP the least cost,
/// by Dijkstra's algorithm of costs none negative and by Bellman-Ford of
/// a negative one; a negative cycle the start reaches and a cost not a
/// number are refused with SQLCODE -301 (GRF-0034), a negative cycle it
/// does not reach is not.
#[test]
fn traversals_of_small_graphs() {
    let memory = Memory::new(PLENTY);
    let levels = of(&memory, 6, &[(0, 1), (0, 2), (1, 3), (2, 3), (3, 4)]);
    let costs = weighted(&memory, 4, &[(0, 1, 4.0_f64), (0, 2, 1.0_f64), (2, 1, 2.0_f64), (1, 3, 1.0_f64)]);
    let negative = weighted(&memory, 5, &[(0, 1, 2.0_f64), (0, 2, 5.0_f64), (2, 1, -4.0_f64), (3, 4, -1.0_f64), (4, 3, -1.0_f64)]);
    let cycle = weighted(&memory, 3, &[(0, 1, 1.0_f64), (1, 2, -2.0_f64), (2, 1, 1.0_f64)]);
    let nan = weighted(&memory, 2, &[(0, 1, f64::NAN)]);
    for width in WIDTHS {
        assert_eq!(with(width, |c| bfs_levels(&levels, 0, c)), vec![0, 1, 1, 2, 3, UNREACHED]);
        assert_eq!(with(width, |c| sssp(&costs, 0, c)), vec![0.0_f64, 3.0_f64, 1.0_f64, 4.0_f64]);
        let found = with(width, |c| sssp(&negative, 0, c));
        assert_eq!(found[..3], [0.0_f64, 1.0_f64, 5.0_f64]);
        assert!(found[3].is_infinite() && found[4].is_infinite(), "{found:?}");
        let unit = with(width, |c| sssp(&levels, 0, c));
        assert_eq!(unit, vec![0.0_f64, 1.0_f64, 1.0_f64, 2.0_f64, 3.0_f64, f64::INFINITY]);
        let control = Control::new(width, &memory, &Never);
        let e = sssp(&cycle, 0, &control).unwrap_err();
        assert_eq!(e.sqlcode(), -301_i32);
        assert!(e.msg.contains("GRF-0034") && e.msg.contains("negative cycle"), "{}", e.msg);
        assert!(sssp(&nan, 0, &control).unwrap_err().msg.contains("not a number"));
    }
}

/// Verifies: [EXTENSION] [GS §18] TRIANGLE_COUNT of a 4-clique with a
/// pendant vertex gives each clique vertex 3 and the pendant 0, the
/// edges' directions and a parallel edge and a loop not counted; and
/// LOCAL_CLUSTERING 1 of the clique's, 0.5 of the vertex of four
/// neighbours and 0 of the pendant.
#[test]
fn triangles_of_small_graphs() {
    let memory = Memory::new(PLENTY);
    let g = of(&memory, 5, &[(0, 1), (1, 0), (0, 2), (2, 1), (0, 3), (3, 1), (2, 3), (3, 4), (4, 4)]);
    for width in WIDTHS {
        let (t, c) = with(width, |control| {
            let u = g.undirected(control)?;
            let t = triangle_count(&u, control)?;
            let c = local_clustering(&u, &t);
            Ok((t, c))
        });
        assert_eq!(t, vec![3, 3, 3, 3, 0]);
        assert_eq!(c, vec![1.0_f64, 1.0_f64, 1.0_f64, 0.5_f64, 0.0_f64]);
    }
}

/// Verifies: [EXTENSION] [GS §18] BETWEENNESS of a path 0 → 1 → 2 gives
/// the middle vertex 1, the pair (0, 2) through it; of a diamond, whose
/// two shortest paths from 0 to 3 pass one middle vertex each, each
/// middle 0.5; a sample as big as the graph is exact.
#[test]
fn betweenness_of_small_graphs() {
    let memory = Memory::new(PLENTY);
    let path = of(&memory, 3, &[(0, 1), (1, 2)]);
    let diamond = of(&memory, 4, &[(0, 1), (0, 2), (1, 3), (2, 3)]);
    for width in WIDTHS {
        assert_eq!(with(width, |c| betweenness(&path, &Betweenness::default(), c)), vec![0.0_f64, 1.0_f64, 0.0_f64]);
        assert_eq!(with(width, |c| betweenness(&diamond, &Betweenness::default(), c)), vec![0.0_f64, 0.5_f64, 0.5_f64, 0.0_f64]);
        let whole = Betweenness { sample: Some(4), seed: 7 };
        assert_eq!(with(width, |c| betweenness(&diamond, &whole, c)), vec![0.0_f64, 0.5_f64, 0.5_f64, 0.0_f64]);
    }
}

/// Verifies: [EXTENSION] [GS §18] LOUVAIN of two triangles joined by an
/// edge finds the two triangles, of modularity 2 (6/14 - (7/14)²) =
/// 5/14; LABEL_PROPAGATION of two triangles apart finds them, each
/// labelled by its least vertex; a resolution of 0 is refused with
/// SQLCODE -301.
#[test]
fn communities_of_small_graphs() {
    let memory = Memory::new(PLENTY);
    let joined = of(&memory, 6, &[(0, 1), (1, 2), (2, 0), (3, 4), (4, 5), (5, 3), (2, 3)]);
    let apart = of(&memory, 6, &[(0, 1), (1, 2), (2, 0), (3, 4), (4, 5), (5, 3)]);
    for width in WIDTHS {
        let found = with(width, |c| louvain(&joined.undirected(c)?, &Louvain::default(), c));
        assert_eq!(found.labels, vec![0, 0, 0, 3, 3, 3]);
        assert!(near(found.modularity, 5.0_f64 / 14.0_f64), "{found:?}");
        let (labels, _) = with(width, |c| label_propagation(&apart.undirected(c)?, &LabelPropagation::default(), c));
        assert_eq!(labels, vec![0, 0, 0, 3, 3, 3]);
    }
    let control = Control::new(1, &memory, &Never);
    let u = joined.undirected(&control).unwrap();
    let e = louvain(&u, &Louvain { resolution: 0.0_f64, ..Louvain::default() }, &control).unwrap_err();
    assert_eq!(e.sqlcode(), -301_i32);
}

/// Verifies: [EXTENSION] [GS §18] K_CORE of a 4-clique with a pendant
/// vertex and an isolated one gives the clique 3, the pendant 1 and the
/// isolated 0; of a path, each vertex 1, the middle one falling as its
/// ends are peeled.
#[test]
fn cores_of_small_graphs() {
    let memory = Memory::new(PLENTY);
    let g = of(&memory, 6, &[(0, 1), (0, 2), (0, 3), (1, 2), (1, 3), (2, 3), (3, 4)]);
    let path = of(&memory, 3, &[(0, 1), (1, 2)]);
    for width in WIDTHS {
        assert_eq!(with(width, |c| k_core(&g.undirected(c)?, c)), vec![3, 3, 3, 3, 1, 0]);
        assert_eq!(with(width, |c| k_core(&path.undirected(c)?, c)), vec![1, 1, 1]);
    }
}

/// Verifies: [EXTENSION] [GS §18] JACCARD_SIMILARITY by threshold gives
/// each pair of at least it once, its lesser vertex first; by top_k each
/// vertex's best k, the lesser vertex between equals; a threshold past 1
/// and a top_k of 0 are refused with SQLCODE -301.
#[test]
fn jaccard_of_small_graphs() {
    let memory = Memory::new(PLENTY);
    let g = of(&memory, 5, &[(0, 2), (1, 2), (0, 3), (1, 3), (1, 4)]);
    let two_thirds = 2.0_f64 / 3.0_f64;
    for width in WIDTHS {
        let kept = with(width, |c| jaccard(&g.undirected(c)?, &Jaccard { top_k: None, threshold: 0.6_f64 }, c));
        assert_eq!(kept, vec![(0, 1, two_thirds), (2, 3, 1.0_f64)]);
        let all = with(width, |c| jaccard(&g.undirected(c)?, &Jaccard::default(), c));
        assert_eq!(all.len(), 4, "(0, 1), (2, 3), (2, 4), (3, 4): {all:?}");
        let best = with(width, |c| jaccard(&g.undirected(c)?, &Jaccard { top_k: Some(1), threshold: 0.0_f64 }, c));
        assert_eq!(best, vec![(0, 1, two_thirds), (1, 0, two_thirds), (2, 3, 1.0_f64), (3, 2, 1.0_f64), (4, 2, 0.5_f64)]);
    }
    let control = Control::new(1, &memory, &Never);
    let u = g.undirected(&control).unwrap();
    assert_eq!(jaccard(&u, &Jaccard { top_k: None, threshold: 1.5_f64 }, &control).unwrap_err().sqlcode(), -301_i32);
    assert_eq!(jaccard(&u, &Jaccard { top_k: Some(0), threshold: 0.0_f64 }, &control).unwrap_err().sqlcode(), -301_i32);
}

//! Randomized cross-checks against brute force, on graphs of a fixed
//! seed: each algorithm's result is what a naive method computes from
//! the definition.

use super::*;

/// The graphs each check runs on: (vertices, edges).
const SIZES: [(usize, usize); 4] = [(12, 20), (30, 60), (40, 160), (25, 25)];

/// Each graph's seed.
const SEEDS: [u64; 3] = [1, 42, 2026];

/// `v` as a place.
fn u(v: Vertex) -> usize {
    usize::try_from(v).unwrap()
}

/// The vertex of place `i`.
fn vx(i: usize) -> Vertex {
    Vertex::try_from(i).unwrap()
}

/// A count as a float.
fn f(n: usize) -> f64 {
    f64::from(u32::try_from(n).unwrap())
}

/// Calls `check` with each seed and size and the random edges of them.
fn each(mut check: impl FnMut(usize, &[(Vertex, Vertex)])) {
    for seed in SEEDS {
        for (n, m) in SIZES {
            check(n, &random_edges(seed, n, m));
        }
    }
}

/// The reachability closure of `edges` on `n` vertices, each vertex
/// reaching itself (Warshall).
fn closure(n: usize, edges: &[(Vertex, Vertex)]) -> Vec<Vec<bool>> {
    let mut r = vec![vec![false; n]; n];
    for (v, row) in r.iter_mut().enumerate() {
        row[v] = true;
    }
    for &(s, d) in edges {
        r[u(s)][u(d)] = true;
    }
    for k in 0..n {
        for i in 0..n {
            if r[i][k] {
                let through = r[k].clone();
                for (x, y) in r[i].iter_mut().zip(through) {
                    *x = *x || y;
                }
            }
        }
    }
    r
}

/// The undirected simple adjacency of `edges`, no loops.
fn adjacency(n: usize, edges: &[(Vertex, Vertex)]) -> Vec<Vec<bool>> {
    let mut a = vec![vec![false; n]; n];
    for &(s, d) in edges.iter().filter(|(s, d)| s != d) {
        a[u(s)][u(d)] = true;
        a[u(d)][u(s)] = true;
    }
    a
}

/// Verifies: [EXTENSION] [GS §18] SCC labels two vertices alike exactly
/// when each reaches the other, by the reachability closure, each label
/// the least such vertex; WCC alike exactly when they are joined by the
/// closure of the edges both ways; on one thread and on three.
#[test]
fn components_equal_closures() {
    each(|n, edges| {
        let memory = Memory::new(PLENTY);
        let g = of(&memory, n, edges);
        let both: Vec<(Vertex, Vertex)> = edges.iter().flat_map(|&(s, d)| [(s, d), (d, s)]).collect();
        let (r, w) = (closure(n, edges), closure(n, &both));
        for width in [1, 3] {
            let strong = with(width, |c| scc(&g, c));
            let weak = with(width, |c| wcc(&g, c));
            for v in 0..n {
                let mutual = (0..n).find(|&u| r[u][v] && r[v][u]).unwrap();
                assert_eq!(u(strong[v]), mutual, "SCC of {v} on {width} threads");
                assert_eq!(u(weak[v]), (0..n).find(|&u| w[u][v]).unwrap(), "WCC of {v}");
            }
        }
    });
}

/// Verifies: [EXTENSION] [GS §18] BFS_LEVELS and unweighted SSSP give
/// each vertex its distance by Floyd and Warshall's all-pairs distances;
/// SSSP of random integer costs, none negative and some, gives its
/// distance by Bellman-Ford's relaxation of every edge n times.
#[test]
fn traversals_equal_brute_force() {
    each(|n, edges| {
        let memory = Memory::new(PLENTY);
        let g = of(&memory, n, edges);
        let mut rng = random::Random::new(u64::try_from(n).unwrap());
        let plus: Vec<(Vertex, Vertex, f64)> = edges.iter().map(|&(s, d)| (s, d, f(rng.below(9).wrapping_add(1)))).collect();
        let dag: Vec<(Vertex, Vertex, f64)> = plus.iter().map(|&(s, d, w)| if s < d { (s, d, w - 5.0_f64) } else { (s, d, w) }).collect();
        for width in [1, 4] {
            let levels = with(width, |c| bfs_levels(&g, 0, c));
            let unit: Vec<(Vertex, Vertex, f64)> = edges.iter().map(|&(s, d)| (s, d, 1.0_f64)).collect();
            let brute = relaxed(n, &unit);
            for v in 0..n {
                let expected = if brute[v].is_finite() { brute[v].to_string().parse::<u32>().unwrap() } else { UNREACHED };
                assert_eq!(levels[v], expected, "level of {v}");
            }
            assert_eq!(with(width, |c| sssp(&weighted(&memory, n, &plus), 0, c)), relaxed(n, &plus));
            let found = sssp(&weighted(&memory, n, &dag), 0, &Control::new(width, &memory, &Never));
            match found {
                Ok(costs) => assert_eq!(costs, relaxed(n, &dag)),
                Err(e) => assert!(relaxes_after(n, &dag) && e.msg.contains("GRF-0034"), "{}", e.msg),
            }
        }
    });
}

/// The least costs from 0 by relaxing every edge n times.
fn relaxed(n: usize, edges: &[(Vertex, Vertex, f64)]) -> Vec<f64> {
    let mut d = vec![f64::INFINITY; n];
    d[0] = 0.0_f64;
    for _ in 0..n {
        for &(s, t, w) in edges {
            if d[u(s)] + w < d[u(t)] {
                d[u(t)] = d[u(s)] + w;
            }
        }
    }
    d
}

/// Whether an edge still relaxes after n rounds: a negative cycle 0
/// reaches.
fn relaxes_after(n: usize, edges: &[(Vertex, Vertex, f64)]) -> bool {
    let d = relaxed(n, edges);
    edges.iter().any(|&(s, t, w)| d[u(s)] + w < d[u(t)])
}

/// Verifies: [EXTENSION] [GS §18] BETWEENNESS equals the brute-force sum
/// over ordered pairs (s, t) of σ(s, v) σ(v, t) / σ(s, t) of each v on
/// their shortest paths, the path counts and distances from a BFS of
/// each vertex, parallel edges counted as paths of their own.
#[test]
fn betweenness_equals_all_pairs_paths() {
    each(|n, edges| {
        let memory = Memory::new(PLENTY);
        let g = of(&memory, n, edges);
        let (dist, sigma) = all_pairs(&g, n);
        let mut expected = vec![0.0_f64; n];
        for s in 0..n {
            for t in (0..n).filter(|&t| t != s && dist[s][t] != u32::MAX) {
                for v in (0..n).filter(|&v| v != s && v != t) {
                    expected[v] += through((&dist, &sigma), (s, v, t));
                }
            }
        }
        for width in [1, 3] {
            let found = with(width, |c| betweenness(&g, &Betweenness::default(), c));
            for v in 0..n {
                assert!((found[v] - expected[v]).abs() < 1e-9_f64 * (1.0_f64 + expected[v]), "{v}: {} {}", found[v], expected[v]);
            }
        }
    });
}

/// The share of the shortest paths from `s` to `t` through `v`.
fn through((dist, sigma): (&[Vec<u32>], &[Vec<f64>]), (s, v, t): (usize, usize, usize)) -> f64 {
    let on = dist[s][v] != u32::MAX && dist[v][t] != u32::MAX && dist[s][v].wrapping_add(dist[v][t]) == dist[s][t];
    if on { sigma[s][v] * sigma[v][t] / sigma[s][t] } else { 0.0_f64 }
}

/// The distances and shortest path counts of every pair of `g`.
fn all_pairs(g: &Graph<'_>, n: usize) -> (Vec<Vec<u32>>, Vec<Vec<f64>>) {
    let mut dist = vec![vec![u32::MAX; n]; n];
    let mut sigma = vec![vec![0.0_f64; n]; n];
    for s in 0..n {
        dist[s][s] = 0;
        sigma[s][s] = 1.0_f64;
        let mut queue = std::collections::VecDeque::from([s]);
        while let Some(v) = queue.pop_front() {
            for &w in g.out(vx(v)) {
                let w = u(w);
                if dist[s][w] == u32::MAX {
                    dist[s][w] = dist[s][v].wrapping_add(1);
                    queue.push_back(w);
                }
                if dist[s][w] == dist[s][v].wrapping_add(1) {
                    sigma[s][w] += sigma[s][v];
                }
            }
        }
    }
    (dist, sigma)
}

/// Verifies: [EXTENSION] [GS §18] TRIANGLE_COUNT equals the triples of
/// mutually adjacent vertices of the undirected simple graph; K_CORE the
/// core numbers of peeling a least-degree vertex at a time; JACCARD the
/// index of every pair's neighbour sets.
#[test]
fn undirected_algorithms_equal_brute_force() {
    each(|n, edges| {
        let memory = Memory::new(PLENTY);
        let graph = of(&memory, n, edges);
        let adjacent = adjacency(n, edges);
        let triangles = brute_triangles(&adjacent, n);
        let pairs = brute_jaccard(&adjacent, n);
        for width in [1, 4] {
            let (found, cores, similar) = with(width, |c| {
                let und = graph.undirected(c)?;
                Ok((triangle_count(&und, c)?, k_core(&und, c)?, jaccard(&und, &Jaccard::default(), c)?))
            });
            assert_eq!(found, triangles);
            assert_eq!(cores, peeled(&adjacent, n));
            assert_eq!(similar.len(), pairs.len());
            for ((u, v, x), (bu, bv, bx)) in similar.iter().zip(&pairs) {
                assert_eq!((*u, *v), (*bu, *bv));
                assert!((x - bx).abs() < 1e-12_f64);
            }
        }
    });
}

/// Each vertex's triangles, of every triple of vertices.
fn brute_triangles(a: &[Vec<bool>], n: usize) -> Vec<u64> {
    let mut triangles = vec![0_u64; n];
    let triples = (0..n).flat_map(|i| (i.wrapping_add(1)..n).flat_map(move |j| (j.wrapping_add(1)..n).map(move |k| (i, j, k))));
    for (i, j, k) in triples.filter(|&(i, j, k)| a[i][j] && a[j][k] && a[i][k]) {
        for x in [i, j, k] {
            triangles[x] = triangles[x].wrapping_add(1);
        }
    }
    triangles
}

/// The core numbers by peeling a vertex of least degree at a time.
fn peeled(a: &[Vec<bool>], n: usize) -> Vec<u32> {
    let mut degree: Vec<u32> = (0..n).map(|v| vx(a[v].iter().filter(|&&x| x).count())).collect();
    let (mut core, mut left, mut k) = (vec![0_u32; n], vec![true; n], 0_u32);
    for _ in 0..n {
        let v = (0..n).filter(|&v| left[v]).min_by_key(|&v| (degree[v], v)).unwrap();
        k = k.max(degree[v]);
        core[v] = k;
        left[v] = false;
        for u in (0..n).filter(|&u| a[v][u] && left[u]) {
            degree[u] = degree[u].wrapping_sub(1);
        }
    }
    core
}

/// Every pair u < v sharing a neighbour, and their Jaccard index.
fn brute_jaccard(a: &[Vec<bool>], n: usize) -> Vec<(Vertex, Vertex, f64)> {
    let mut out = Vec::new();
    for u in 0..n {
        for v in u.wrapping_add(1)..n {
            let shared = (0..n).filter(|&w| a[u][w] && a[v][w]).count();
            let either = (0..n).filter(|&w| a[u][w] || a[v][w]).count();
            if shared > 0 {
                out.push((vx(u), vx(v), f(shared) / f(either)));
            }
        }
    }
    out
}

/// Verifies: [EXTENSION] [GS §18] LOUVAIN's modularity is that of its
/// communities by the definition, Σ over communities of their inner
/// weight over 2m less (their strength over 2m)², at least that of each
/// vertex alone; and LABEL_PROPAGATION's labels are each the least
/// vertex of its label.
#[test]
fn communities_equal_their_definitions() {
    each(|n, edges| {
        let memory = Memory::new(PLENTY);
        let g = of(&memory, n, edges);
        let (found, labels) = with(1, |c| {
            let u = g.undirected(c)?;
            Ok((louvain(&u, &Louvain::default(), c)?, label_propagation(&u, &LabelPropagation::default(), c)?.0))
        });
        let mut loops = vec![0.0_f64; n];
        let mut w = vec![vec![0.0_f64; n]; n];
        for &(s, d) in edges {
            if s == d {
                loops[u(s)] += 1.0_f64;
            } else {
                w[u(s)][u(d)] += 1.0_f64;
                w[u(d)][u(s)] += 1.0_f64;
            }
        }
        let strength: Vec<f64> = (0..n).map(|v| w[v].iter().sum::<f64>() + 2.0_f64 * loops[v]).collect();
        let m2: f64 = strength.iter().sum();
        let q = |label: &[Vertex]| {
            let inner: f64 =
                (0..n).map(|v| 2.0_f64 * loops[v] + (0..n).filter(|&u| label[u] == label[v]).map(|u| w[v][u]).sum::<f64>()).sum();
            let spread: f64 =
                (0..n).map(|c| (0..n).filter(|&v| u(label[v]) == c).map(|v| strength[v]).sum::<f64>()).map(|t| (t / m2).powi(2)).sum();
            inner / m2 - spread
        };
        assert!((found.modularity - q(&found.labels)).abs() < 1e-9_f64, "{} {}", found.modularity, q(&found.labels));
        let alone: Vec<Vertex> = (0..n).map(vx).collect();
        assert!(found.modularity >= q(&alone) - 1e-12_f64);
        for (v, &l) in labels.iter().enumerate() {
            assert!(u(l) <= v && labels[u(l)] == l, "{v}: {l}");
        }
    });
}

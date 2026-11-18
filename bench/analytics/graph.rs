//! How fast path search runs on a random directed graph of 100,000
//! vertices and 1,000,000 edges held in memory: BFS of ANY SHORTEST from
//! one start to every vertex, on one worker and level by level on four;
//! BiBFS between pairs of vertices; and MS-BFS from 256 starts, 64 at once,
//! to 1,000 ends, on one worker and in morsels on four (RULES.md
//! R-BENCH-1).

use std::hint::black_box;

use super::harness::{Bench, Case, note, scatter};
use turborel::graph::in_memory::InMemoryGraph;
use turborel::graph::program::{ElementTest, PathPattern, Pattern, Quantified, Selector, VarKind, Variable};
use turborel::graph::search::{PathSearch, SearchOptions};
use turborel::graph::view::{ElementId, Orientation};

/// Vertices of the graph.
const VERTICES: u64 = 100_000;
/// Edges out of each vertex.
const DEGREE: u64 = 10;
/// Pairs each run of BiBFS searches between.
const PAIRS: u64 = 200;
/// Starts of each run of MS-BFS.
const STARTS: usize = 256;
/// Ends of each run of MS-BFS.
const ENDS: u64 = 1_000;
/// Thread counts.
const THREADS: [usize; 2] = [1, 4];

/// The graph: each vertex's edges to vertices scattered over all.
fn graph() -> (InMemoryGraph, Vec<ElementId>) {
    let mut g = InMemoryGraph::new();
    let vertices: Vec<ElementId> = (0..VERTICES).map(|_| g.vertex(0, &[])).collect();
    for (n, &from) in (0_u64..).zip(&vertices) {
        for d in 0..DEGREE {
            let to = vertices[scatter(n * DEGREE + d, VERTICES) as usize];
            g.edge(1, (from, to), true, &[]);
        }
    }
    (g, vertices)
}

/// `(x)-[e]->*(y)`, ANY SHORTEST.
fn reach() -> PathPattern {
    let vertex = |var| Pattern::Vertex(ElementTest { var: Some(var), ..ElementTest::default() });
    let edge = Pattern::Edge(ElementTest::default(), Orientation::RIGHT);
    let star = Pattern::Quantified(Box::new(Quantified { body: edge, min: 0, max: None }));
    let variables = ["x", "y"].map(|name| Variable { name: name.to_owned(), kind: VarKind::Vertex }).to_vec();
    PathPattern::new(variables, Pattern::Concat(vec![vertex(0), star, vertex(1)])).select(Selector::AnyShortest)
}

pub fn workloads(bench: &Bench) {
    let (g, vertices) = graph();
    note(&format!("graph: {VERTICES} vertices, {} edges", VERTICES * DEGREE));
    let pattern = reach();
    for threads in THREADS {
        let search = PathSearch::new(&pattern, &g, SearchOptions { threads, ..SearchOptions::default() }).unwrap();
        bench.measure(Case::new("graph BFS, ANY SHORTEST to all", "paths").threads(threads), || {
            search.run(&vertices[..1], None).unwrap().fold(0, |n, p| {
                black_box(p.unwrap());
                n + 1
            })
        });
    }
    let pairs = PathSearch::new(&pattern, &g, SearchOptions::default()).unwrap();
    bench.measure(Case::new("graph BiBFS, ANY SHORTEST of a pair", "searches"), || {
        for p in 0..PAIRS {
            let (s, t) = (vertices[scatter(p, VERTICES) as usize], vertices[scatter(p + PAIRS, VERTICES) as usize]);
            black_box(pairs.run(&[s], Some(&[t])).unwrap().count());
        }
        PAIRS
    });
    let starts: Vec<ElementId> = (0..STARTS as u64).map(|s| vertices[scatter(s + 2 * PAIRS, VERTICES) as usize]).collect();
    let ends: Vec<ElementId> = (0..ENDS).map(|e| vertices[scatter(e + 3 * PAIRS + STARTS as u64, VERTICES) as usize]).collect();
    for threads in THREADS {
        let search = PathSearch::new(&pattern, &g, SearchOptions { threads, ..SearchOptions::default() }).unwrap();
        bench.measure(Case::new("graph MS-BFS, ANY SHORTEST to 1,000", "paths").threads(threads), || {
            search.run(&starts, Some(&ends)).unwrap().fold(0, |n, p| {
                black_box(p.unwrap());
                n + 1
            })
        });
    }
}

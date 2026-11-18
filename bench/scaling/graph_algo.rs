//! How fast the graph algorithm library runs (the graph standard's §18,
//! RULES.md D111): PAGERANK, WCC and TRIANGLE_COUNT on a
//! generated graph of 1,000,000 edges, on one thread and on four, each
//! rate of edges per second. The graph is a skewed one, each edge's
//! source drawn uniformly and its destination from the square of a
//! uniform draw, so a few vertices take many edges, as in real graphs
//! (RULES.md R-BENCH-1).

use super::harness::{Bench, Case};
use turborel::graph::algo::{Control, Graph, Memory, Never, PageRank, Vertex, graph, pagerank, triangle_count, wcc};

/// The graph's vertices.
const VERTICES: u64 = 100_000;
/// Its edges.
const EDGES: u64 = 1_000_000;
/// Thread counts.
const THREADS: [usize; 2] = [1, 4];
/// The budget each run holds its arrays in.
const BUDGET: usize = 1 << 31;
/// The iterations PAGERANK runs.
const ITERATIONS: u32 = 20;

/// SplitMix64, the seeded draws of the graph's edges.
fn draw(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The generated graph, of `memory`.
fn generated(memory: &Memory) -> Graph<'_> {
    let mut state = 2026_u64;
    let edges = (0..EDGES).map(|_| {
        let s = draw(&mut state) % VERTICES;
        let u = draw(&mut state) % VERTICES;
        let d = u * u / VERTICES;
        (s as Vertex, d as Vertex, 1.0_f64)
    });
    graph(memory, (VERTICES as usize, false), edges).unwrap()
}

/// Measures PAGERANK, WCC and TRIANGLE_COUNT on each width.
pub fn workloads(bench: &Bench) {
    let memory = Memory::new(BUDGET);
    let g = generated(&memory);
    for threads in THREADS {
        let control = Control::new(threads, &memory, &Never);
        bench.measure(Case::new("graph PAGERANK, 20 iterations", "edges").threads(threads), || {
            let p = PageRank { iterations: ITERATIONS, tolerance: 0.0, ..PageRank::default() };
            pagerank(&g, &p, &control).unwrap();
            EDGES * u64::from(ITERATIONS)
        });
        bench.measure(Case::new("graph WCC", "edges").threads(threads), || {
            wcc(&g, &control).unwrap();
            EDGES
        });
        bench.measure(Case::new("graph TRIANGLE_COUNT", "edges").threads(threads), || {
            triangle_count(&g.undirected(&control).unwrap(), &control).unwrap();
            EDGES
        });
    }
}

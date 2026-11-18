//! Tests of the graph algorithms [GS §18]: hand-computed results on small
//! graphs (`cases.rs`), brute-force oracles on random graphs of a fixed
//! seed (`oracles.rs`), the same result on every width, the budget and
//! the cancel (`runs.rs`).

mod cases;
mod oracles;
mod runs;

use super::*;
use crate::error::Result;

/// A budget no test graph reaches.
pub(super) const PLENTY: usize = 1 << 30;

/// The graph of `n` vertices and unweighted `edges`.
pub(super) fn of<'m>(memory: &'m Memory, n: usize, edges: &[(Vertex, Vertex)]) -> Graph<'m> {
    graph(memory, (n, false), edges.iter().map(|&(s, d)| (s, d, 1.0_f64))).unwrap()
}

/// The graph of `n` vertices and weighted `edges`.
pub(super) fn weighted<'m>(memory: &'m Memory, n: usize, edges: &[(Vertex, Vertex, f64)]) -> Graph<'m> {
    graph(memory, (n, true), edges.iter().copied()).unwrap()
}

/// Runs `f` with a control of `width` threads and a budget of plenty.
pub(super) fn with<T>(width: usize, f: impl FnOnce(&Control<'_, Never>) -> Result<T>) -> T {
    let memory = Memory::new(PLENTY);
    f(&Control::new(width, &memory, &Never)).unwrap()
}

/// A random graph of `n` vertices and `m` edges of seed `seed`, loops and
/// parallel edges among them.
pub(super) fn random_edges(seed: u64, n: usize, m: usize) -> Vec<(Vertex, Vertex)> {
    let mut r = random::Random::new(seed);
    (0..m).map(|_| (csr::vertex(r.below(n)), csr::vertex(r.below(n)))).collect()
}

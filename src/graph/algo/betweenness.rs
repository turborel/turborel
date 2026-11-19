//! \[EXTENSION\] BETWEENNESS (the graph standard's §18,
//! `GRAPH_ALGO.BETWEENNESS(g [, sample])`): each vertex's betweenness
//! centrality along directed shortest paths of edges, the sum over the
//! ordered pairs of other vertices of the share of their shortest paths
//! that pass through it, by Brandes' algorithm ("A faster algorithm for
//! betweenness centrality", J. Math. Sociol. 25, 2001): a search from
//! each source counts its shortest paths to each vertex, then gives each
//! vertex, in the order of distance from the farthest, its dependency.
//!
//! Of a sample of k sources, chosen by a seeded sequence (`random.rs`),
//! the sums are of those sources alone, scaled by n/k, the estimate of
//! Brandes and Pich ("Centrality estimation in large networks", Int. J.
//! Bifurc. Chaos 17, 2007). Sources are taken in morsels, each summing
//! its own sources in order into a vector of its own, and the morsels'
//! vectors are summed in morsel order, so every width gives the same
//! sums to the bit.

use super::control::{Cancel, Control};
use super::csr::{Graph, Vertex, ix, real, vertex};
use super::parallel::folded;
use super::random::Random;
use crate::error::Result;

/// \[DESIGN\] The sources of a morsel: each morsel holds a vector of a
/// float for each vertex, so they are few, yet enough to share out.
const SOURCES: usize = 64;

/// The arrays a search holds for each vertex: distances, path counts,
/// dependencies, the order reached and the morsel's sums.
const ARRAYS: usize = 5;

/// A vertex the search has not reached.
const UNREACHED: u32 = u32::MAX;

/// BETWEENNESS's parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Betweenness {
    /// The sources sampled, all if none or as many as the vertices.
    pub sample: Option<usize>,
    /// The seed of the sample's choice.
    pub seed: u64,
}

/// The betweenness centrality of each vertex of `g`.
///
/// # Errors
///
/// SQLCODE -918 past the budget; the cancel's.
pub fn betweenness<C: Cancel>(g: &Graph<'_>, p: &Betweenness, control: &Control<'_, C>) -> Result<Vec<f64>> {
    let n = g.vertices();
    let held = n.saturating_mul(ARRAYS).saturating_mul(control.threads()).saturating_add(n);
    let _charge = control.memory.items::<f64>(held, "BETWEENNESS's searches")?;
    let sources: Vec<Vertex> = match p.sample.filter(|&k| k < n) {
        Some(k) => {
            let mut chosen: Vec<Vertex> = Random::new(p.seed).shuffled(n).into_iter().take(k).collect();
            chosen.sort_unstable();
            chosen
        }
        None => (0..n).map(vertex).collect(),
    };
    let mut sums = vec![0.0_f64; n];
    let work = |r: std::ops::Range<usize>| {
        let mut search = Search::new(n);
        for &s in sources.get(r).unwrap_or_default() {
            search.from(g, s);
        }
        Ok(search.sums)
    };
    folded(control, (sources.len(), SOURCES), work, |part| {
        for (total, x) in sums.iter_mut().zip(part) {
            *total += x;
        }
        Ok(())
    })?;
    if !sources.is_empty() && sources.len() < n {
        let scale = real(n) / real(sources.len());
        for x in &mut sums {
            *x *= scale;
        }
    }
    Ok(sums)
}

/// One morsel's searches: their arrays, kept between sources and reset
/// where a search reached, and the morsel's sums.
struct Search {
    distance: Vec<u32>,
    paths: Vec<f64>,
    dependency: Vec<f64>,
    order: Vec<Vertex>,
    sums: Vec<f64>,
}

impl Search {
    /// The arrays of a graph of `n` vertices.
    fn new(n: usize) -> Search {
        Search {
            distance: vec![UNREACHED; n],
            paths: vec![0.0_f64; n],
            dependency: vec![0.0_f64; n],
            order: Vec::new(),
            sums: vec![0.0_f64; n],
        }
    }

    /// Adds the dependencies of source `s` to its sums.
    fn from(&mut self, g: &Graph<'_>, s: Vertex) {
        self.reach(g, s);
        for k in (0..self.order.len()).rev() {
            let Some(&w) = self.order.get(k) else { continue };
            let xw = self.depend(g, w);
            if w != s {
                fadd(&mut self.sums, w, xw);
            }
        }
        for &v in &self.order {
            set(&mut self.distance, v, UNREACHED);
            fset(&mut self.paths, v, 0.0_f64);
            fset(&mut self.dependency, v, 0.0_f64);
        }
        self.order.clear();
    }

    /// Gives each predecessor of `w` on its shortest paths its share of
    /// `w`'s dependency: `w`'s.
    fn depend(&mut self, g: &Graph<'_>, w: Vertex) -> f64 {
        let (dw, pw, xw) = (get(&self.distance, w), fget(&self.paths, w), fget(&self.dependency, w));
        for &v in g.ins(w).iter().filter(|&&v| get(&self.distance, v).saturating_add(1) == dw) {
            fadd(&mut self.dependency, v, fget(&self.paths, v) / pw * (1.0_f64 + xw));
        }
        xw
    }

    /// The breadth-first search from `s`: each vertex's distance and
    /// number of shortest paths, and the order they were reached in.
    fn reach(&mut self, g: &Graph<'_>, s: Vertex) {
        set(&mut self.distance, s, 0);
        fset(&mut self.paths, s, 1.0_f64);
        self.order.push(s);
        let mut k = 0;
        while let Some(&v) = self.order.get(k) {
            k = k.saturating_add(1);
            let (dv, pv) = (get(&self.distance, v), fget(&self.paths, v));
            for &w in g.out(v) {
                if get(&self.distance, w) == UNREACHED {
                    set(&mut self.distance, w, dv.saturating_add(1));
                    self.order.push(w);
                }
                if get(&self.distance, w) == dv.saturating_add(1) {
                    fadd(&mut self.paths, w, pv);
                }
            }
        }
    }
}

/// The distance of `v`.
fn get(of: &[u32], v: Vertex) -> u32 {
    of.get(ix(v)).copied().unwrap_or(UNREACHED)
}

/// Sets the distance of `v`.
fn set(of: &mut [u32], v: Vertex, value: u32) {
    if let Some(x) = of.get_mut(ix(v)) {
        *x = value;
    }
}

/// The float of `v`.
fn fget(of: &[f64], v: Vertex) -> f64 {
    of.get(ix(v)).copied().unwrap_or(0.0_f64)
}

/// Adds `x` to the float of `v`.
fn fadd(of: &mut [f64], v: Vertex, x: f64) {
    if let Some(y) = of.get_mut(ix(v)) {
        *y += x;
    }
}

/// Sets the float of `v`.
fn fset(of: &mut [f64], v: Vertex, value: f64) {
    if let Some(x) = of.get_mut(ix(v)) {
        *x = value;
    }
}

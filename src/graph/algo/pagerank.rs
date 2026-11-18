//! \[EXTENSION\] PAGERANK (the graph standard's §18, `GRAPH_ALGO.PAGERANK(g,
//! damping => 0.85, iterations => 20)`): the power iteration of Page,
//! Brin, Motwani and Winograd, "The PageRank Citation Ranking", 1999.
//! Each vertex starts at 1/N; each iteration gives each vertex
//! (1 - d)/N, and d times the shares its in-neighbours give, each
//! its rank over its out-degree. A vertex of no outgoing edge, dangling,
//! gives its rank to every vertex alike, so the ranks always sum to 1.
//! The iterations stop at `iterations`, or once the ranks moved by less
//! than `tolerance` in all (their L1 distance from the last).
//!
//! Each vertex pulls from its in-neighbours, so a morsel of vertices is
//! computed alone, and the dangling ranks and the distance are summed
//! by morsel and the morsels' sums in order, so every width gives the
//! same ranks to the bit.

use super::control::{Cancel, Control};
use super::csr::{Graph, ix, real, vertex};
use super::invalid;
use super::parallel::{MORSEL, morsels};
use crate::error::Result;

/// The damping factor the standard's example gives.
pub const DAMPING: f64 = 0.85_f64;
/// The iterations the standard's example gives.
pub const ITERATIONS: u32 = 20;
/// \[DESIGN\] The distance of ranks that stops the iterations early.
pub const TOLERANCE: f64 = 1e-6_f64;

/// The arrays of a vertex's floats an iteration holds: the ranks, the
/// shares they give and the next ranks.
const ARRAYS: usize = 3;

/// PAGERANK's parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageRank {
    /// The damping factor d, from 0 to 1.
    pub damping: f64,
    /// The most iterations run.
    pub iterations: u32,
    /// The L1 distance of two iterations' ranks below which it stops.
    pub tolerance: f64,
}

impl Default for PageRank {
    fn default() -> Self {
        PageRank { damping: DAMPING, iterations: ITERATIONS, tolerance: TOLERANCE }
    }
}

/// PAGERANK's result: each vertex's rank, and the iterations run.
#[derive(Debug, Clone, PartialEq)]
pub struct Ranked {
    /// The rank of each vertex.
    pub ranks: Vec<f64>,
    /// The iterations run.
    pub iterations: u32,
}

/// The ranks of `g`'s vertices.
///
/// # Errors
///
/// SQLCODE -301 if the damping is not from 0 to 1 or the tolerance is
/// negative or not a number; -918 past the budget; the cancel's.
pub fn pagerank<C: Cancel>(g: &Graph<'_>, p: &PageRank, control: &Control<'_, C>) -> Result<Ranked> {
    if !(0.0_f64..=1.0_f64).contains(&p.damping) {
        return Err(invalid("PAGERANK", "damping", "from 0 to 1"));
    }
    if p.tolerance.is_nan() || p.tolerance < 0.0_f64 {
        return Err(invalid("PAGERANK", "tolerance", "0 or more"));
    }
    let n = g.vertices();
    let _charge = control.memory.items::<f64>(n.saturating_mul(ARRAYS), "PAGERANK's ranks")?;
    let start = if n == 0 { 0.0_f64 } else { 1.0_f64 / real(n) };
    let mut ranks = vec![start; n];
    let mut run = 0;
    while run < p.iterations {
        run = run.saturating_add(1);
        let (next, moved) = step(g, p.damping, &ranks, control)?;
        ranks = next;
        if moved < p.tolerance {
            break;
        }
    }
    Ok(Ranked { ranks, iterations: run })
}

/// One iteration from `ranks`: the next ranks and their L1 distance from
/// them.
fn step<C: Cancel>(g: &Graph<'_>, damping: f64, ranks: &[f64], control: &Control<'_, C>) -> Result<(Vec<f64>, f64)> {
    let n = g.vertices();
    let shares: Vec<f64> = ranks
        .iter()
        .enumerate()
        .map(|(v, &r)| match g.out_degree(vertex(v)) {
            0 => 0.0_f64,
            d => r / real(d),
        })
        .collect();
    let dangling: f64 = morsels(control, (n, MORSEL), |r| {
        Ok(r.filter(|&v| g.out_degree(vertex(v)) == 0).map(|v| ranks.get(v).copied().unwrap_or(0.0_f64)).sum::<f64>())
    })?
    .into_iter()
    .sum();
    let nf = real(n);
    let base = (1.0_f64 - damping) / nf + damping * dangling / nf;
    let parts = morsels(control, (n, MORSEL), |r| {
        let mut next = Vec::with_capacity(r.len());
        let mut moved = 0.0_f64;
        for v in r {
            let pulled: f64 = g.ins(vertex(v)).iter().map(|&u| shares.get(ix(u)).copied().unwrap_or(0.0_f64)).sum();
            let rank = base + damping * pulled;
            moved += (rank - ranks.get(v).copied().unwrap_or(0.0_f64)).abs();
            next.push(rank);
        }
        Ok((next, moved))
    })?;
    let mut next = Vec::with_capacity(n);
    let mut moved = 0.0_f64;
    for (part, m) in parts {
        next.extend(part);
        moved += m;
    }
    Ok((next, moved))
}

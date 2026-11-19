//! \[EXTENSION\] LABEL_PROPAGATION (the graph standard's §18,
//! `GRAPH_ALGO.LABEL_PROPAGATION(g)`): communities of the graph's
//! undirected form by the label propagation of Raghavan, Albert and
//! Kumara ("Near linear time algorithm to detect community structures
//! in large-scale networks", Phys. Rev. E 76, 2007). Each vertex starts
//! labelled by itself; each iteration gives each vertex the label most
//! frequent among its neighbours and itself, the least label between
//! equals, until no label changes or after `iterations`.
//!
//! The iterations are synchronous: each reads the last one's labels
//! alone, so a morsel of vertices is computed by itself, and the least
//! label wins a tie rather than a random one, so every run and width
//! gives the same labels. Counting a vertex's own label damps the
//! two-cycle a synchronous run may fall into on a bipartite graph. Each
//! community is labelled at the end by its least vertex.

use super::control::{Cancel, Control};
use super::csr::{Undirected, Vertex, ix, vertex};
use super::parallel::{MORSEL, morsels};
use crate::error::Result;

/// \[DESIGN\] The most iterations: labels settle in a few on most graphs.
pub const ITERATIONS: u32 = 10;

/// The arrays of a vertex's labels held: the last and the next.
const ARRAYS: usize = 2;

/// LABEL_PROPAGATION's parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LabelPropagation {
    /// The most iterations.
    pub iterations: u32,
}

impl Default for LabelPropagation {
    fn default() -> Self {
        LabelPropagation { iterations: ITERATIONS }
    }
}

/// The community of each vertex of `u`, by its least vertex, and the
/// iterations run.
///
/// # Errors
///
/// SQLCODE -918 past the budget; the cancel's.
pub fn label_propagation<C: Cancel>(u: &Undirected<'_>, p: &LabelPropagation, control: &Control<'_, C>) -> Result<(Vec<Vertex>, u32)> {
    let n = u.vertices();
    let _charge = control.memory.items::<Vertex>(n.saturating_mul(ARRAYS), "LABEL_PROPAGATION's labels")?;
    let mut labels: Vec<Vertex> = (0..n).map(vertex).collect();
    let mut run = 0;
    while run < p.iterations {
        run = run.saturating_add(1);
        let parts = morsels(control, (n, MORSEL), |r| {
            let mut seen = Vec::new();
            let mut next = Vec::with_capacity(r.len());
            for v in r.map(vertex) {
                seen.clear();
                seen.extend(u.neighbours(v).iter().map(|&w| labels.get(ix(w)).copied().unwrap_or(w)));
                seen.push(labels.get(ix(v)).copied().unwrap_or(v));
                seen.sort_unstable();
                next.push(most_frequent(&seen));
            }
            Ok(next)
        })?;
        let next: Vec<Vertex> = parts.concat();
        let changed = next != labels;
        labels = next;
        if !changed {
            break;
        }
    }
    Ok((least(&labels), run))
}

/// The most frequent of the sorted `seen`, the least between equals.
fn most_frequent(seen: &[Vertex]) -> Vertex {
    let (mut best, mut most) = (Vertex::MAX, 0_usize);
    let mut k = 0;
    while let Some(&x) = seen.get(k) {
        let run = seen.get(k..).unwrap_or_default().iter().take_while(|&&y| y == x).count();
        if run > most {
            (best, most) = (x, run);
        }
        k = k.saturating_add(run.max(1));
    }
    best
}

/// Each vertex's label replaced by the least vertex of its label.
pub(crate) fn least(labels: &[Vertex]) -> Vec<Vertex> {
    let mut first = vec![Vertex::MAX; labels.len()];
    for (v, &l) in labels.iter().enumerate() {
        if let Some(f) = first.get_mut(ix(l)) {
            *f = (*f).min(vertex(v));
        }
    }
    labels.iter().map(|&l| first.get(ix(l)).copied().unwrap_or(l)).collect()
}

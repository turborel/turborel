//! \[EXTENSION\] JACCARD_SIMILARITY (the graph standard's §18,
//! `GRAPH_ALGO.JACCARD_SIMILARITY(g, ...)`): of pairs of vertices of the
//! graph's undirected form, the Jaccard index of their neighbours, the
//! neighbours they share over those of either (Jaccard, "Étude
//! comparative de la distribution florale", Bull. Soc. Vaudoise Sci.
//! Nat. 37, 1901). Pairs of no shared neighbour are of index 0, and
//! none is given.
//!
//! The standard leaves its parameters open, written `...`; they are two
//! here. `threshold` keeps the pairs of an index of at least it, each
//! pair once, its lesser vertex first; `top_k` keeps, of each vertex,
//! the k pairs of the greatest index with it first, the lesser other
//! vertex between equals, of at least the threshold too. Each vertex's
//! candidates are those two edges away, counted by sorting; a morsel of
//! vertices is computed alone and the morsels' pairs kept in order, so
//! every width gives the same pairs.

use super::control::{Cancel, Control};
use super::csr::{Undirected, Vertex, real, vertex};
use super::invalid;
use super::parallel::{MORSEL, morsels};
use crate::error::Result;

/// The bytes of a pair given.
const PAIR_BYTES: usize = size_of::<(Vertex, Vertex, f64)>();

/// JACCARD_SIMILARITY's parameters.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Jaccard {
    /// The pairs of each vertex kept, if any limit.
    pub top_k: Option<usize>,
    /// The least index of a pair kept, from 0 to 1.
    pub threshold: f64,
}

/// A pair of vertices and their index.
pub type Pair = (Vertex, Vertex, f64);

/// The pairs of `u` `p` keeps, each vertex's in order.
///
/// # Errors
///
/// SQLCODE -301 if the threshold is not from 0 to 1 or top_k is 0; -918
/// past the budget, the pairs' bytes counted; the cancel's.
pub fn jaccard<C: Cancel>(u: &Undirected<'_>, p: &Jaccard, control: &Control<'_, C>) -> Result<Vec<Pair>> {
    if !(0.0_f64..=1.0_f64).contains(&p.threshold) {
        return Err(invalid("JACCARD_SIMILARITY", "threshold", "from 0 to 1"));
    }
    if p.top_k == Some(0) {
        return Err(invalid("JACCARD_SIMILARITY", "top_k", "1 or more"));
    }
    let n = u.vertices();
    let parts = morsels(control, (n, MORSEL), |r| {
        let mut out = Vec::new();
        let mut reached = Vec::new();
        for v in r.map(vertex) {
            reached.clear();
            for &w in u.neighbours(v) {
                reached.extend(u.neighbours(w).iter().copied().filter(|&x| x != v && (p.top_k.is_some() || x > v)));
            }
            reached.sort_unstable();
            let before = out.len();
            pairs_of(u, v, &reached, p.threshold, &mut out);
            if let Some(k) = p.top_k {
                let mine = out.get_mut(before..).unwrap_or_default();
                mine.sort_by(|a, b| b.2.total_cmp(&a.2).then(a.1.cmp(&b.1)));
                out.truncate(before.saturating_add(k));
            }
        }
        let charge = control.memory.charge(out.len().saturating_mul(PAIR_BYTES), "JACCARD_SIMILARITY's pairs")?;
        Ok((out, charge))
    })?;
    let total = parts.iter().map(|(pairs, _)| pairs.len()).sum::<usize>();
    let _charge = control.memory.charge(total.saturating_mul(PAIR_BYTES), "JACCARD_SIMILARITY's pairs, together")?;
    let mut all = Vec::with_capacity(total);
    for (pairs, charge) in parts {
        all.extend(pairs);
        drop(charge);
    }
    Ok(all)
}

/// Pushes onto `out` `v`'s pair with each vertex of the sorted `reached`,
/// which holds a vertex once for each neighbour it shares with `v`, of
/// an index of at least `threshold`.
fn pairs_of(u: &Undirected<'_>, v: Vertex, reached: &[Vertex], threshold: f64, out: &mut Vec<Pair>) {
    let dv = u.degree(v);
    let mut k = 0;
    while let Some(&x) = reached.get(k) {
        let shared = reached.get(k..).unwrap_or_default().iter().take_while(|&&y| y == x).count();
        k = k.saturating_add(shared.max(1));
        let either = dv.saturating_add(u.degree(x)).saturating_sub(shared);
        let index = real(shared) / real(either);
        if index >= threshold {
            out.push((v, x, index));
        }
    }
}

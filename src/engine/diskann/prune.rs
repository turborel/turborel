//! \[DESIGN\] RobustPrune(p, V, α, R) [DiskANN, NeurIPS 2019 Algorithm 2]:
//! of the candidates V ∪ N_out(p) \ {p}, repeatedly keep the nearest p*
//! and drop each p′ with α · d(p*, p′) ≤ d(p, p′), until R are kept. With
//! α > 1 a kept edge must be α times nearer than any it shadows, which
//! keeps long edges and makes the search path logarithmic [§2.2].

use crate::error::Result;

use super::node::corrupt;
use super::{Metric, finite};

/// A candidate: its distance to p, its node, its vector's row in a
/// flat array of vectors.
#[derive(Debug, Clone, Copy)]
pub(super) struct Candidate {
    pub(super) distance: f32,
    pub(super) id: u32,
    pub(super) row: usize,
}

/// Flat vectors, `dims` each.
#[derive(Clone, Copy)]
pub(super) struct Vectors<'a> {
    pub(super) data: &'a [f32],
    pub(super) dims: usize,
}

impl<'a> Vectors<'a> {
    /// The vector of row `row`.
    pub(super) fn row(&self, row: usize) -> Result<&'a [f32]> {
        let start = row.saturating_mul(self.dims);
        self.data.get(start..start.saturating_add(self.dims)).ok_or_else(|| corrupt("a pruned candidate's vector is missing"))
    }
}

/// What a prune keeps: α and R.
#[derive(Debug, Clone, Copy)]
pub(super) struct Bound {
    pub(super) alpha: f64,
    pub(super) degree: usize,
}

/// The out-neighbours RobustPrune keeps for `p` of `candidates`, nearest
/// first; `candidates` is sorted and left so.
///
/// # Errors
///
/// SQLCODE -910 for a distance that is not finite.
pub(super) fn robust_prune(
    p: u32,
    candidates: &mut Vec<Candidate>,
    vectors: Vectors<'_>,
    bound: Bound,
    metric: &dyn Metric,
) -> Result<Vec<u32>> {
    candidates.retain(|c| c.id != p);
    candidates.sort_by(|a, b| a.distance.total_cmp(&b.distance).then(a.id.cmp(&b.id)));
    candidates.dedup_by_key(|c| c.id);
    let mut alive = vec![true; candidates.len()];
    let mut kept = Vec::with_capacity(bound.degree);
    for (at, chosen) in candidates.iter().enumerate() {
        if !alive.get(at).copied().unwrap_or(false) {
            continue;
        }
        kept.push(chosen.id);
        if kept.len() >= bound.degree {
            break;
        }
        let star = vectors.row(chosen.row)?;
        for (other, open) in candidates.iter().zip(alive.iter_mut()).skip(at.saturating_add(1)) {
            if *open {
                let between = finite(metric.distance(star, vectors.row(other.row)?), "metric")?;
                *open = bound.alpha * f64::from(between) > f64::from(other.distance);
            }
        }
    }
    Ok(kept)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l2(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum::<f32>().sqrt()
    }

    /// Verifies: [DESIGN] [DiskANN, NeurIPS 2019 Algorithm 2] On a line,
    /// α = 1 keeps only the nearest point on each side, since every
    /// farther one is shadowed; α = 2 also keeps the long edge to the
    /// point at 4, which the point at 1 shadows only at α = 1. R bounds
    /// the edges kept; p itself and repeats never appear.
    #[test]
    fn robust_prune_keeps_long_edges_only_past_alpha() {
        let data = [0.0_f32, 1.0_f32, 2.0_f32, 4.0_f32, -1.0_f32];
        let vectors = Vectors { data: &data, dims: 1 };
        let make = || -> Vec<Candidate> {
            [0_usize, 1, 2, 3, 4, 1]
                .iter()
                .map(|&row| Candidate { distance: l2(&data[0..1], &data[row..=row]), id: u32::try_from(row).unwrap(), row })
                .collect()
        };
        let metric = |a: &[f32], b: &[f32]| l2(a, b);
        let bound = |alpha, degree| Bound { alpha, degree };
        assert_eq!(robust_prune(0, &mut make(), vectors, bound(1.0_f64, 8), &metric).unwrap(), [1, 4]);
        assert_eq!(robust_prune(0, &mut make(), vectors, bound(2.0_f64, 8), &metric).unwrap(), [1, 4, 3]);
        assert_eq!(robust_prune(0, &mut make(), vectors, bound(2.0_f64, 2), &metric).unwrap(), [1, 4]);
        let nan = |_: &[f32], _: &[f32]| f32::NAN;
        let refused = robust_prune(0, &mut make(), vectors, bound(1.0_f64, 8), &nan).unwrap_err();
        assert_eq!(refused.code, crate::error::Code::Api);
    }
}

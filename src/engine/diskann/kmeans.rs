//! \[DESIGN\] Lloyd's k-means on a sample, for the merged build's
//! overlapping clusters: "we first partition a billion point dataset into
//! k clusters (with k = 40, say) using k-means, and then assign each base
//! point to the ℓ-closest centers (typically ℓ = 2 suffices)" [DiskANN,
//! NeurIPS 2019 §3.1]. Seeded, a fixed number of rounds, so a build
//! repeats; an empty cluster keeps its centre.

use crate::error::Result;

use super::random::Random;
use super::{Metric, finite};

/// Rounds of assignment and update.
const ROUNDS: usize = 10;

/// `k` centres of `sample` (flat, `dims` each), fewer if it has fewer
/// points, one after another.
///
/// # Errors
///
/// SQLCODE -910 for a distance that is not finite.
pub(super) fn kmeans(sample: &[f32], dims: usize, k: usize, metric: &dyn Metric, seed: u64) -> Result<Vec<f32>> {
    let rows: Vec<&[f32]> = sample.chunks_exact(dims.max(1)).collect();
    let mut order: Vec<usize> = (0..rows.len()).collect();
    Random::new(seed).shuffle(&mut order);
    let k = k.min(rows.len()).max(1);
    let mut centers: Vec<f32> = order.iter().take(k).filter_map(|&i| rows.get(i)).flat_map(|r| r.iter().copied()).collect();
    if centers.is_empty() {
        return Ok(centers);
    }
    let mut sums = vec![0.0_f32; centers.len()];
    let mut counts = vec![0.0_f32; k];
    for _ in 0..ROUNDS {
        sums.fill(0.0_f32);
        counts.fill(0.0_f32);
        for row in &rows {
            let Some(&nearest) = nearest_centers(row, &centers, dims, 1, metric)?.first() else { continue };
            let start = nearest.saturating_mul(dims);
            if let Some(sum) = sums.get_mut(start..start.saturating_add(dims)) {
                sum.iter_mut().zip(row.iter()).for_each(|(s, x)| *s += x);
            }
            if let Some(count) = counts.get_mut(nearest) {
                *count += 1.0_f32;
            }
        }
        for ((center, sum), &count) in centers.chunks_exact_mut(dims).zip(sums.chunks_exact(dims)).zip(&counts) {
            if count > 0.0_f32 {
                center.iter_mut().zip(sum).for_each(|(c, s)| *c = s / count);
            }
        }
    }
    Ok(centers)
}

/// The `copies` centres nearest `vector`, nearest first.
///
/// # Errors
///
/// SQLCODE -910 for a distance that is not finite.
pub(super) fn nearest_centers(vector: &[f32], centers: &[f32], dims: usize, copies: usize, metric: &dyn Metric) -> Result<Vec<usize>> {
    let mut best: Vec<(f32, usize)> = Vec::with_capacity(copies.saturating_add(1));
    for (at, center) in centers.chunks_exact(dims.max(1)).enumerate() {
        let d = finite(metric.distance(vector, center), "metric")?;
        if best.len() < copies || best.last().is_some_and(|b| d < b.0) {
            let place = best.partition_point(|b| b.0 <= d);
            best.insert(place, (d, at));
            best.truncate(copies);
        }
    }
    Ok(best.into_iter().map(|b| b.1).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] [DiskANN, NeurIPS 2019 §3.1] Two well-separated
    /// groups give one centre each, and each point's ℓ = 2 nearest centres
    /// list its own group's first.
    #[test]
    fn kmeans_finds_separated_groups() {
        let sample: Vec<f32> = (0_u32..40).flat_map(|i| if i < 20 { [0.0_f32, 0.1_f32] } else { [10.0_f32, 9.9_f32] }).collect();
        let l2 = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum::<f32>();
        let centers = kmeans(&sample, 2, 2, &l2, 5).unwrap();
        assert!(centers.chunks(2).any(|c| c[0] < 0.5_f32) && centers.chunks(2).any(|c| c[0] > 9.5_f32));
        let near = nearest_centers(&[9.0_f32, 9.0_f32], &centers, 2, 2, &l2).unwrap();
        assert_eq!(near.len(), 2);
        assert!(centers[near[0] * 2] > 5.0_f32);
    }
}

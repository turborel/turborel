//! \[DESIGN\] Seeded k-means for IVF's centroids (vector design document
//! §10.4: "centroids from k-means++ on a sample"): k-means++ seeding
//! (Arthur and Vassilvitskii, "k-means++: The Advantages of Careful
//! Seeding", SODA 2007), then Lloyd's iterations, each point to its
//! nearest centre by the index's metric and each centre to its points'
//! mean. A centre left with no point takes the point farthest from its
//! own centre, so every list stays in use.

use crate::engine::hnsw::Metric;
use crate::engine::hnsw::random::Rng;

/// \[DESIGN\] Lloyd's iterations: past ten, IVF's recall gains little,
/// as the lists' boundaries move by few points.
pub const ITERATIONS: usize = 10;

/// The index of the centre of `centres`, `dims` each, nearest `v`, and
/// its distance.
pub fn nearest(metric: &Metric, centres: &[f32], dims: usize, v: &[f32]) -> (usize, f32) {
    let mut best = (0_usize, f32::INFINITY);
    for (at, c) in centres.chunks_exact(dims).enumerate() {
        let d = metric.distance(v, c);
        if d < best.1 {
            best = (at, d);
        }
    }
    best
}

/// `k` centres of `sample`'s vectors of `dims`, by k-means++ then
/// `ITERATIONS` of Lloyd's, seeded by `seed`: fewer if the sample holds
/// fewer distinct points.
pub fn train(metric: &Metric, sample: &[f32], (dims, k): (usize, usize), seed: u64) -> Vec<f32> {
    let n = sample.len().checked_div(dims).unwrap_or(0);
    let k = k.min(n);
    if k == 0 {
        return Vec::new();
    }
    let mut rng = Rng::new(seed);
    let mut centres = seeded(metric, sample, (dims, k), &mut rng);
    let mut assigned = vec![0_usize; n];
    for _ in 0..ITERATIONS {
        let moved = assign(metric, sample, (&centres, dims), &mut assigned);
        centres = means(sample, (&centres, dims), &assigned, metric);
        if !moved {
            break;
        }
    }
    centres
}

/// k-means++: the first centre uniform, each next drawn with
/// probability in proportion to its distance from the nearest chosen.
fn seeded(metric: &Metric, sample: &[f32], (dims, k): (usize, usize), rng: &mut Rng) -> Vec<f32> {
    let n = sample.len().checked_div(dims).unwrap_or(0);
    let point = |at: usize| sample.get(at.saturating_mul(dims)..at.saturating_add(1).saturating_mul(dims)).unwrap_or_default();
    let mut centres = Vec::with_capacity(k.saturating_mul(dims));
    centres.extend_from_slice(point(rng.below(n)));
    let mut near: Vec<f64> = (0..n).map(|at| f64::from(metric.distance(point(at), &centres).max(0.0))).collect();
    while centres.len() < k.saturating_mul(dims) {
        let total: f64 = near.iter().filter(|d| d.is_finite()).sum();
        let mut target = rng.unit() * total;
        let mut pick = rng.below(n);
        if total > 0.0_f64 {
            for (at, d) in near.iter().enumerate().filter(|(_, d)| d.is_finite()) {
                target -= d;
                pick = at;
                if target <= 0.0_f64 {
                    break;
                }
            }
        }
        let start = centres.len();
        centres.extend_from_slice(point(pick));
        let chosen = centres.get(start..).unwrap_or_default();
        for (at, d) in near.iter_mut().enumerate() {
            *d = d.min(f64::from(metric.distance(point(at), chosen).max(0.0)));
        }
    }
    centres
}

/// Assigns each point its nearest centre: whether any changed.
fn assign(metric: &Metric, sample: &[f32], (centres, dims): (&[f32], usize), assigned: &mut [usize]) -> bool {
    let mut moved = false;
    for (v, a) in sample.chunks_exact(dims).zip(assigned.iter_mut()) {
        let (at, _) = nearest(metric, centres, dims, v);
        moved |= at != *a;
        *a = at;
    }
    moved
}

/// Each centre its points' mean; a centre with none, the point farthest
/// from its own centre, taken from it. Sums are `f32`s: a sample is at
/// most 100,000 points, so counts are exact and means lose only rounding.
fn means(sample: &[f32], (centres, dims): (&[f32], usize), assigned: &[usize], metric: &Metric) -> Vec<f32> {
    let k = centres.len().checked_div(dims).unwrap_or(0);
    let mut sums = vec![0.0_f32; centres.len()];
    let mut counts = vec![0.0_f32; k];
    for (v, a) in sample.chunks_exact(dims).zip(assigned) {
        if let Some(count) = counts.get_mut(*a) {
            *count += 1.0;
        }
        let sum = sums.get_mut(a.saturating_mul(dims)..a.saturating_add(1).saturating_mul(dims)).unwrap_or_default();
        sum.iter_mut().zip(v).for_each(|(s, x)| *s += x);
    }
    let mut taken = vec![false; assigned.len()];
    for (at, count) in counts.iter().enumerate() {
        let span = at.saturating_mul(dims)..at.saturating_add(1).saturating_mul(dims);
        let sum = sums.get_mut(span).unwrap_or_default();
        if *count > 0.0 {
            for s in sum.iter_mut() {
                *s /= count;
            }
        } else if let Some(far) = farthest(sample, (centres, dims), assigned, (metric, &mut taken)) {
            sum.iter_mut().zip(far).for_each(|(s, f)| *s = *f);
        }
    }
    sums
}

/// The point farthest from its centre not taken yet, now taken.
fn farthest<'a>(
    sample: &'a [f32],
    (centres, dims): (&[f32], usize),
    assigned: &[usize],
    (metric, taken): (&Metric, &mut [bool]),
) -> Option<&'a [f32]> {
    let mut best: Option<(usize, f32)> = None;
    for (at, (v, a)) in sample.chunks_exact(dims).zip(assigned).enumerate() {
        if taken.get(at).copied().unwrap_or(true) {
            continue;
        }
        let centre = centres.get(a.saturating_mul(dims)..a.saturating_add(1).saturating_mul(dims)).unwrap_or_default();
        let d = metric.distance(v, centre);
        if best.is_none_or(|(_, b)| d > b) {
            best = Some((at, d));
        }
    }
    let (at, _) = best?;
    if let Some(t) = taken.get_mut(at) {
        *t = true;
    }
    sample.get(at.saturating_mul(dims)..at.saturating_add(1).saturating_mul(dims))
}

#[cfg(test)]
mod tests;

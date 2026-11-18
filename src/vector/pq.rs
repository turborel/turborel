//! \[EXTENSION\] PQ, product quantization (design document §7.2; Jégou,
//! Douze and Schmid, "Product quantization for nearest neighbor search",
//! IEEE TPAMI 2011): the dimensions split into `m` consecutive
//! subspaces, each coded by the nearest of up to 256 centroids that
//! k-means finds there, so a code is `m` bytes (nbits 8). A query's
//! distances to every centroid are tabulated once; a code's distance is
//! then `m` lookups.

use super::kernels::{fold_f32, gap, product, square};
use super::quantizer::{
    CODEBOOK_VERSION, CodeDistance, Codebook, Decoding, Measure, PQ, Quantizer, bad_codebook, collect, finish, floats, measure, packed,
    training, word_at,
};
use super::random::Random;
use crate::error::Result;
use crate::vector::metric::VectorMetric;

/// Centroids a subspace has at most: a byte's values.
const CENTROIDS: usize = 256;
/// k-means rounds at most, as past a dozen or so assignments rarely move.
const ROUNDS: usize = 16;
/// Dimensions a code byte stands for by default: PQ_BYTES = dim / 4
/// (design document §10.4).
const DIMS_PER_BYTE: u32 = 4;
/// A codebook's header: version, dimensions, subspaces, centroids.
const HEADER: usize = 13;

/// The PQ quantizer.
#[derive(Debug, Clone, Copy)]
pub struct ProductQuantizer;

/// A trained PQ codebook.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProductCodebook {
    dims: u32,
    /// Centroids in each subspace.
    k: usize,
    /// Each subspace's first dimension and width.
    spans: Vec<(usize, usize)>,
    /// Each subspace's centroids, `k` of its width, one after another.
    centroids: Vec<Vec<f32>>,
}

/// `dims` split into `m` consecutive spans, the first ones a dimension
/// wider where `m` does not divide `dims`.
fn spans(dims: usize, m: usize) -> Vec<(usize, usize)> {
    let base = dims.checked_div(m).unwrap_or_default();
    let wider = dims.checked_rem(m).unwrap_or_default();
    let mut start = 0_usize;
    (0..m)
        .map(|s| {
            let width = base.saturating_add(usize::from(s < wider));
            let span = (start, width);
            start = start.saturating_add(width);
            span
        })
        .collect()
}

fn part(v: &[f32], (start, width): (usize, usize)) -> &[f32] {
    v.get(start..start.saturating_add(width)).unwrap_or_default()
}

fn centroid(all: &[f32], width: usize, c: usize) -> &[f32] {
    let start = c.saturating_mul(width);
    all.get(start..start.saturating_add(width)).unwrap_or_default()
}

/// The nearest of `centroids`, `width` wide, to `point`.
fn nearest(centroids: &[f32], width: usize, point: &[f32]) -> usize {
    let mut best = (0_usize, f32::INFINITY);
    for (c, cent) in centroids.chunks_exact(width.max(1)).enumerate() {
        let d = fold_f32(point, cent, square);
        if d < best.1 {
            best = (c, d);
        }
    }
    best.0
}

/// k-means over `points`, `width` wide: `k` of them, picked in a seeded
/// shuffle, then rounds of assignment and means until none moves. A
/// centroid left with no point keeps its place.
fn k_means(points: &[&[f32]], width: usize, k: usize, random: &mut Random) -> Vec<f32> {
    let mut order: Vec<usize> = (0..points.len()).collect();
    random.shuffle(&mut order);
    let mut centroids: Vec<f32> = order.iter().take(k).flat_map(|&i| points.get(i).copied().unwrap_or_default().iter().copied()).collect();
    let mut owner = vec![usize::MAX; points.len()];
    for _ in 0..ROUNDS {
        let mut moved = false;
        for (slot, point) in owner.iter_mut().zip(points) {
            let c = nearest(&centroids, width, point);
            moved |= *slot != c;
            *slot = c;
        }
        if !moved {
            break;
        }
        means(&mut centroids, width, points, &owner);
    }
    centroids
}

fn means(centroids: &mut [f32], width: usize, points: &[&[f32]], owner: &[usize]) {
    let mut sums = vec![0.0_f64; centroids.len()];
    let mut counts = vec![0_u32; centroids.len().checked_div(width.max(1)).unwrap_or_default()];
    for (point, &c) in points.iter().zip(owner) {
        if let Some(count) = counts.get_mut(c) {
            *count = count.saturating_add(1);
        }
        let start = c.saturating_mul(width);
        for (sum, x) in sums.iter_mut().skip(start).take(width).zip(point.iter()) {
            *sum += f64::from(*x);
        }
    }
    for ((cent, sum), count) in centroids.chunks_exact_mut(width.max(1)).zip(sums.chunks_exact(width.max(1))).zip(counts) {
        if count == 0 {
            continue;
        }
        for (x, s) in cent.iter_mut().zip(sum) {
            *x = super::single(s / f64::from(count));
        }
    }
}

impl Quantizer for ProductQuantizer {
    fn name(&self) -> &'static str {
        PQ
    }

    fn train(
        &self,
        sample: &mut dyn Iterator<Item = Result<Vec<f32>>>,
        dims: u32,
        code_bytes: usize,
        seed: u64,
    ) -> Result<Box<dyn Codebook>> {
        let vectors = collect(sample, dims)?;
        let width = usize::try_from(dims).unwrap_or_default();
        let m = if code_bytes == 0 {
            usize::try_from(dims.checked_div(DIMS_PER_BYTE).unwrap_or_default().max(1)).unwrap_or(1)
        } else {
            code_bytes
        };
        if m > width {
            return Err(training(&format!("PQ codes of {m} bytes need at least {m} dimensions")));
        }
        let k = vectors.len().min(CENTROIDS);
        let spans = spans(width, m);
        let mut random = Random::new(seed);
        let centroids = spans
            .iter()
            .map(|span| {
                let points: Vec<&[f32]> = vectors.iter().map(|v| part(v, *span)).collect();
                k_means(&points, span.1, k, &mut random)
            })
            .collect();
        Ok(Box::new(ProductCodebook { dims, k, spans, centroids }))
    }

    fn read(&self, bytes: &[u8]) -> Result<Box<dyn Codebook>> {
        let bad = || bad_codebook(PQ);
        let (version, dims, m, k) = (bytes.first(), word_at(bytes, 1), word_at(bytes, 5), word_at(bytes, 9));
        let (Some(&CODEBOOK_VERSION), Some(dims), Some(m), Some(k)) = (version, dims, m, k) else { return Err(bad()) };
        let (width, m, k) =
            (usize::try_from(dims).map_err(|_| bad())?, usize::try_from(m).map_err(|_| bad())?, usize::try_from(k).map_err(|_| bad())?);
        let floats_len = width.checked_mul(k).and_then(|n| n.checked_mul(4)).ok_or_else(bad)?;
        if m == 0 || m > width || k == 0 || k > CENTROIDS || bytes.len() != HEADER.saturating_add(floats_len) {
            return Err(bad());
        }
        let all = floats(bytes.get(HEADER..).unwrap_or_default());
        let spans = spans(width, m);
        let mut at = 0_usize;
        let centroids = spans
            .iter()
            .map(|(_, w)| {
                let n = w.saturating_mul(k);
                let taken = all.get(at..at.saturating_add(n)).unwrap_or_default().to_vec();
                at = at.saturating_add(n);
                taken
            })
            .collect();
        Ok(Box::new(ProductCodebook { dims, k, spans, centroids }))
    }
}

/// A query's table: for each subspace, its term with each centroid, and
/// for COSINE each centroid's squared norm.
struct Table {
    measure: Measure,
    k: usize,
    terms: Vec<f32>,
    norms: Vec<f32>,
    query_norm: f64,
}

impl CodeDistance for Table {
    fn distance(&self, code: &[u8]) -> f32 {
        let (mut sum, mut squares) = (0.0_f64, 0.0_f64);
        for (s, c) in code.iter().enumerate() {
            let at = s.saturating_mul(self.k).saturating_add(usize::from(*c));
            sum += f64::from(self.terms.get(at).copied().unwrap_or_default());
            squares += f64::from(self.norms.get(at).copied().unwrap_or_default());
        }
        finish(self.measure, sum, squares, self.query_norm)
    }
}

impl Codebook for ProductCodebook {
    fn code_bytes(&self) -> usize {
        self.spans.len()
    }

    fn dims(&self) -> u32 {
        self.dims
    }

    fn encode(&self, v: &[f32], out: &mut [u8]) {
        for ((slot, span), cents) in out.iter_mut().zip(&self.spans).zip(&self.centroids) {
            *slot = u8::try_from(nearest(cents, span.1, part(v, *span))).unwrap_or_default();
        }
    }

    fn decode(&self, code: &[u8], out: &mut Vec<f32>) {
        out.clear();
        for ((c, span), cents) in code.iter().zip(&self.spans).zip(&self.centroids) {
            out.extend_from_slice(centroid(cents, span.1, usize::from(*c)));
        }
    }

    fn table<'a>(&'a self, query: &[f32], metric: &'a dyn VectorMetric) -> Box<dyn CodeDistance + 'a> {
        let Some(m) = measure(metric) else { return Box::new(Decoding { book: self, query: packed(query), metric }) };
        let term = match m {
            Measure::Square | Measure::Euclidean => square,
            Measure::Dot | Measure::Cosine => product,
            Measure::Gap => gap,
        };
        let (mut terms, mut norms) = (Vec::new(), Vec::new());
        for (span, cents) in self.spans.iter().zip(&self.centroids) {
            let q = part(query, *span);
            for c in 0..self.k {
                let cent = centroid(cents, span.1, c);
                terms.push(fold_f32(q, cent, term));
                norms.push(fold_f32(cent, cent, product));
            }
        }
        Box::new(Table { measure: m, k: self.k, terms, norms, query_norm: super::quantizer::norm_of(query) })
    }

    fn write(&self, out: &mut Vec<u8>) {
        out.push(CODEBOOK_VERSION);
        for n in [self.dims, u32::try_from(self.spans.len()).unwrap_or_default(), u32::try_from(self.k).unwrap_or_default()] {
            out.extend_from_slice(&n.to_le_bytes());
        }
        for cents in &self.centroids {
            out.extend(cents.iter().flat_map(|x| x.to_le_bytes()));
        }
    }
}

//! A plain product quantizer for the engine's tests (Jégou, Douze and
//! Schmid, "Product Quantization for Nearest Neighbor Search", TPAMI
//! 2011): `subspaces` slices of the vector, 256 centroids each by k-means,
//! one byte a slice, and the asymmetric distance as a sum of table
//! lookups. The engine does not depend on it; the database supplies its
//! own quantizers.

use super::super::kmeans::kmeans;
use super::*;

/// Centroids of each subspace.
const CENTROIDS: usize = 256;

pub(crate) struct TestPq {
    pub(crate) subspaces: usize,
}

/// One query's distances to each subspace's centroids.
struct Table(Vec<f32>);

impl CodeTable for Table {
    fn distance(&self, code: &[u8]) -> f32 {
        self.0.as_chunks::<CENTROIDS>().0.iter().zip(code).map(|(row, &c)| row[usize::from(c)]).sum()
    }
}

struct Book {
    dims: usize,
    /// Each subspace's coordinates.
    ranges: Vec<std::ops::Range<usize>>,
    /// For each subspace, CENTROIDS centroids of its width.
    centroids: Vec<Vec<f32>>,
}

/// The coordinates of each of `subspaces` slices of `dims`, the last
/// taking the rest.
fn ranges(dims: usize, subspaces: usize) -> Vec<std::ops::Range<usize>> {
    let width = dims.checked_div(subspaces).unwrap();
    (0..subspaces)
        .map(|s| {
            let start = s.checked_mul(width).unwrap();
            let end = if s.checked_add(1) == Some(subspaces) { dims } else { start.checked_add(width).unwrap() };
            start..end
        })
        .collect()
}

impl Quantizer for TestPq {
    fn train(&self, sample: &[f32], dims: usize, seed: u64) -> Result<Arc<dyn Codes>> {
        let ranges = ranges(dims, self.subspaces.min(dims));
        let mut centroids = Vec::new();
        for (s, range) in (0_u64..).zip(&ranges) {
            let sub: Vec<f32> = sample.chunks_exact(dims).flat_map(|v| v[range.clone()].iter().copied()).collect();
            let mut centers = kmeans(&sub, range.len(), CENTROIDS, &l2, seed ^ s)?;
            let first = centers[..range.len()].to_vec();
            while centers.len() < CENTROIDS.checked_mul(range.len()).unwrap() {
                centers.extend_from_slice(&first);
            }
            centroids.push(centers);
        }
        Ok(Arc::new(Book { dims, ranges, centroids }))
    }

    fn read(&self, bytes: &[u8]) -> Result<Arc<dyn Codes>> {
        let words: Vec<u32> = bytes.as_chunks::<4>().0.iter().map(|w| u32::from_le_bytes(*w)).collect();
        let size =
            |at: usize| words.get(at).and_then(|&w| usize::try_from(w).ok()).ok_or_else(|| SqlError::new(Code::Api, "a short codebook"));
        let (dims, subspaces) = (size(0)?, size(1)?);
        let ranges = ranges(dims, subspaces);
        let mut floats = words[2..].iter().map(|&w| f32::from_bits(w));
        let centroids = ranges.iter().map(|r| floats.by_ref().take(CENTROIDS.checked_mul(r.len()).unwrap()).collect()).collect();
        Ok(Arc::new(Book { dims, ranges, centroids }))
    }
}

impl Codes for Book {
    fn code_bytes(&self) -> usize {
        self.ranges.len()
    }

    fn encode(&self, vector: &[f32], out: &mut [u8]) {
        for ((range, centroids), slot) in self.ranges.iter().zip(&self.centroids).zip(out.iter_mut()) {
            let sub = &vector[range.clone()];
            let best =
                centroids.chunks_exact(range.len()).enumerate().min_by(|a, b| l2(a.1, sub).total_cmp(&l2(b.1, sub))).map_or(0, |(i, _)| i);
            *slot = u8::try_from(best).unwrap();
        }
    }

    fn table<'a>(&'a self, query: &[f32]) -> Box<dyn CodeTable + 'a> {
        let mut table = Vec::with_capacity(self.ranges.len().checked_mul(CENTROIDS).unwrap());
        for (range, centroids) in self.ranges.iter().zip(&self.centroids) {
            let sub = &query[range.clone()];
            table.extend(centroids.chunks_exact(range.len()).map(|c| l2(c, sub)));
        }
        Box::new(Table(table))
    }

    fn write(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&u32::try_from(self.dims).unwrap().to_le_bytes());
        out.extend_from_slice(&u32::try_from(self.ranges.len()).unwrap().to_le_bytes());
        for c in self.centroids.iter().flatten() {
            out.extend_from_slice(&c.to_bits().to_le_bytes());
        }
        out
    }
}

//! Unit tests of the quantizers: training, codes, tables, bytes, and
//! recall against exact distances.

use super::*;
use crate::vector::metric::{COSINE, DOT, EUCLIDEAN, HAMMING, MANHATTAN, built_in_metrics, metric_named};
use crate::vector::quantizer::{BQ, Codebook, PQ, SQ8, built_in_quantizers, quantizer_named};

const DIMS: usize = 32;
const CLUSTERS: usize = 20;
const POINTS: usize = 600;

/// Seeded points around seeded centres.
fn clustered(seed: u64) -> Vec<Vec<f32>> {
    let mut random = Random::new(seed);
    let centres: Vec<Vec<f64>> = (0..CLUSTERS).map(|_| noise(&mut random, DIMS)).collect();
    (0..POINTS)
        .map(|i| {
            let jitter = noise(&mut random, DIMS);
            centres[i % CLUSTERS].iter().zip(jitter).map(|(c, j)| single(c * 4.0 + j * 0.3)).collect()
        })
        .collect()
}

fn train(name: &str, points: &[Vec<f32>], code_bytes: usize, seed: u64) -> Box<dyn Codebook> {
    let q = quantizer_named(&built_in_quantizers(), name).unwrap();
    let mut sample = points.iter().cloned().map(Ok);
    q.train(&mut sample, u32::try_from(DIMS).unwrap(), code_bytes, seed).unwrap()
}

fn squared(a: &[f32], b: &[f32]) -> f64 {
    a.iter().zip(b).map(|(x, y)| (f64::from(*x) - f64::from(*y)).powi(2)).sum()
}

/// The share of the exact ten nearest of each of `queries` found among
/// the `kept` nearest by the codebook's distances.
fn recall(book: &dyn Codebook, metric: &str, points: &[Vec<f32>], queries: &[Vec<f32>], kept: usize) -> f64 {
    let m = metric_named(&built_in_metrics(), metric).unwrap();
    let codes: Vec<Vec<u8>> = points
        .iter()
        .map(|p| {
            let mut code = vec![0; book.code_bytes()];
            book.encode(p, &mut code);
            code
        })
        .collect();
    let mut found = 0_usize;
    for q in queries {
        let mut exact: Vec<usize> = (0..points.len()).collect();
        exact.sort_by(|&a, &b| squared(q, &points[a]).total_cmp(&squared(q, &points[b])));
        let table = book.table(q, m.as_ref());
        let mut approx: Vec<usize> = (0..points.len()).collect();
        approx.sort_by(|&a, &b| table.distance(&codes[a]).total_cmp(&table.distance(&codes[b])));
        found = found.saturating_add(exact[..10].iter().filter(|i| approx[..kept].contains(i)).count());
    }
    f64::from(u32::try_from(found).unwrap()) / f64::from(u32::try_from(queries.len().saturating_mul(10)).unwrap())
}

/// Verifies: [EXTENSION] §7.2 PQ: with 8 subspaces of 256 centroids,
/// ranking by the per-query table then re-ranking 4·k (R7.1) finds most
/// of the exact ten nearest neighbours; the table is the decoded codes'
/// distance; the same seed trains the same codebook.
#[test]
fn product_quantization_recalls_neighbours() {
    let points = clustered(1);
    let queries: Vec<Vec<f32>> = clustered(2).into_iter().take(20).collect();
    let book = train(PQ, &points, 8, 7);
    assert_eq!(book.code_bytes(), 8);
    let r = recall(book.as_ref(), EUCLIDEAN, &points, &queries, 40);
    assert!(r >= 0.9_f64, "PQ recall {r}");
    let m = metric_named(&built_in_metrics(), metric::EUCLIDEAN_SQUARED).unwrap();
    let mut code = vec![0; 8];
    book.encode(&points[3], &mut code);
    let mut decoded = Vec::new();
    book.decode(&code, &mut decoded);
    let looked = f64::from(book.table(&queries[0], m.as_ref()).distance(&code));
    assert!((looked - squared(&queries[0], &decoded)).abs() <= 1e-3_f64 * looked.max(1.0));
    let (mut once, mut again) = (Vec::new(), Vec::new());
    book.write(&mut once);
    train(PQ, &points, 8, 7).write(&mut again);
    assert_eq!(once, again);
    let mut other = Vec::new();
    train(PQ, &points, 8, 8).write(&mut other);
    assert_ne!(once, other);
}

/// Verifies: [EXTENSION] SQ8 keeps neighbours better still, and BQ's
/// Hamming pre-ranking finds them among a wider candidate list.
#[test]
fn scalar_and_binary_quantization_recall() {
    let points = clustered(3);
    let queries: Vec<Vec<f32>> = clustered(4).into_iter().take(20).collect();
    let sq = train(SQ8, &points, 0, 0);
    assert_eq!(sq.code_bytes(), DIMS);
    assert!(recall(sq.as_ref(), EUCLIDEAN, &points, &queries, 20) >= 0.95_f64);
    let bq = train(BQ, &points, 0, 0);
    assert_eq!(bq.code_bytes(), DIMS >> 3_u32);
    assert!(recall(bq.as_ref(), EUCLIDEAN, &points, &queries, 100) >= 0.6_f64);
}

/// Verifies: [EXTENSION] every built-in codebook's bytes read back as an
/// equal codebook (the contract's write/read round trip), and bytes cut
/// short or of another version are SQLCODE -916; each measures every
/// built-in metric over FLOAT32, falling back to decoding where it has no
/// table, with finite distances.
#[test]
fn codebooks_round_trip_and_measure() {
    let points = clustered(5);
    for (name, code_bytes) in [(PQ, 4), (SQ8, 0), (BQ, 0)] {
        let quantizer = quantizer_named(&built_in_quantizers(), name).unwrap();
        let book = train(name, &points, code_bytes, 11);
        let mut bytes = Vec::new();
        book.write(&mut bytes);
        let read = quantizer.read(&bytes).unwrap();
        let mut again = Vec::new();
        read.write(&mut again);
        assert_eq!(bytes, again, "{name}");
        assert_eq!(quantizer.read(&bytes[..bytes.len() - 1]).err().unwrap().sqlcode(), -916_i32);
        let mut other = bytes.clone();
        other[0] = 2;
        assert_eq!(quantizer.read(&other).err().unwrap().sqlcode(), -916_i32);
        let mut code = vec![0; book.code_bytes()];
        book.encode(&points[0], &mut code);
        for metric in [COSINE, DOT, EUCLIDEAN, MANHATTAN, HAMMING] {
            let m = metric_named(&built_in_metrics(), metric).unwrap();
            assert!(book.table(&points[1], m.as_ref()).distance(&code).is_finite(), "{name} {metric}");
        }
    }
}

/// Verifies: [EXTENSION] training refuses an empty sample, a vector of
/// another dimension count and a code length the quantizer cannot make,
/// with SQLCODE -301.
#[test]
fn training_refuses_bad_samples() {
    let q = quantizer_named(&built_in_quantizers(), "pq").unwrap();
    let dims = u32::try_from(DIMS).unwrap();
    assert_eq!(q.train(&mut std::iter::empty(), dims, 4, 0).err().unwrap().sqlcode(), -301_i32);
    let mut short = std::iter::once(Ok(vec![0.0_f32; 3]));
    assert_eq!(q.train(&mut short, dims, 4, 0).err().unwrap().sqlcode(), -301_i32);
    let mut one = std::iter::once(Ok(vec![0.0_f32; DIMS]));
    assert_eq!(q.train(&mut one, dims, DIMS + 1, 0).err().unwrap().sqlcode(), -301_i32);
    for name in [SQ8, BQ] {
        let per_dimension = quantizer_named(&built_in_quantizers(), name).unwrap();
        let mut single_vector = std::iter::once(Ok(vec![0.0_f32; DIMS]));
        assert_eq!(per_dimension.train(&mut single_vector, dims, 3, 0).err().unwrap().sqlcode(), -301_i32);
    }
    assert!(quantizer_named(&built_in_quantizers(), "OPQ").is_none());
}

/// Verifies: [EXTENSION] §7.2 PQ's COSINE table, its centroids' squared
/// norms made once with the codebook (trained or read back), gives each
/// code the COSINE distance of the vector the code decodes to, within an
/// `f32` sum's error.
#[test]
fn product_cosine_tables_use_the_centroid_norms() {
    let points = clustered(9);
    let book = train(PQ, &points, 8, 13);
    let quantizer = quantizer_named(&built_in_quantizers(), PQ).unwrap();
    let mut bytes = Vec::new();
    book.write(&mut bytes);
    let read = quantizer.read(&bytes).unwrap();
    let m = metric_named(&built_in_metrics(), COSINE).unwrap();
    let cosine = |a: &[f32], b: &[f32]| {
        let dot: f64 = a.iter().zip(b).map(|(x, y)| f64::from(*x) * f64::from(*y)).sum();
        1.0_f64 - dot / (squared(a, &vec![0.0_f32; a.len()]).sqrt() * squared(b, &vec![0.0_f32; b.len()]).sqrt())
    };
    for codebook in [&book, &read] {
        let table = codebook.table(&points[3], m.as_ref());
        let (mut code, mut decoded) = (vec![0; codebook.code_bytes()], Vec::new());
        for p in points.iter().step_by(37) {
            codebook.encode(p, &mut code);
            codebook.decode(&code, &mut decoded);
            let gap = (f64::from(table.distance(&code)) - cosine(&points[3], &decoded)).abs();
            assert!(gap < 1e-4_f64, "{gap}");
        }
    }
}

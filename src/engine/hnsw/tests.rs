//! HNSW requirements on seeded clustered data: recall@10 against brute
//! force, persistence across reopening, deletes never returned, filtered
//! and range searches, quantized codes, and memory refused. The fixtures
//! are shared with the IVF engine's tests.

use std::sync::Arc;

use super::*;
use crate::disk::encoding::Pipeline;
use crate::disk::memory::MemoryFileSystem;
use crate::disk::pager::Pager;
use crate::hash::crc::{CRC32_ISCSI, Crc};
use crate::memory::MemoryBudget;

mod changes;
mod persistence;
mod reach;

/// A pager file of 8 KiB pages in memory, in a pool of its own.
pub(crate) fn pages(budget: &Arc<MemoryBudget>) -> (Arc<BufferPool>, FileId) {
    let fs = MemoryFileSystem::within(Arc::clone(budget), "vector index test pages");
    let pager =
        Pager::create(&fs, std::path::Path::new("/vectors"), Arc::new(Crc::new(CRC32_ISCSI).unwrap()), Pipeline::none(), 8192).unwrap();
    let pool = Arc::new(BufferPool::in_budget(budget, (8 << 20_u32, 8192), "vector index test pool").unwrap());
    let file = pool.add_pager(Arc::new(pager));
    (pool, file)
}

/// Squared Euclidean distance.
pub(crate) fn euclidean() -> Metric {
    Metric::new("EUCLIDEAN_SQUARED", Arc::new(|a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum()))
}

/// Row `i`.
pub(crate) fn row(i: usize) -> u64 {
    u64::try_from(i).unwrap()
}

/// `n` as a float, for shares.
pub(crate) fn float(n: usize) -> f64 {
    f64::from(u32::try_from(n).unwrap())
}

/// The seed of the clusters' centres, the same for data and queries.
const CENTRES: u64 = 1_000;

/// `n` vectors of `dims` in `clusters` clusters, seeded: each a uniform
/// centre in [-1, 1], the same for every seed, plus noise of about 0.1 a
/// dimension.
pub(crate) fn clustered(n: usize, dims: usize, clusters: usize, seed: u64) -> Vec<f32> {
    let draw = |rng: &mut random::Rng| f32::from(u16::try_from(rng.next_u64() >> 48_u32).unwrap()) / 65_535.0;
    let mut centre_rng = random::Rng::new(CENTRES);
    let centres: Vec<f32> = (0..clusters.wrapping_mul(dims)).map(|_| draw(&mut centre_rng) * 2.0 - 1.0_f32).collect();
    let mut rng = random::Rng::new(seed);
    let mut unit = move || draw(&mut rng);
    let mut out = Vec::with_capacity(n.wrapping_mul(dims));
    for i in 0..n {
        let c = i.wrapping_mul(7919).checked_rem(clusters).unwrap();
        for d in 0..dims {
            let noise = (unit() + unit() + unit() - 1.5) * 0.2;
            out.push(centres[c.wrapping_mul(dims).wrapping_add(d)] + noise);
        }
    }
    out
}

/// The rows of the `k` nearest of `flat` (row = position) to `q` that
/// `keep` takes, by brute force.
pub(crate) fn brute(flat: &[f32], dims: usize, q: &[f32], k: usize, keep: &dyn Fn(u64) -> bool) -> Vec<u64> {
    let metric = euclidean();
    let mut all: Vec<(f32, u64)> =
        flat.chunks_exact(dims).enumerate().map(|(i, v)| (metric.distance(q, v), row(i))).filter(|(_, r)| keep(*r)).collect();
    all.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    all.into_iter().take(k).map(|(_, r)| r).collect()
}

/// The share of `truth` that `found` holds.
pub(crate) fn recall(found: &[(f32, u64)], truth: &[u64]) -> f64 {
    let hits = truth.iter().filter(|t| found.iter().any(|(_, r)| r == *t)).count();
    float(hits) / float(truth.len().max(1))
}

/// The configuration of the tests' indexes: M 16, efConstruction 100,
/// efSearch 64.
pub(crate) fn config(dims: usize) -> Config {
    Config { dims, m: 16, ef_construction: 100, ef_search: 64, extend: false, keep_pruned: true, seed: 42, train_rows: 1_000, rerank: true }
}

/// An index of `flat` inserted one by one, rows numbered from 0.
fn built(flat: &[f32], dims: usize, quantizer: Option<Arc<dyn Quantizer>>) -> (Hnsw, Arc<MemoryBudget>) {
    let budget = MemoryBudget::new(usize::MAX);
    let (pool, file) = pages(&budget);
    let hnsw = Hnsw::create(pool, file, &config(dims), (euclidean(), quantizer), &budget).unwrap();
    for (i, v) in flat.chunks_exact(dims).enumerate() {
        hnsw.insert(v, row(i)).unwrap();
    }
    (hnsw, budget)
}

/// A search of ten rows by a query vector.
pub(crate) type Searcher<'a> = dyn Fn(&[f32]) -> Vec<(f32, u64)> + 'a;

/// Mean recall@10 of `search` over 100 seeded queries, of rows `keep` takes.
pub(crate) fn mean_recall(search: &Searcher<'_>, flat: &[f32], dims: usize, keep: &dyn Fn(u64) -> bool) -> f64 {
    let queries = clustered(100, dims, 20, 7);
    let total: f64 = queries.chunks_exact(dims).map(|q| recall(&search(q), &brute(flat, dims, q, 10, keep))).sum();
    total / 100.0
}

/// Verifies: [EXTENSION] HNSW search (Malkov and Yashunin, Algorithms 1,
/// 2, 4 and 5) finds at least 95% of the 10 true nearest neighbours of
/// clustered data at M 16 and efSearch 64; a smaller efSearch finds no more.
#[test]
fn hnsw_recall_at_10_of_clustered_data() {
    let dims = 32;
    let flat = clustered(3_000, dims, 20, 1);
    let (hnsw, _budget) = built(&flat, dims, None);
    let r = mean_recall(&|q| hnsw.nearest(q, 10, Search::default()).unwrap(), &flat, dims, &|_| true);
    assert!(r >= 0.95_f64, "recall@10 {r}");
    let low = mean_recall(&|q| hnsw.nearest(q, 10, Search { ef: Some(10), ..Search::default() }).unwrap(), &flat, dims, &|_| true);
    assert!(low <= r, "a smaller efSearch finds no more: {low} against {r}");
}

/// Verifies: [EXTENSION] results come nearest first with their distances,
/// ties by row (vector design document R9.5), a vector's own row first.
#[test]
fn hnsw_results_are_ordered_and_measured() {
    let dims = 8;
    let flat = clustered(500, dims, 5, 3);
    let (hnsw, _budget) = built(&flat, dims, None);
    let q = &flat[..dims];
    let found = hnsw.nearest(q, 10, Search::default()).unwrap();
    assert_eq!(found.len(), 10);
    assert_eq!(found[0].1, 0);
    assert!(found.windows(2).all(|w| w[0].0 <= w[1].0));
    for (d, r) in &found {
        let at = usize::try_from(*r).unwrap().wrapping_mul(dims);
        assert_eq!(d.to_bits(), euclidean().distance(q, &flat[at..at.wrapping_add(dims)]).to_bits());
    }
}

/// Verifies: [EXTENSION] a filtered search (R9.3) gives only rows its
/// filter takes, and finds at least 90% of the true nearest of those,
/// for filters keeping a half and a tenth of the rows.
#[test]
fn hnsw_filtered_recall() {
    let dims = 16;
    let flat = clustered(2_000, dims, 10, 5);
    let (hnsw, _budget) = built(&flat, dims, None);
    for share in [2_u64, 10] {
        let keep = move |r: u64| r.is_multiple_of(share);
        let search = |q: &[f32]| {
            let mut filter = keep;
            let found = hnsw.nearest(q, 10, Search { filter: Some(&mut filter), ..Search::default() }).unwrap();
            assert!(found.iter().all(|(_, r)| keep(*r)));
            found
        };
        let r = mean_recall(&search, &flat, dims, &keep);
        assert!(r >= 0.9_f64, "filtered recall {r} keeping one in {share}");
    }
}

/// Verifies: [EXTENSION] a range search (R9.1) gives only rows within its
/// radius, nearest first, and at least 95% of those brute force finds.
#[test]
fn hnsw_range_search() {
    let dims = 16;
    let flat = clustered(2_000, dims, 10, 9);
    let (hnsw, _budget) = built(&flat, dims, None);
    let metric = euclidean();
    let (mut found_all, mut truth_all) = (0_usize, 0_usize);
    for q in clustered(20, dims, 10, 11).chunks_exact(dims) {
        let radius = 0.2;
        let found = hnsw.range(q, radius, Search::default()).unwrap();
        assert!(found.iter().all(|(d, _)| *d <= radius));
        assert!(found.windows(2).all(|w| w[0].0 <= w[1].0));
        found_all = found_all.wrapping_add(found.len());
        truth_all = truth_all.wrapping_add(flat.chunks_exact(dims).filter(|v| metric.distance(q, v) <= radius).count());
    }
    assert!(truth_all > 0);
    assert!(float(found_all) >= 0.95_f64 * float(truth_all), "range found {found_all} of {truth_all}");
}

/// Verifies: [EXTENSION] a vector of the wrong length, or with a component
/// not finite, is refused with SQLCODE -910 and adds nothing.
#[test]
fn hnsw_refuses_vectors_it_cannot_measure() {
    let budget = MemoryBudget::new(usize::MAX);
    let (pool, file) = pages(&budget);
    let hnsw = Hnsw::create(pool, file, &config(4), (euclidean(), None), &budget).unwrap();
    assert_eq!(hnsw.insert(&[1.0, 2.0], 1).unwrap_err().code, Code::Api);
    assert_eq!(hnsw.insert(&[1.0, f32::NAN, 0.0, 0.0], 1).unwrap_err().code, Code::Api);
    assert_eq!(hnsw.nearest(&[1.0], 1, Search::default()).unwrap_err().code, Code::Api);
    assert_eq!(hnsw.counts().unwrap(), (0, 0));
}

/// Verifies: [EXTENSION] the graph's memory is reserved of the budget
/// (D38, R10.7): an insert the budget cannot grant fails with SQLCODE
/// -918 and adds nothing.
#[test]
fn hnsw_memory_comes_from_the_budget() {
    let budget = MemoryBudget::new(usize::MAX);
    let (pool, file) = pages(&budget);
    let hnsw = Hnsw::create(pool, file, &config(4), (euclidean(), None), &budget).unwrap();
    hnsw.insert(&[1.0, 2.0, 3.0, 4.0], 1).unwrap();
    assert!(budget.reservations().iter().any(|(what, bytes)| what.contains("HNSW") && *bytes > 0));
    budget.set_limit(budget.reserved());
    for i in 2..60_000_u16 {
        let v = [f32::from(i), 0.0, 0.0, 0.0];
        if let Err(error) = hnsw.insert(&v, u64::from(i)) {
            assert_eq!(error.code, Code::Resources);
            assert_eq!(hnsw.rows(&v).unwrap(), Vec::<u64>::new());
            return;
        }
    }
    panic!("the budget never refused");
}

/// Verifies: [DESIGN] an entry's layer comes from the index's seed and the
/// entry alone, so it is the same when the entry is replayed, and layers
/// thin out by M: about one node in M reaches layer 1.
#[test]
fn hnsw_layers_are_seeded_and_thin_out() {
    let levels = |seed: u64| -> Vec<u8> { (0..10_000_u64).map(|r| random::level_of(seed, r, 7, 16)).collect() };
    assert_eq!(levels(42), levels(42));
    assert_ne!(levels(42), levels(43));
    let upper = levels(42).iter().filter(|l| **l >= 1).count();
    assert!((450..=800).contains(&upper), "{upper} of 10000 reach layer 1");
}

/// Verifies: [DESIGN] a training sample of a graph past `SAMPLE_MOST`
/// vectors is `SAMPLE_MOST` of them, each taken once and whole, the same
/// for one seed; of fewer, every vector.
#[test]
fn a_training_sample_is_bounded_and_seeded() {
    let flat: Vec<f32> =
        (0..u32::try_from(SAMPLE_MOST).unwrap() + 10).flat_map(|i| [f32::from(u16::try_from(i % 60_000).unwrap()), 1.0]).collect();
    let picked = sample(&flat, 2, 7);
    assert_eq!(picked.len(), SAMPLE_MOST * 2);
    assert!(picked.iter().skip(1).step_by(2).all(|y| *y > 0.5));
    assert_eq!(picked, sample(&flat, 2, 7));
    assert_eq!(sample(&flat[..20], 2, 7), &flat[..20]);
}

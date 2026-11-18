//! DiskANN requirements on a seeded clustered dataset: recall@10 against
//! brute force, hop counts, persistence across reopen, tombstones and
//! consolidation, the merged build, filtered and range search, and
//! partitions. Every dataset and build is seeded (R-TEST-5).

use super::*;
use crate::disk::encoding::Pipeline;
use crate::disk::io::Durability;
use crate::disk::memory::MemoryFileSystem;
use crate::disk::pager::Pager;
use crate::hash::crc::{CRC32_ISCSI, Crc};

mod fresh;
mod merged;
mod partitions;
mod pq;
mod recall;

/// Page size of the tests' pager files.
const PAGE: u32 = 8192;

/// A pool of 8 KiB pages in memory, big enough to hold every test index.
pub(super) fn pool() -> (Arc<BufferPool>, FileId, Arc<MemoryBudget>) {
    let budget = MemoryBudget::new(usize::MAX);
    let fs = MemoryFileSystem::within(Arc::clone(&budget), "DiskANN test pages");
    let pager =
        Pager::create(&fs, std::path::Path::new("/diskann"), Arc::new(Crc::new(CRC32_ISCSI).unwrap()), Pipeline::none(), PAGE).unwrap();
    let pool = Arc::new(BufferPool::in_budget(&budget, (64 << 20_u32, usize::try_from(PAGE).unwrap()), "DiskANN test pool").unwrap());
    let file = pool.add_pager(Arc::new(pager));
    (pool, file, budget)
}

/// Squared Euclidean distance.
pub(super) fn l2(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum()
}

/// The plug-ins: squared Euclidean, and the test PQ if `pq`.
pub(super) fn plugins(pq: bool) -> Plugins {
    let quantizer: Arc<dyn Quantizer> = Arc::new(pq::TestPq { subspaces: 8 });
    Plugins { metric: Arc::new(l2), quantizer: pq.then_some(quantizer) }
}

/// `n` points and `q` queries from the same `clusters` seeded clusters.
pub(super) fn dataset(n: usize, q: usize, (dims, clusters): (usize, usize), seed: u64) -> (Vec<Vec<f32>>, Vec<Vec<f32>>) {
    let mut points = clustered(n.checked_add(q).unwrap(), dims, clusters, seed);
    let queries = points.split_off(n);
    (points, queries)
}

/// `n` points of `dims` coordinates in `clusters` seeded Gaussian-like
/// clusters (a sum of four uniforms per coordinate).
pub(super) fn clustered(n: usize, dims: usize, clusters: usize, seed: u64) -> Vec<Vec<f32>> {
    spread(n, dims, clusters, seed, 1.5_f32)
}

/// As `clustered`, each coordinate's noise `width` times the sum.
pub(super) fn spread(n: usize, dims: usize, clusters: usize, seed: u64, width: f32) -> Vec<Vec<f32>> {
    let mut random = random::Random::new(seed);
    let mut unit = move || f32::from(u16::try_from(random.next() >> 48_u32).unwrap()) / 65_536.0_f32;
    let centers: Vec<Vec<f32>> = (0..clusters).map(|_| (0..dims).map(|_| unit() * 20.0_f32 - 10.0_f32).collect()).collect();
    (0..n)
        .map(|i| {
            let center = &centers[i.checked_rem(clusters).unwrap()];
            center.iter().map(|c| c + (unit() + unit() + unit() + unit() - 2.0_f32) * width).collect()
        })
        .collect()
}

/// The `k` nearest of `points` to `query` by brute force, their indexes.
pub(super) fn truth(points: &[Vec<f32>], query: &[f32], k: usize) -> Vec<u64> {
    let mut all: Vec<(f32, u64)> = points.iter().enumerate().map(|(i, p)| (l2(p, query), u64::try_from(i).unwrap())).collect();
    all.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    all.into_iter().take(k).map(|e| e.1).collect()
}

/// Mean recall@k of `index` over `queries`, and mean hops.
pub(super) fn recall(index: &DiskAnn, points: &[Vec<f32>], queries: &[Vec<f32>], search: &Search<'_>) -> (f64, f64) {
    let (mut hit, mut hops) = (0_usize, 0_u64);
    for q in queries {
        let (found, stats) = index.search(q, search).unwrap();
        let expected = truth(points, q, search.results);
        hit = hit.checked_add(found.iter().filter(|c| expected.contains(&c.row)).count()).unwrap();
        hops = hops.checked_add(u64::from(stats.hops)).unwrap();
    }
    let total = u32::try_from(queries.len().checked_mul(search.results).unwrap()).unwrap();
    (
        f64::from(u32::try_from(hit).unwrap()) / f64::from(total),
        u32::try_from(hops).map(f64::from).unwrap() / f64::from(u32::try_from(queries.len()).unwrap()),
    )
}

/// A layout of `dims` and degree `degree`, binary32.
pub(super) fn layout(dims: u32, degree: u32) -> Layout {
    Layout { dims, degree, element: Element::Float32 }
}

/// An index of `points` loaded with rows 0.., in a fresh pool.
pub(super) fn loaded(points: &[Vec<f32>], layout: Layout, tuning: Tuning, pq: bool) -> DiskAnn {
    let (pool, file, budget) = pool();
    let index = DiskAnn::create(pool, file, layout, (tuning, plugins(pq)), budget).unwrap();
    index.load(&mut points.iter().cloned().zip(0_u64..).map(Ok)).unwrap();
    index
}

/// A test tuning: L = 64, the rest the defaults.
pub(super) fn tuning() -> Tuning {
    Tuning { build_list: 64, search_list: 64, train_at: 512, ..Tuning::default() }
}

/// A search for the 10 nearest.
pub(super) fn ten() -> Search<'static> {
    Search { results: 10, ..Search::default() }
}

/// Verifies: [EXTENSION] A new index is empty, searches find nothing, a
/// vector of other dimensions is refused with SQLCODE -910, and a layout
/// too wide for a page with SQLCODE -900.
#[test]
fn an_empty_index_finds_nothing_and_refuses_bad_shapes() {
    let (pool, file, budget) = pool();
    let index = DiskAnn::create(Arc::clone(&pool), file, layout(4, 8), (tuning(), plugins(false)), Arc::clone(&budget)).unwrap();
    assert_eq!(index.search(&[0.0_f32; 4], &ten()).unwrap().0, []);
    assert_eq!(index.insert(&[0.0_f32; 3], 1).unwrap_err().code, Code::Api);
    let wide = DiskAnn::create(pool, file, layout(4000, 64), (tuning(), plugins(false)), budget).err().unwrap();
    assert_eq!(wide.code, Code::Unsupported);
}

/// Verifies: [EXTENSION] [DiskANN, NeurIPS 2019 §3.2] The meta page,
/// chunk directories, codes and row map reopen after a checkpoint: the
/// reopened index finds what the first did, and a page of another format
/// is refused with SQLCODE -916.
#[test]
fn an_index_reopens_after_a_checkpoint() {
    let points = clustered(1500, 16, 10, 11);
    let (pool, file, budget) = pool();
    let index = DiskAnn::create(Arc::clone(&pool), file, layout(16, 24), (tuning(), plugins(true)), Arc::clone(&budget)).unwrap();
    index.load(&mut points.iter().cloned().zip(0_u64..).map(Ok)).unwrap();
    for (i, p) in points.iter().enumerate().take(50) {
        index.insert(p, 10_000 + u64::try_from(i).unwrap()).unwrap();
    }
    index.flush().unwrap();
    pool.sync(file, Durability::Full).unwrap();
    let reopened = DiskAnn::open(Arc::clone(&pool), file, index.meta(), (tuning(), plugins(true)), Arc::clone(&budget)).unwrap();
    assert_eq!(reopened.counts().live, index.counts().live);
    for q in points.iter().step_by(97) {
        assert_eq!(reopened.search(q, &ten()).unwrap().0, index.search(q, &ten()).unwrap().0);
    }
    assert_eq!(reopened.rows(&points[3]).unwrap().len(), 2);
    *pool.write(file, index.meta()).unwrap() = vec![78, 2];
    assert_eq!(DiskAnn::open(pool, file, index.meta(), (tuning(), plugins(true)), budget).err().unwrap().code, Code::Corrupt);
}

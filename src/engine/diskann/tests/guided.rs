//! A search guided by coarse codes on dense clusters, the hot-node cache
//! of an index grown by inserts, and the slack of a back edge's prune.

use super::*;
use node::Node;

/// Points of the guided recall test: dense clusters, whose points are
/// nearly equidistant from a query of their own cluster.
const N: usize = 3000;
/// Their dimensions, four to a byte of the test PQ's code.
const DIMS: u32 = 32;
/// R of the guided recall test.
const DEGREE: u32 = 32;
/// The list of the guided recall test.
const LIST: u32 = 40;

/// An index of `points` built by inserts, maintenance done as it falls
/// due, as a database's job does it.
fn grown(points: &[Vec<f32>], layout: Layout, (tuning, pq): (Tuning, bool)) -> DiskAnn {
    let (pool, file, budget) = pool();
    let index = DiskAnn::create(pool, file, layout, (tuning, plugins(pq)), budget).unwrap();
    for (row, p) in (0_u64..).zip(points) {
        index.insert(p, row).unwrap();
        while index.maintenance_due() {
            assert!(index.maintain().unwrap());
        }
    }
    index
}

/// Verifies: [EXTENSION] [DiskANN, NeurIPS 2019 §3.1, §3.5] A regression
/// test of recall. An index built by inserts and searched with a list of
/// 40, guided by codes of four dimensions a byte, finds at least 92% of
/// the true 10 nearest on dense seeded clusters (93.7% as written). Each
/// expanded node takes its full-precision distance in the list; ordered
/// by its code's distance alone, as before the fix, the list kept the
/// wrong candidates and found 89.2%.
#[test]
fn a_guided_search_reaches_its_recall_at_a_fixed_list() {
    let mut points = spread(N.checked_add(100).unwrap(), 32, 4, 21, 1.5_f32);
    let queries = points.split_off(N);
    let index = grown(&points, layout(DIMS, DEGREE), (Tuning { train_at: 1024, ..tuning() }, true));
    assert!(read(&index.inner.book).unwrap().codes.is_some());
    let (rate, _) = recall(&index, &points, &queries, &Search { list: Some(LIST), ..ten() });
    assert!(rate >= 0.92_f64, "guided recall@10 at L = {LIST}: {rate}");
}

/// Verifies: [EXTENSION] [DiskANN, NeurIPS 2019 §3.4] An index grown by
/// inserts makes its hot-node cache again as maintenance once its live
/// nodes double, so its searches take their first hops from the cache;
/// before the fix only opening, loading or consolidating made it, and
/// such an index cached nothing. It has no codes, so no training makes
/// the cache in its stead.
#[test]
fn an_index_grown_by_inserts_caches_its_first_hops() {
    let (points, queries) = dataset(1000, 5, (16, 10), 22);
    let index = grown(&points, layout(16, 24), (Tuning { cache_percent: 5, ..tuning() }, false));
    assert!(index.counts().cached >= 25, "{:?}", index.counts());
    let (_, stats) = index.search(&queries[0], &ten()).unwrap();
    assert!(stats.cached > 0, "{stats:?}");
    assert!(!index.maintenance_due());
}

/// Verifies: [DESIGN] A back edge that takes a full list past R prunes
/// it to R × 10 / 13, leaving room for the next back edges with no
/// prune; before the fix it pruned to R, and pruned again a few back
/// edges later. The points are the origin and unit vectors, none of
/// which shadows another at α = 1.2, so the prune keeps as many as it may.
#[test]
fn a_back_edge_past_r_prunes_with_slack() {
    let (pool, file, budget) = pool();
    let index = DiskAnn::create(pool, file, layout(10, 8), (tuning(), plugins(false)), budget).unwrap();
    let unit = |i: usize| -> Vec<f32> { (0..10).map(|d| if d == i { 1.0_f32 } else { 0.0_f32 }).collect() };
    index.insert(&[0.0_f32; 10], 0).unwrap();
    for i in 0..9_usize {
        index.insert(&unit(i), u64::try_from(i).unwrap().checked_add(1).unwrap()).unwrap();
    }
    let mut node = Node::default();
    index.read_node(0, &mut node).unwrap();
    let full: Vec<u32> = (1..=8).collect();
    assert!(index.replace_edges(0, &node.neighbours, &full).unwrap());
    index.link(0, 9, &unit(8)).unwrap();
    index.read_node(0, &mut node).unwrap();
    assert_eq!(node.neighbours.len(), 6, "{:?}", node.neighbours);
}

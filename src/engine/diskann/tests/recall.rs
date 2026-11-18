//! Recall, hops, the cache, filtered and range search of a loaded index.

use super::*;

/// Points of the recall tests.
const N: usize = 2000;
/// Their dimensions.
const DIMS: u32 = 16;
/// R of the recall tests.
const DEGREE: u32 = 24;

fn data() -> (Vec<Vec<f32>>, Vec<Vec<f32>>) {
    dataset(N, 100, (16, 20), 1)
}

/// Verifies: [EXTENSION] [DiskANN, NeurIPS 2019 §2.3, §3.3, §3.5] An
/// index built by two Vamana passes and searched by BeamSearch guided by
/// PQ codes, re-ranked by full vectors, finds at least 95% of the true 10
/// nearest; guided by full vectors, at least 97%.
#[test]
fn two_pass_vamana_with_codes_reaches_its_recall() {
    let (points, queries) = data();
    let coded = loaded(&points, layout(DIMS, DEGREE), tuning(), true);
    let (with_codes, _) = recall(&coded, &points, &queries, &ten());
    assert!(with_codes >= 0.95_f64, "recall@10 with codes {with_codes}");
    let full = loaded(&points, layout(DIMS, DEGREE), tuning(), false);
    let (without, _) = recall(&full, &points, &queries, &ten());
    assert!(without >= 0.97_f64, "recall@10 with full vectors {without}");
}

/// Verifies: [EXTENSION] [DiskANN, NeurIPS 2019 §2.2, §3.3] α > 1 keeps
/// long edges, so a search needs fewer hops than on an α = 1 graph; and a
/// beam of W = 4 needs fewer rounds of reads than W = 1, which is greedy
/// search.
#[test]
fn alpha_and_beam_width_cut_hops() {
    let (points, queries) = data();
    let one = loaded(&points, layout(DIMS, DEGREE), Tuning { alpha: 1.0, passes: 1, ..tuning() }, false);
    let robust = loaded(&points, layout(DIMS, DEGREE), tuning(), false);
    let (_, hops_one) = recall(&one, &points, &queries, &ten());
    let (_, hops_robust) = recall(&robust, &points, &queries, &ten());
    assert!(hops_robust < hops_one, "α=1.2 {hops_robust} hops, α=1 {hops_one}");
    let greedy = Search { width: Some(1), ..ten() };
    let (_, hops_greedy) = recall(&robust, &points, &queries, &greedy);
    assert!(hops_robust * 2.0_f64 < hops_greedy, "W=4 {hops_robust} hops, W=1 {hops_greedy}");
}

/// Verifies: [EXTENSION] [DiskANN, NeurIPS 2019 §3.4] The nodes within C
/// hops of the medoid are cached, capped at the share asked; searches
/// take them from the cache; C = 0 caches nothing.
#[test]
fn the_hot_node_cache_serves_the_first_hops() {
    let (points, queries) = data();
    let index = loaded(&points, layout(DIMS, DEGREE), Tuning { cache_percent: 5, ..tuning() }, true);
    assert_eq!(index.counts().cached, N.checked_mul(5).unwrap().checked_div(100).unwrap());
    let (_, stats) = index.search(&queries[0], &ten()).unwrap();
    assert!(stats.cached > 0 && stats.reads > 0, "{stats:?}");
    index.retune(Tuning { cache_hops: 0, ..tuning() }).unwrap();
    assert_eq!(index.counts().cached, 0);
}

/// Verifies: [EXTENSION] Filtered search returns only rows the filter
/// accepts, traversing the others, and with a filter of 10% selectivity
/// still finds at least 90% of the true 10 nearest accepted rows.
#[test]
fn filtered_search_returns_only_accepted_rows() {
    let (points, queries) = data();
    let index = loaded(&points, layout(DIMS, DEGREE), tuning(), true);
    let accept = |row: u64| row.checked_rem(10) == Some(3);
    let kept: Vec<(u64, Vec<f32>)> = points.iter().cloned().zip(0_u64..).filter(|(_, r)| accept(*r)).map(|(p, r)| (r, p)).collect();
    let (mut hit, mut total) = (0_usize, 0_usize);
    for search in [Search { filter: Some(&accept), ..ten() }, Search { filter: Some(&accept), selectivity: Some(10), ..ten() }] {
        for q in &queries {
            let (found, _) = index.search(q, &search).unwrap();
            assert!(found.iter().all(|c| accept(c.row)));
            let mut all: Vec<(f32, u64)> = kept.iter().map(|(r, p)| (l2(p, q), *r)).collect();
            all.sort_by(|a, b| a.0.total_cmp(&b.0));
            hit = found.iter().filter(|c| all.iter().take(10).any(|e| e.1 == c.row)).count().checked_add(hit).unwrap();
            total = total.checked_add(10).unwrap();
        }
    }
    let rate = f64::from(u32::try_from(hit).unwrap()) / f64::from(u32::try_from(total).unwrap());
    assert!(rate >= 0.9_f64, "filtered recall {rate}");
}

/// Verifies: [EXTENSION] A range search returns nodes within the radius
/// only, nearest first, and at least 95% of those brute force finds,
/// growing its list past the default when more than L are within it.
#[test]
fn range_search_returns_the_nodes_within_the_radius() {
    let (points, queries) = data();
    let index = loaded(&points, layout(DIMS, DEGREE), tuning(), true);
    let (mut hit, mut total, mut grew) = (0_usize, 0_usize, false);
    for q in queries.iter().take(30) {
        let radius = f64::from(l2(&points[usize::try_from(truth(&points, q, 150)[149]).unwrap()], q));
        let (found, stats) = index.search(q, &Search { radius: Some(radius), ..ten() }).unwrap();
        assert!(found.iter().all(|c| f64::from(c.distance) <= radius));
        assert!(found.windows(2).all(|w| w[0].distance <= w[1].distance));
        grew |= stats.rounds > 1;
        let inside = points.iter().filter(|p| f64::from(l2(p, q)) <= radius).count();
        hit = hit.checked_add(found.len()).unwrap();
        total = total.checked_add(inside).unwrap();
    }
    let rate = f64::from(u32::try_from(hit).unwrap()) / f64::from(u32::try_from(total).unwrap());
    assert!(rate >= 0.95_f64 && grew, "range recall {rate}");
}

/// Verifies: [EXTENSION] FLOAT16 node vectors halve the vector's bytes
/// and still reach 95% recall@10, the re-rank by the rounded vectors.
#[test]
fn half_precision_nodes_keep_recall() {
    let (points, queries) = data();
    let half = Layout { element: Element::Float16, ..layout(DIMS, DEGREE) };
    let index = loaded(&points, half, tuning(), true);
    let (rate, _) = recall(&index, &points, &queries, &ten());
    assert!(rate >= 0.95_f64, "FLOAT16 recall@10 {rate}");
    assert_eq!(index.inner.shape.vector, 32);
}

//! The merged build past the build memory [DiskANN, NeurIPS 2019 §3.1].

use super::*;
use node::Node;

/// Verifies: [EXTENSION] [DiskANN, NeurIPS 2019 §3.1, §4.3] A build past
/// its memory clusters the points by k-means, sends each to ℓ = 2
/// clusters, builds each, and unites the edges: every node keeps at most
/// R edges, and recall@10 is within 2 points of the one-shot build's, on
/// clusters that overlap as real data's do. Clusters that do not overlap
/// at all are still connected (`merged.rs`, `stitch`).
#[test]
fn a_merged_build_is_within_a_margin_of_one_shot() {
    let mut points = spread(2080, 16, 20, 8, 5.0_f32);
    let queries = points.split_off(2000);
    let one_shot = loaded(&points, layout(16, 32), tuning(), true);
    let small = Tuning { build_memory: 64 << 10, clusters: 4, ..tuning() };
    let merged = loaded(&points, layout(16, 32), small, true);
    let (whole, _) = recall(&one_shot, &points, &queries, &ten());
    let (parts, _) = recall(&merged, &points, &queries, &ten());
    assert!(parts >= whole - 0.02_f64, "merged {parts}, one-shot {whole}");
    let mut node = Node::default();
    let mut degrees = 0_usize;
    for id in 0..2000 {
        merged.read_node(id, &mut node).unwrap();
        assert!(node.neighbours.len() <= 32 && node.live());
        degrees = degrees.checked_add(node.neighbours.len()).unwrap();
    }
    assert!(degrees > 24_000, "{degrees} edges");
}

/// Verifies: [EXTENSION] Clusters far apart, which the merged build's
/// overlapping clusters do not connect, are joined by linking each part
/// the medoid cannot reach, so every node is reachable and recall@10
/// stays at 70% even with clusters of a few dozen points.
#[test]
fn a_merged_build_reaches_every_node() {
    let (points, queries) = dataset(2000, 80, (16, 20), 8);
    let merged = loaded(&points, layout(16, 32), Tuning { build_memory: 16 << 10, ..tuning() }, true);
    let medoid = merged.inner.medoid.load(Ordering::Acquire);
    let mut reached = vec![false; 2000];
    merged.reach(medoid, &mut reached).unwrap();
    assert!(reached.iter().all(|&r| r));
    let (rate, _) = recall(&merged, &points, &queries, &Search { list: Some(200), ..ten() });
    assert!(rate >= 0.7_f64, "far-apart merged recall {rate}");
}

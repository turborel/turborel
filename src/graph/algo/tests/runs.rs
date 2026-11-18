//! Runs on many threads, past the budget and cancelled.

use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::error::{Code, SqlError};

/// The vertices of the graph that spans several morsels.
const VERTICES: usize = 5_000;
/// Its edges.
const EDGES: usize = 20_000;

/// Every algorithm's result on `g` at `width`, as text: floats by their
/// bits, so equal text is equal to the bit.
fn everything(g: &Graph<'_>, width: usize) -> Vec<String> {
    with(width, |c| {
        let u = g.undirected(c)?;
        let bits = |xs: &[f64]| xs.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
        let t = triangle_count(&u, c)?;
        let found = louvain(&u, &Louvain { seed: 9, ..Louvain::default() }, c)?;
        Ok(vec![
            format!("{:?}", bits(&pagerank(g, &PageRank::default(), c)?.ranks)),
            format!("{:?}", wcc(g, c)?),
            format!("{:?}", scc(g, c)?),
            format!("{:?}", bfs_levels(g, 3, c)?),
            format!("{:?}", bits(&sssp(g, 3, c)?)),
            format!("{t:?}"),
            format!("{:?}", bits(&local_clustering(&u, &t))),
            format!("{:?}", bits(&betweenness(g, &Betweenness { sample: Some(300), seed: 5 }, c)?)),
            format!("{:?} {}", found.labels, found.modularity.to_bits()),
            format!("{:?}", label_propagation(&u, &LabelPropagation::default(), c)?),
            format!("{:?}", k_core(&u, c)?),
            format!("{:?}", jaccard(&u, &Jaccard { top_k: Some(3), threshold: 0.0_f64 }, c)?.len()),
        ])
    })
}

/// Verifies: [EXTENSION] [GS §17, §18 R18.3] every algorithm gives the
/// same result on one, two and eight threads, its floats to the bit, on
/// a graph of several morsels.
#[test]
fn every_width_gives_the_same_results() {
    let memory = Memory::new(PLENTY);
    let g = of(&memory, VERTICES, &random_edges(11, VERTICES, EDGES));
    let one = everything(&g, 1);
    for width in [2, 8] {
        for (k, (a, b)) in one.iter().zip(everything(&g, width)).enumerate() {
            assert_eq!(*a, b, "algorithm {k} on {width} threads");
        }
    }
}

/// Verifies: [EXTENSION] [GS §18] a graph or a run past its budget fails
/// with SQLCODE -918 before it allocates, and the bytes are given back
/// as a run ends; an edge of no vertex of the graph is refused with
/// SQLCODE -910.
#[test]
fn runs_past_the_budget_fail_cleanly() {
    let small = Memory::new(1_000);
    let edges = random_edges(3, 100, 400);
    let built = graph(&small, (100, false), edges.iter().map(|&(s, d)| (s, d, 1.0_f64)));
    assert_eq!(built.unwrap_err().code, Code::Resources);
    let memory = Memory::new(60_000);
    let g = of(&memory, 100, &edges);
    let held = memory.peak();
    assert!(held > 0 && held <= memory.limit());
    let wide = Control::new(1, &memory, &Never);
    let e = betweenness(&g, &Betweenness::default(), &Control::new(64, &memory, &Never)).unwrap_err();
    assert_eq!(e.code, Code::Resources, "{}", e.msg);
    assert!(pagerank(&g, &PageRank::default(), &wide).is_ok(), "the failed run gave its bytes back");
    let mut b = GraphBuilder::new(&memory, 2, false).unwrap();
    assert_eq!(b.edge(0, 2, 1.0_f64).unwrap_err().code, Code::Api);
}

/// Verifies: [EXTENSION] [GS §18] a run told to stop stops with the
/// cancel's error, on one thread and on four, at its next morsel.
#[test]
fn a_cancelled_run_stops() {
    let memory = Memory::new(PLENTY);
    let g = of(&memory, VERTICES, &random_edges(5, VERTICES, EDGES));
    for width in [1, 4] {
        let looks = AtomicUsize::new(0);
        let cancel = || {
            if looks.fetch_add(1, Ordering::Relaxed) >= 3 { Err(SqlError::new(Code::Cancelled, "[DESIGN] cancelled")) } else { Ok(()) }
        };
        let control = Control::new(width, &memory, &cancel);
        let e = pagerank(&g, &PageRank::default(), &control).unwrap_err();
        assert_eq!(e.code, Code::Cancelled);
        let u = g.undirected(&Control::new(width, &memory, &Never)).unwrap();
        let again = || Err(SqlError::new(Code::Cancelled, "[DESIGN] cancelled"));
        let stopped = Control::new(width, &memory, &again);
        assert_eq!(louvain(&u, &Louvain::default(), &stopped).unwrap_err().code, Code::Cancelled);
        assert_eq!(scc(&g, &stopped).unwrap_err().code, Code::Cancelled);
        assert_eq!(k_core(&u, &stopped).unwrap_err().code, Code::Cancelled);
    }
}

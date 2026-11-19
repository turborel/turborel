//! Every node reachable: no back edge lost to contention or maintenance.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};

use super::*;
use crate::sync::read;

/// The live nodes of `hnsw` no search from its entry can reach, by the
/// links of every level.
fn unreachable(hnsw: &Hnsw) -> usize {
    let graph = read(&hnsw.inner.graph).unwrap();
    let mut seen = vec![false; graph.len()];
    let mut queue = VecDeque::from([graph.entry]);
    while let Some(id) = queue.pop_front() {
        let Some(at) = seen.get_mut(graph::slot(id)).filter(|s| !**s) else { continue };
        *at = true;
        let top = graph.levels.get(graph::slot(id)).copied().unwrap_or(0);
        for level in 0..=top {
            queue.extend(graph.links(id, level).iter().copied());
        }
    }
    (0..graph.len()).filter(|at| !seen[*at] && graph.is_live(u32::try_from(*at).unwrap())).count()
}

/// Inserts row `i` under `v`, and removes it again if `i` is a multiple
/// of 3, leaving tombstones for maintenance to repair.
fn churn(hnsw: &Hnsw, v: &[f32], i: usize) {
    hnsw.insert(v, row(i)).unwrap();
    if i.is_multiple_of(3) {
        hnsw.remove(v, row(i)).unwrap();
    }
}

/// Verifies: [DESIGN] a back edge that keeps changing is kept, not
/// dropped: four writers inserting into one small region at once, while
/// maintenance repairs tombstones and swaps in renumbered graphs, leave
/// every live node reachable from the entry.
#[test]
fn hnsw_keeps_every_node_reachable_under_contention() {
    let dims = 8;
    let flat = clustered(2_400, dims, 2, 29);
    let budget = MemoryBudget::new(usize::MAX);
    let (pool, file) = pages(&budget);
    let hnsw = Hnsw::create(pool, file, &config(dims), (euclidean(), None), &budget).unwrap();
    let done = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let writers: Vec<_> = (0..4)
            .map(|w| {
                let (hnsw, flat) = (&hnsw, &flat);
                scope.spawn(move || {
                    for (i, v) in flat.chunks_exact(dims).enumerate().filter(|(i, _)| i % 4 == w) {
                        churn(hnsw, v, i);
                    }
                })
            })
            .collect();
        scope.spawn(|| {
            while !done.load(Ordering::Relaxed) {
                hnsw.maintain().unwrap();
            }
        });
        for w in writers {
            w.join().unwrap();
        }
        done.store(true, Ordering::Relaxed);
    });
    assert_eq!(unreachable(&hnsw), 0);
    hnsw.maintain().unwrap();
    assert_eq!(unreachable(&hnsw), 0);
}

/// Whether `hnsw`'s count of each node's incoming edges on layer 0 is
/// the count its lists give.
fn counted(hnsw: &Hnsw) -> bool {
    let graph = read(&hnsw.inner.graph).unwrap();
    let mut again = graph.clone();
    again.recount();
    (0..u32::try_from(graph.len()).unwrap()).all(|id| graph.incoming(id) == again.incoming(id))
}

/// Verifies: [DESIGN] a back edge's reselection never lets go of a
/// node's last incoming edge (RULES.md D97): inserted one by one at M 2
/// and 3, where a full list is chosen again at almost every insert, and
/// then repaired, every live node stays reachable from the entry, and
/// the incoming counts kept are those the lists give. A regression test,
/// failing without the shelter, as a full list's reselection let go of
/// nodes it alone linked to.
#[test]
fn hnsw_reselection_keeps_every_node_reachable() {
    let dims = 8;
    for (seed, m) in [(1_u64, 2_usize), (6, 2), (1, 3), (5, 3)] {
        let flat = clustered(1_500, dims, 2, seed);
        let budget = MemoryBudget::new(usize::MAX);
        let (pool, file) = pages(&budget);
        let hnsw = Hnsw::create(pool, file, &Config { m, ..config(dims) }, (euclidean(), None), &budget).unwrap();
        for (i, v) in flat.chunks_exact(dims).enumerate() {
            churn(&hnsw, v, i);
        }
        assert_eq!(unreachable(&hnsw), 0, "seed {seed}, M {m}");
        assert!(counted(&hnsw));
        hnsw.maintain().unwrap();
        assert_eq!(unreachable(&hnsw), 0, "seed {seed}, M {m}, repaired");
        assert!(counted(&hnsw));
    }
}

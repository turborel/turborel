//! [P2 §6.1] Q↑E, the increasing-walk query, written with PREV
//! [GS §11.1]: `(a) [-[t]-> WHERE PREV(t) IS NULL OR t.ts > PREV(t).ts]+ (b)`.
//! Checked against a brute-force enumeration of increasing walks on small
//! graphs, and an earliest-arrival oracle up to 10,000 vertices [GS §23.2
//! T20].

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::{filter, node, random_graph, search, sparse_graph, times, variables};
use crate::graph::binding::PathBinding;
use crate::graph::in_memory::InMemoryGraph;
use crate::graph::program::{ElementTest, PathPattern, Pattern, Read, Selector};
use crate::graph::search::SearchOptions;
use crate::graph::view::{ElementId, GraphView, Orientation};

/// Edge variable `t`.
const T: u32 = 2;

/// Q↑E under `selector`.
fn increasing(g: &Arc<InMemoryGraph>, selector: Selector) -> PathPattern {
    let ts = Arc::clone(g);
    let test = Some(filter(&[Read { var: T, previous: true }], move |b| {
        let now = ts.int(b.element().unwrap(), "ts");
        Ok(b.previous(T).is_none_or(|p| now > ts.int(p, "ts")))
    }));
    let edge = Pattern::Edge(ElementTest { var: Some(T), labels: None, filter: test }, Orientation::RIGHT);
    let pattern = Pattern::Concat(vec![node(Some(0), None, None), times(edge, 1, None), node(Some(1), None, None)]);
    PathPattern::new(variables(&["a", "b"], &["t"]), pattern).select(selector)
}

/// Whether `path` is a walk of `g` with timestamps strictly rising.
fn rises(g: &InMemoryGraph, path: &PathBinding) -> bool {
    let ts: Vec<i64> = path.edges().iter().map(|&e| g.int(e, "ts").unwrap()).collect();
    let linked = path.elements.windows(3).step_by(2).all(|w| {
        let ends = g.endpoints(w[1]).unwrap();
        (ends.source, ends.destination) == (w[0], w[2])
    });
    linked && ts.windows(2).all(|w| w[0] < w[1]) && !ts.is_empty()
}

/// The fewest edges of an increasing walk from each start to each vertex,
/// by listing every increasing walk.
fn brute_force(g: &InMemoryGraph) -> HashMap<(ElementId, ElementId), usize> {
    let mut best = HashMap::new();
    for start in g.vertices() {
        let mut stack: Vec<(ElementId, Option<i64>, usize)> = vec![(start, None, 0)];
        while let Some((at, last, length)) = stack.pop() {
            let mut hops = Vec::new();
            g.hops(at, Orientation::RIGHT, &mut hops).unwrap();
            for hop in hops {
                let ts = g.int(hop.edge, "ts").unwrap();
                if last.is_none_or(|l| l < ts) {
                    let e = best.entry((start, hop.vertex)).or_insert(usize::MAX);
                    *e = (*e).min(length.wrapping_add(1));
                    stack.push((hop.vertex, Some(ts), length.wrapping_add(1)));
                }
            }
        }
    }
    best
}

/// The vertices an increasing walk from `start` reaches: earliest arrival,
/// the edges taken in order of time, those of one time at once.
fn earliest_arrival(g: &InMemoryGraph, start: ElementId) -> HashSet<ElementId> {
    let mut edges: Vec<(i64, ElementId, ElementId)> =
        g.edges().iter().map(|&e| (g.int(e, "ts").unwrap(), g.endpoints(e).unwrap().source, g.endpoints(e).unwrap().destination)).collect();
    edges.sort_unstable();
    let mut arrival: HashMap<ElementId, i64> = HashMap::new();
    for same in edges.chunk_by(|x, y| x.0 == y.0) {
        let reached: Vec<(ElementId, i64)> =
            same.iter().filter(|(ts, u, _)| *u == start || arrival.get(u).is_some_and(|a| a < ts)).map(|&(ts, _, v)| (v, ts)).collect();
        for (v, ts) in reached {
            arrival.entry(v).or_insert(ts);
        }
    }
    arrival.into_keys().collect()
}

/// Verifies: [EXTENSION] [GS §23.2 T20] [P2 §6.1] [GS §11.1]: on dense
/// random graphs of up to 12 vertices, ANY SHORTEST of Q↑E reaches each
/// pair an increasing walk does, by one of the fewest edges, and each
/// path it gives rises.
#[test]
fn t20_increasing_walks_match_brute_force() {
    for (seed, n, p) in [(7, 6, 500), (11, 9, 300), (13, 12, 100), (19, 12, 250)] {
        let g = Arc::new(random_graph(seed, n, p, 8));
        let found = search(&increasing(&g, Selector::AnyShortest), &g, &g.vertices(), None, SearchOptions::default()).unwrap();
        let want = brute_force(&g);
        let mut got = HashMap::new();
        for path in &found {
            assert!(rises(&g, path), "{path} does not rise");
            assert!(got.insert((path.start().unwrap(), path.end().unwrap()), path.length()).is_none(), "a partition twice");
        }
        assert_eq!(got, want, "seed {seed}");
    }
}

/// Verifies: [EXTENSION] [GS §23.2 T20] [P2 §6.1]: on sparse random graphs
/// of 1,000 and 10,000 vertices, ANY of Q↑E from a sample of starts
/// reaches the vertices earliest arrival does, each path rising.
#[test]
fn t20_increasing_walks_match_earliest_arrival() {
    for (seed, n, sample) in [(23, 1_000, 12), (29, 10_000, 4)] {
        let g = Arc::new(sparse_graph(seed, n, 3, 1_000));
        let vertices = g.vertices();
        for &start in vertices.iter().step_by(vertices.len().div_euclid(sample)) {
            let found = search(&increasing(&g, Selector::Any(1)), &g, &[start], None, SearchOptions::default()).unwrap();
            assert!(found.iter().all(|p| rises(&g, p)));
            let ends: HashSet<ElementId> = found.iter().map(|p| p.end().unwrap()).collect();
            assert_eq!(ends.len(), found.len(), "one path to each end");
            assert_eq!(ends, earliest_arrival(&g, start), "from {start}");
        }
    }
}

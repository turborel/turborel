//! How a search is chosen: MS-BFS or BFS from each start by cost, workers
//! when the work pays for them, a PATH_ALGO hint within its family, and a
//! frontier's hops asked of the view at once; the results the same every
//! way.

use std::sync::atomic::{AtomicUsize, Ordering};

use super::super::choice::reach;
use super::{label, node, right, search, sparse_graph, times, variables};
use crate::ast::PathAlgorithm;
use crate::error::{Code, Result};
use crate::graph::binding::PathBinding;
use crate::graph::in_memory::InMemoryGraph;
use crate::graph::program::{PathPattern, Pattern, Restrictor, Selector};
use crate::graph::search::{Algorithm, Parallelism, PathSearch, SearchOptions};
use crate::graph::view::{ElementId, Endpoints, GraphSize, GraphView, Hop, LabelId, Orientation};

/// `(x)-[e:1]->*(y)` under `restrictor` and `selector`.
fn reach_all(restrictor: Restrictor, selector: Selector) -> PathPattern {
    let pattern =
        Pattern::Concat(vec![node(Some(0), None, None), times(right(Some(2), label(1), None), 0, None), node(Some(1), None, None)]);
    PathPattern::new(variables(&["x", "y"], &["e"]), pattern).restrict(restrictor).select(selector)
}

/// Options of `threads` workers from `least` states, of hint `algorithm`.
fn options(threads: usize, least: usize, algorithm: Option<PathAlgorithm>) -> SearchOptions {
    SearchOptions { threads, parallel_min_states: least, algorithm, deterministic: true, ..SearchOptions::default() }
}

/// The paths' elements, sorted.
fn sorted(paths: &[PathBinding]) -> Vec<Vec<ElementId>> {
    let mut all: Vec<Vec<ElementId>> = paths.iter().map(|p| p.elements.clone()).collect();
    all.sort();
    all
}

/// `n` vertices, each even one with an edge to the next: every start
/// reaches one vertex but itself.
fn pairs(n: u32) -> InMemoryGraph {
    let mut g = InMemoryGraph::new();
    let vertices: Vec<ElementId> = (0..n).map(|_| g.vertex(0, &[])).collect();
    for pair in vertices.chunks(2) {
        if let [a, b] = pair {
            g.edge(1, (*a, *b), true, &[1]);
        }
    }
    g
}

/// Verifies: [DESIGN] a start's reach is its graph's average degree's
/// powers summed, to the longest walk if one is given, at most every
/// vertex; of a degree below one, the start alone.
#[test]
fn reach_is_estimated_from_the_degree() {
    let size = GraphSize { vertices: 1_000, edges: 3_000 };
    assert_eq!(reach(size, None), 1_000);
    assert_eq!(reach(size, Some(2)), 13);
    assert_eq!(reach(size, Some(0)), 1);
    assert_eq!(reach(GraphSize { vertices: 1_000, edges: 500 }, None), 1);
    assert_eq!(reach(GraphSize { vertices: 0, edges: 0 }, None), 1);
}

/// Verifies: [EXTENSION] [GS §15]: of ANY SHORTEST from many starts,
/// MS-BFS is chosen when a batch's reaches cover the graph, BFS from each
/// start when they do not; each finds the walks the other does.
#[test]
fn multi_source_is_chosen_by_cost() {
    let path = reach_all(Restrictor::Walk, Selector::AnyShortest);
    let dense = sparse_graph(37, 300, 3, 10);
    let starts: Vec<ElementId> = dense.vertices().into_iter().take(100).collect();
    let chosen = PathSearch::new(&path, &dense, options(1, 0, None)).unwrap();
    assert_eq!(chosen.plan(100, None).unwrap().algorithm, Algorithm::MultiSourceBfs);
    let each = search(&path, &dense, &starts, None, options(1, 0, Some(PathAlgorithm::Bfs))).unwrap();
    assert_eq!(sorted(&search(&path, &dense, &starts, None, options(1, 0, None)).unwrap()), sorted(&each));
    let apart = pairs(2_000);
    let everyone = apart.vertices();
    let each_alone = PathSearch::new(&path, &apart, options(1, 0, None)).unwrap();
    assert_eq!(each_alone.plan(everyone.len(), None).unwrap().algorithm, Algorithm::Bfs, "each start reaches little");
    let batched = search(&path, &apart, &everyone, None, options(1, 0, Some(PathAlgorithm::Msbfs))).unwrap();
    assert_eq!(sorted(&search(&path, &apart, &everyone, None, options(1, 0, None)).unwrap()), sorted(&batched));
    assert_eq!(batched.len(), 3_000);
}

/// Verifies: [EXTENSION] [GS §17]: a search runs on its workers only when
/// there are more than one and its estimated states reach
/// GRAPH.PARALLEL_MIN_STATES; its results are the same either way.
#[test]
fn workers_are_used_when_they_pay() {
    let g = sparse_graph(41, 300, 3, 10);
    let path = reach_all(Restrictor::Walk, Selector::AnyShortest);
    let plan = |threads, least, starts| PathSearch::new(&path, &g, options(threads, least, None)).unwrap().plan(starts, None).unwrap();
    assert_eq!(plan(4, 0, 200).parallelism, Parallelism::Morsels(4));
    assert_eq!(plan(4, 1_000_000, 200).parallelism, Parallelism::Sequential);
    assert_eq!(plan(1, 0, 200).parallelism, Parallelism::Sequential);
    assert_eq!(plan(4, 0, 1).parallelism, Parallelism::LevelSynchronous(4));
    assert_eq!(plan(4, 300, 1).parallelism, Parallelism::LevelSynchronous(4), "one start reaches every vertex");
    assert_eq!(plan(4, 301, 1).parallelism, Parallelism::Sequential);
    let starts = g.vertices();
    let one = search(&path, &g, &starts, None, options(1, 0, Some(PathAlgorithm::Bfs))).unwrap();
    let four = search(&path, &g, &starts, None, options(4, 0, Some(PathAlgorithm::Bfs))).unwrap();
    assert_eq!(sorted(&one), sorted(&four));
    let text = PathSearch::new(&path, &g, options(4, 0, None)).unwrap().explain(200, None);
    assert!(text.starts_with("MS-BFS (morsels of starts, 4 workers) by cost: 200 starts, est. reach 300 of 300 vertices\n"), "{text}");
}

/// The algorithm hint `hint` chooses for `restrictor` and `selector`, of
/// `starts` starts and `ends` ends, or the code it is refused with.
fn hinted(
    hint: PathAlgorithm,
    (restrictor, selector): (Restrictor, Selector),
    (starts, ends): (usize, Option<usize>),
) -> Result<Algorithm> {
    let g = sparse_graph(43, 50, 2, 10);
    let path = reach_all(restrictor, selector);
    Ok(PathSearch::new(&path, &g, options(1, 0, Some(hint)))?.plan(starts, ends)?.algorithm)
}

/// Verifies: [EXTENSION] [GS §16 R16.8]: PATH_ALGO chooses an algorithm
/// of the family the selector and restrictors call for, and is refused,
/// SQLCODE -103 (GRF-0070), when none of it answers the pattern.
#[test]
fn hints_choose_within_the_family() {
    let walk = |s| (Restrictor::Walk, s);
    assert_eq!(hinted(PathAlgorithm::Msbfs, walk(Selector::AnyShortest), (1, None)).unwrap(), Algorithm::MultiSourceBfs);
    assert_eq!(hinted(PathAlgorithm::Bfs, walk(Selector::AnyShortest), (200, None)).unwrap(), Algorithm::Bfs);
    assert_eq!(hinted(PathAlgorithm::Bfs, walk(Selector::AllShortest), (2, None)).unwrap(), Algorithm::AllShortest);
    assert_eq!(hinted(PathAlgorithm::Bfs, (Restrictor::Trail, Selector::AnyShortest), (2, None)).unwrap(), Algorithm::PathBfs);
    assert_eq!(hinted(PathAlgorithm::Bibfs, walk(Selector::AnyShortest), (1, Some(1))).unwrap(), Algorithm::BidirectionalBfs);
    assert_eq!(hinted(PathAlgorithm::Dfs, (Restrictor::Trail, Selector::All), (2, None)).unwrap(), Algorithm::Dfs);
    assert_eq!(hinted(PathAlgorithm::Dijkstra, walk(Selector::AnyCheapest), (2, None)).unwrap(), Algorithm::Dijkstra);
    assert_eq!(hinted(PathAlgorithm::Dijkstra, (Restrictor::Trail, Selector::AnyCheapest), (2, None)).unwrap(), Algorithm::PathDijkstra);
    for (hint, case, counts) in [
        (PathAlgorithm::Dijkstra, walk(Selector::AnyShortest), (2, None)),
        (PathAlgorithm::Bibfs, walk(Selector::AnyShortest), (2, None)),
        (PathAlgorithm::Bibfs, walk(Selector::AnyShortest), (1, None)),
        (PathAlgorithm::Dfs, walk(Selector::AnyShortest), (2, None)),
        (PathAlgorithm::Msbfs, walk(Selector::AllShortest), (2, None)),
        (PathAlgorithm::Bfs, (Restrictor::Trail, Selector::All), (2, None)),
    ] {
        let e = hinted(hint, case, counts).unwrap_err();
        assert_eq!(e.code, Code::SyntaxRule, "{hint:?}");
        assert!(e.msg.contains("GRF-0070") && e.msg.contains(hint.word()), "{}", e.msg);
    }
}

/// A view of a graph counting the hops it is asked for one vertex at a
/// time and a frontier at a time.
struct Counted<'g> {
    graph: &'g InMemoryGraph,
    alone: AtomicUsize,
    batches: AtomicUsize,
}

impl GraphView for Counted<'_> {
    fn labels(&self, element: ElementId, out: &mut Vec<LabelId>) -> Result<()> {
        self.graph.labels(element, out)
    }

    fn hops(&self, vertex: ElementId, orientation: Orientation, out: &mut Vec<Hop>) -> Result<()> {
        self.alone.fetch_add(1, Ordering::Relaxed);
        self.graph.hops(vertex, orientation, out)
    }

    fn expand(&self, asked: &[(ElementId, Orientation)], out: &mut Vec<Vec<Hop>>) -> Result<()> {
        self.batches.fetch_add(1, Ordering::Relaxed);
        self.graph.expand(asked, out)
    }

    fn endpoints(&self, edge: ElementId) -> Result<Endpoints> {
        self.graph.endpoints(edge)
    }

    fn cost(&self, edge: ElementId) -> Result<f64> {
        self.graph.cost(edge)
    }
}

/// Verifies: [DESIGN] the breadth-first searches, from one start, from
/// many at once, of ALL SHORTEST and level by level on workers, ask the
/// view a frontier's hops at once, never one vertex's, and find what they
/// find of a view asked one vertex at a time.
#[test]
fn frontiers_are_expanded_at_once() {
    let g = sparse_graph(47, 400, 3, 10);
    let starts: Vec<ElementId> = g.vertices().into_iter().take(70).collect();
    for (selector, hint, threads) in [
        (Selector::AnyShortest, PathAlgorithm::Bfs, 1),
        (Selector::AnyShortest, PathAlgorithm::Msbfs, 1),
        (Selector::AllShortest, PathAlgorithm::Bfs, 1),
        (Selector::AnyShortest, PathAlgorithm::Bfs, 4),
    ] {
        let path = reach_all(Restrictor::Walk, selector);
        let view = Counted { graph: &g, alone: AtomicUsize::new(0), batches: AtomicUsize::new(0) };
        let from = if threads > 1 { &starts[..1] } else { &starts[..] };
        let found: Vec<PathBinding> = PathSearch::new(&path, &view, options(threads, 0, Some(hint)))
            .unwrap()
            .run(from, None)
            .unwrap()
            .collect::<Result<_>>()
            .unwrap();
        assert_eq!(view.alone.load(Ordering::Relaxed), 0, "{selector} {hint:?}");
        assert!(view.batches.load(Ordering::Relaxed) > 0, "{selector} {hint:?}");
        assert_eq!(sorted(&found), sorted(&search(&path, &g, from, None, options(1, 0, Some(hint))).unwrap()), "{selector} {hint:?}");
    }
}

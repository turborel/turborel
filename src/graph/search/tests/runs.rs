//! How searches run: in parallel, from many starts at once, under limits,
//! cancelled, out of memory, and as EXPLAIN shows them; and match modes
//! over every restrictor and selector.

use std::sync::Arc;

use super::{label, node, random_graph, right, search, sparse_graph, times, variables};
use crate::error::Code;
use crate::graph::binding::PathBinding;
use crate::graph::in_memory::InMemoryGraph;
use crate::graph::limits::{Area, Limits, OnLimit};
use crate::graph::matching::{PathMatches, matches};
use crate::graph::program::{MatchMode, PathPattern, Pattern, Restrictor, Selector};
use crate::graph::search::{Algorithm, Parallelism, PathSearch, SearchOptions};
use crate::graph::view::ElementId;
use crate::memory::MemoryBudget;
use crate::txn::Stop;

/// `(x)-[e:1]->*(y)` under `restrictor` and `selector`.
fn reach(restrictor: Restrictor, selector: Selector) -> PathPattern {
    let pattern =
        Pattern::Concat(vec![node(Some(0), None, None), times(right(Some(2), label(1), None), 0, None), node(Some(1), None, None)]);
    PathPattern::new(variables(&["x", "y"], &["e"]), pattern).restrict(restrictor).select(selector)
}

/// Options of `threads` workers, deterministic.
fn workers(threads: usize) -> SearchOptions {
    SearchOptions { threads, deterministic: true, ..SearchOptions::default() }
}

/// The paths' elements, in order.
fn elements(paths: &[PathBinding]) -> Vec<Vec<ElementId>> {
    paths.iter().map(|p| p.elements.clone()).collect()
}

/// Verifies: [EXTENSION] [GS §17] [GS §7.2 R7.2]: with DETERMINISTIC_PATHS,
/// a search on 4 workers, by morsels of starts or level by level, gives
/// what it gives on one, path for path, for every algorithm.
#[test]
fn parallel_searches_equal_sequential() {
    let g = random_graph(31, 70, 50, 10);
    let small = random_graph(33, 12, 200, 10);
    let cases = [
        (Restrictor::Walk, Selector::AnyShortest),
        (Restrictor::Walk, Selector::AllShortest),
        (Restrictor::Walk, Selector::Shortest(3)),
        (Restrictor::Walk, Selector::ShortestGroups(2)),
        (Restrictor::Walk, Selector::Cheapest(2)),
        (Restrictor::Trail, Selector::AnyShortest),
        (Restrictor::Acyclic, Selector::AllCheapest),
    ];
    for (restrictor, selector) in cases {
        let path = reach(restrictor, selector);
        // Restricted searches list paths, many more than walks: a small graph.
        let g = if restrictor == Restrictor::Walk { &g } else { &small };
        let all = g.vertices();
        for starts in [&all[..], &all[..1]] {
            let one = search(&path, g, starts, None, workers(1)).unwrap();
            let four = search(&path, g, starts, None, workers(4)).unwrap();
            assert_eq!(elements(&one), elements(&four), "{restrictor} {selector} from {} starts", starts.len());
        }
    }
    let plan = |starts, ends| PathSearch::new(&reach(Restrictor::Walk, Selector::AnyShortest), &g, workers(4)).unwrap().plan(starts, ends);
    assert_eq!(plan(1, None).parallelism, Parallelism::LevelSynchronous(4));
    assert_eq!(plan(200, None).parallelism, Parallelism::Morsels(4));
    assert_eq!(plan(30, None).parallelism, Parallelism::Sequential, "one batch of MS-BFS");
}

/// Verifies: [EXTENSION] [GS §15, §17]: MS-BFS from up to 64 starts at
/// once gives each start the walks a search of its own gives.
#[test]
fn multi_source_equals_one_search_per_start() {
    let g = sparse_graph(37, 300, 3, 10);
    let starts: Vec<ElementId> = g.vertices().into_iter().take(100).collect();
    let path = reach(Restrictor::Walk, Selector::AnyShortest);
    let options = SearchOptions::default();
    let together = PathSearch::new(&path, &g, options.clone()).unwrap();
    assert_eq!(together.plan(starts.len(), None).algorithm, Algorithm::MultiSourceBfs);
    let batch: Vec<PathBinding> = together.run(&starts, None).unwrap().collect::<crate::error::Result<_>>().unwrap();
    let mut alone = Vec::new();
    for &s in &starts {
        alone.extend(search(&path, &g, &[s], None, options.clone()).unwrap());
    }
    assert_eq!(elements(&batch), elements(&alone));
}

/// Verifies: [EXTENSION] [GS §15]: from one start to one end, ANY SHORTEST
/// runs BiBFS, which finds a walk as short as BFS does, or none if BFS does.
#[test]
fn bidirectional_equals_breadth_first() {
    let g = sparse_graph(41, 400, 2, 10);
    let path = reach(Restrictor::Walk, Selector::AnyShortest);
    let searcher = PathSearch::new(&path, &g, SearchOptions::default()).unwrap();
    assert_eq!(searcher.plan(1, Some(1)).algorithm, Algorithm::BidirectionalBfs);
    let vertices = g.vertices();
    for &s in vertices.iter().step_by(40) {
        let every = search(&path, &g, &[s], None, SearchOptions::default()).unwrap();
        for &t in vertices.iter().step_by(13) {
            let pair = searcher.run(&[s], Some(&[t])).unwrap().collect::<crate::error::Result<Vec<_>>>().unwrap();
            let want = every.iter().find(|p| p.end() == Some(t)).map(PathBinding::length);
            assert_eq!(pair.first().map(PathBinding::length), want, "from {s} to {t}");
            assert!(pair.iter().all(|p| p.start() == Some(s) && p.end() == Some(t)));
        }
    }
}

/// Verifies: [EXTENSION] [GS §15]: with no selector, the depth-first
/// search gives each path as it is found: the first of the trails of a
/// complete graph of 12 vertices, too many to list, come at once.
#[test]
fn depth_first_streams_its_paths() {
    let mut g = InMemoryGraph::new();
    let vertices: Vec<ElementId> = (0_u32..12).map(|_| g.vertex(0, &[])).collect();
    for &a in &vertices {
        for &b in &vertices {
            g.edge(1, (a, b), true, &[1]);
        }
    }
    let searcher = PathSearch::new(&reach(Restrictor::Trail, Selector::All), &g, SearchOptions::default()).unwrap();
    let first: Vec<PathBinding> = searcher.run(&vertices[..1], None).unwrap().take(1_000).collect::<crate::error::Result<_>>().unwrap();
    assert_eq!(first.len(), 1_000);
    assert!(first.iter().all(PathBinding::is_trail));
}

/// Options with `limits`.
fn limited(limits: Limits) -> SearchOptions {
    SearchOptions { limits, stop: Some(Arc::new(Stop::default())), ..SearchOptions::default() }
}

/// Verifies: [EXTENSION] [GS §20]: MAX_SEARCH_STATES ON LIMIT ERROR is
/// GRF-0061, SQLCODE -900, SQLSTATE 54061; ON LIMIT TRUNCATE ends the
/// search with what it has and raises warning 01W61.
#[test]
fn search_state_limit_errors_or_truncates() {
    let g = sparse_graph(43, 500, 3, 10);
    let path = reach(Restrictor::Walk, Selector::AllShortest);
    let starts = g.vertices();
    let mut limits = Limits { max_search_states: Some(50), ..Limits::default() };
    let err = search(&path, &g, &starts, None, limited(limits)).unwrap_err();
    assert_eq!((err.sqlcode(), err.sqlstate().to_string()), (-900_i32, "54061".to_owned()), "{}", err.msg);
    assert!(err.msg.contains("GRF-0061"));
    limits.on_limit = OnLimit::Truncate;
    let options = limited(limits);
    let stop = Arc::clone(options.stop.as_ref().unwrap());
    let searcher = PathSearch::new(&path, &g, options).unwrap();
    let mut run = searcher.run(&starts, None).unwrap();
    let kept = run.by_ref().count();
    assert!(run.truncated() && kept < 500, "{kept}");
    assert_eq!(stop.warnings().take().map(|s| s.to_string()), Some("01W61".to_owned()));
}

/// Verifies: [EXTENSION] [GS §20]: MAX_PATH_LENGTH and
/// MAX_PATHS_PER_PARTITION, truncating, keep paths within them; as errors,
/// they are GRF-0061; the streaming DFS truncates as the others do.
#[test]
fn length_and_partition_limits() {
    let g = random_graph(47, 8, 400, 10);
    let starts = g.vertices();
    let truncate = |limits: Limits| limited(Limits { on_limit: OnLimit::Truncate, ..limits });
    let short = Limits { max_path_length: Some(2), ..Limits::default() };
    for (restrictor, selector) in [(Restrictor::Walk, Selector::Shortest(4)), (Restrictor::Trail, Selector::All)] {
        let found = search(&reach(restrictor, selector), &g, &starts, None, truncate(short)).unwrap();
        assert!(!found.is_empty() && found.iter().all(|p| p.length() <= 2), "{restrictor} {selector}");
        let err = search(&reach(restrictor, selector), &g, &starts, None, limited(short)).unwrap_err();
        assert_eq!(err.sqlcode(), -900_i32, "{restrictor} {selector}: {}", err.msg);
    }
    let few = Limits { max_paths_per_partition: Some(2), ..Limits::default() };
    let found = search(&reach(Restrictor::Acyclic, Selector::All), &g, &starts, None, truncate(few)).unwrap();
    for s in &starts {
        for t in &starts {
            assert!(found.iter().filter(|p| p.start() == Some(*s) && p.end() == Some(*t)).count() <= 2);
        }
    }
}

/// Verifies: [EXTENSION] [GS §20] (RULES.md D44): a cancelled statement's
/// search ends with SQLCODE -920, sequential, parallel or streaming.
#[test]
fn cancel_ends_every_search() {
    let g = sparse_graph(53, 300, 3, 10);
    let stop = Arc::new(Stop::default());
    let _window = stop.window();
    stop.cancel();
    let options = |threads| SearchOptions { threads, stop: Some(Arc::clone(&stop)), ..SearchOptions::default() };
    for (restrictor, selector, threads) in [
        (Restrictor::Walk, Selector::AnyShortest, 1),
        (Restrictor::Walk, Selector::AllShortest, 4),
        (Restrictor::Trail, Selector::All, 1),
        (Restrictor::Walk, Selector::AnyCheapest, 1),
    ] {
        let err = search(&reach(restrictor, selector), &g, &g.vertices(), None, options(threads)).unwrap_err();
        assert_eq!(err.code, Code::Cancelled, "{restrictor} {selector}");
    }
}

/// Verifies: [EXTENSION] [GS §20]: past its work area a search reserves of
/// the budget, failing with -918 when the budget or the area is spent.
#[test]
fn memory_past_the_work_area() {
    let g = sparse_graph(59, 120, 3, 10);
    let path = reach(Restrictor::Walk, Selector::AllShortest);
    let area = |work, budget| SearchOptions { area: Area { work, budget }, ..SearchOptions::default() };
    let unbudgeted = search(&path, &g, &g.vertices(), None, area(1_000, None)).unwrap_err();
    assert_eq!(unbudgeted.code, Code::Resources);
    let spent = search(&path, &g, &g.vertices(), None, area(1_000, Some(MemoryBudget::new(1_000)))).unwrap_err();
    assert_eq!(spent.code, Code::Resources);
    let budget = MemoryBudget::new(1 << 30);
    assert!(search(&path, &g, &g.vertices(), None, area(1_000, Some(Arc::clone(&budget)))).is_ok());
    assert_eq!(budget.reserved(), 0, "the reservation is given back");
}

/// Verifies: [EXTENSION] [GS §7.2] GRF-0034: a negative or NaN cost is
/// SQLCODE -301, SQLSTATE 22W34, for every CHEAPEST search.
#[test]
fn negative_cost_is_grf_0034() {
    for cost in [-1.0_f64, f64::NAN] {
        let mut g = InMemoryGraph::new();
        let (a, b, c) = (g.vertex(0, &[]), g.vertex(0, &[]), g.vertex(0, &[]));
        g.edge(1, (a, b), true, &[1]);
        let bad = g.edge(1, (b, c), true, &[1]);
        g.set_cost(bad, cost);
        for (restrictor, selector) in [
            (Restrictor::Walk, Selector::AnyCheapest),
            (Restrictor::Walk, Selector::Cheapest(2)),
            (Restrictor::Trail, Selector::AllCheapest),
        ] {
            let err = search(&reach(restrictor, selector), &g, &[a], None, SearchOptions::default()).unwrap_err();
            assert_eq!((err.sqlcode(), err.sqlstate().to_string()), (-301_i32, "22W34".to_owned()));
            assert!(err.msg.contains("GRF-0034"));
        }
    }
}

/// Verifies: [EXTENSION] [GS §7.2]: a k of 0 is SQLCODE -103.
#[test]
fn zero_k_is_refused() {
    let g = InMemoryGraph::new();
    for selector in [Selector::Any(0), Selector::Shortest(0), Selector::ShortestGroups(0), Selector::Cheapest(0)] {
        let err = PathSearch::new(&reach(Restrictor::Trail, selector), &g, SearchOptions::default()).err().unwrap();
        assert_eq!(err.code, Code::SyntaxRule, "{selector}");
    }
}

/// Verifies: [EXTENSION] [GS §16 R16.9]: EXPLAIN shows the algorithm, its
/// workers and the automaton.
#[test]
fn explain_shows_algorithm_and_automaton() {
    let g = InMemoryGraph::new();
    let searcher = PathSearch::new(&reach(Restrictor::Walk, Selector::AnyShortest), &g, workers(4)).unwrap();
    let text = searcher.explain(1, None);
    assert!(text.starts_with("PathSearch-BFS (level-synchronous, 4 workers)\nANY SHORTEST WALK start"), "{text}");
    assert!(text.contains("edge -> v2 :L1 -> ") && text.contains("\n0: accept\n"), "{text}");
    assert!(searcher.explain(1, Some(1)).starts_with("PathSearch-BiBFS"));
    let trail = PathSearch::new(&reach(Restrictor::Trail, Selector::All), &g, workers(4)).unwrap();
    assert!(trail.explain(100, None).starts_with("PathSearch-DFS\n"));
}

/// Verifies: [EXTENSION] [GS §7.3] [GS §6.4]: for every restrictor and
/// selector, the match modes keep of two path patterns' matches those a
/// direct check keeps: DIFFERENT EDGES no edge twice, DIFFERENT VERTICES
/// no vertex twice, the shared singleton `y` joining.
#[test]
fn match_modes_over_restrictors_and_selectors() {
    let g = random_graph(61, 5, 300, 10);
    let bounded = |restrictor, selector, names: [&str; 3]| {
        let pattern =
            Pattern::Concat(vec![node(Some(0), None, None), times(right(Some(2), label(1), None), 1, Some(3)), node(Some(1), None, None)]);
        PathPattern::new(variables(&names[..2], &names[2..]), pattern).restrict(restrictor).select(selector)
    };
    for restrictor in [Restrictor::Walk, Restrictor::Trail, Restrictor::Acyclic, Restrictor::Simple] {
        for selector in [Selector::All, Selector::AnyShortest, Selector::AllShortest, Selector::Shortest(2), Selector::AnyCheapest] {
            let first = bounded(restrictor, selector, ["x", "y", "e"]);
            let second = bounded(restrictor, selector, ["y", "z", "f"]);
            let run = |p: &PathPattern| search(p, &g, &g.vertices(), None, SearchOptions::default()).unwrap();
            let paths = [PathMatches { pattern: &first, bindings: run(&first) }, PathMatches { pattern: &second, bindings: run(&second) }];
            for mode in [MatchMode::RepeatableElements, MatchMode::DifferentEdges, MatchMode::DifferentVertices] {
                let pairs = paths[0].bindings.iter().enumerate().flat_map(|a| paths[1].bindings.iter().enumerate().map(move |b| (a, b)));
                let want: Vec<Vec<usize>> =
                    pairs.filter(|((_, a), (_, b))| a.end() == b.start() && keeps(mode, a, b)).map(|((i, _), (j, _))| vec![i, j]).collect();
                assert_eq!(matches(&paths, mode), want, "{restrictor} {selector} {mode:?}");
            }
        }
    }
}

/// Verifies: [EXTENSION] [GS §7.3]: a group variable binding one edge
/// twice, a self-loop followed twice, is a repeat DIFFERENT EDGES drops,
/// not a join; a singleton bound twice to one vertex is a join.
#[test]
fn group_variable_repeat_is_not_a_join() {
    let mut g = InMemoryGraph::new();
    let v = g.vertex(0, &[]);
    g.edge(1, (v, v), true, &[1]);
    let pattern =
        Pattern::Concat(vec![node(Some(0), None, None), times(right(Some(2), label(1), None), 2, Some(2)), node(Some(0), None, None)]);
    let path = PathPattern::new(variables(&["x", "y"], &["e"]), pattern);
    let bindings = search(&path, &g, &[v], None, SearchOptions::default()).unwrap();
    assert_eq!(bindings.len(), 1);
    let paths = [PathMatches { pattern: &path, bindings }];
    assert_eq!(matches(&paths, MatchMode::RepeatableElements).len(), 1);
    assert_eq!(matches(&paths, MatchMode::DifferentEdges).len(), 0, "e binds the loop twice");
    assert_eq!(matches(&paths, MatchMode::DifferentVertices).len(), 0, "the middle vertex is unnamed");
}

/// Whether `mode` keeps the match of `a` then `b`, `a`'s end and `b`'s
/// start the one vertex `y`.
fn keeps(mode: MatchMode, a: &PathBinding, b: &PathBinding) -> bool {
    let distinct = |mut xs: Vec<ElementId>| {
        let n = xs.len();
        xs.sort_unstable();
        xs.dedup();
        xs.len() == n
    };
    match mode {
        MatchMode::RepeatableElements => true,
        MatchMode::DifferentEdges => distinct([a.edges(), b.edges()].concat()),
        MatchMode::DifferentVertices => distinct([a.vertices(), b.vertices().split_off(1)].concat()),
    }
}

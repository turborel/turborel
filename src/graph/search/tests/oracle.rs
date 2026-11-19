//! Every restrictor with every selector, against a brute-force
//! enumerator of walks on small random graphs [GS §7.1, §7.2] [P1 Fig. 7,
//! Fig. 8].

use std::collections::HashMap;

use super::{label, node, random_graph, right, search, times, variables};
use crate::graph::binding::PathBinding;
use crate::graph::in_memory::InMemoryGraph;
use crate::graph::program::{PathPattern, Pattern, Restrictor, Selector};
use crate::graph::search::SearchOptions;
use crate::graph::view::{ElementId, GraphView, Orientation};

/// The longest walk the enumerator lists.
const BOUND: usize = 9;

/// A walk: its vertices and edges.
#[derive(Clone, Debug, PartialEq)]
struct Walk {
    vertices: Vec<ElementId>,
    edges: Vec<ElementId>,
    cost: f64,
}

/// Every walk of label-1 edges of fewer than `BOUND` edges from each vertex.
fn walks(g: &InMemoryGraph) -> Vec<Walk> {
    let mut out = Vec::new();
    let mut stack: Vec<Walk> = g.vertices().into_iter().map(|v| Walk { vertices: vec![v], edges: Vec::new(), cost: 0.0 }).collect();
    while let Some(walk) = stack.pop() {
        if walk.edges.len() < BOUND.wrapping_sub(1) {
            let mut hops = Vec::new();
            g.hops(*walk.vertices.last().unwrap(), Orientation::RIGHT, &mut hops).unwrap();
            for hop in hops {
                let mut labels = Vec::new();
                g.labels(hop.edge, &mut labels).unwrap();
                if labels.contains(&1) {
                    let mut next = walk.clone();
                    next.vertices.push(hop.vertex);
                    next.edges.push(hop.edge);
                    next.cost += g.cost(hop.edge).unwrap();
                    stack.push(next);
                }
            }
        }
        out.push(walk);
    }
    out
}

/// Whether `walk` keeps `restrictor`.
fn keeps(walk: &Walk, restrictor: Restrictor) -> bool {
    let distinct = |xs: &[ElementId]| {
        let mut s = xs.to_vec();
        s.sort();
        s.windows(2).all(|w| w[0] != w[1])
    };
    match restrictor {
        Restrictor::Walk => true,
        Restrictor::Trail => distinct(&walk.edges),
        Restrictor::Acyclic => distinct(&walk.vertices),
        Restrictor::Simple => {
            let v = &walk.vertices;
            if v.len() > 1 && v.first() == v.last() { distinct(&v[1..]) } else { distinct(v) }
        }
    }
}

/// The measure a selector orders by.
fn measure(walk: &Walk, selector: Selector) -> f64 {
    if selector.by_cost() { walk.cost } else { f64::from(u32::try_from(walk.edges.len()).unwrap()) }
}

/// Checks the paths `got` of one partition against its walks `all`,
/// sorted by measure, under `selector`; `finite` if `all` is every walk
/// of the partition. Skips what the bound leaves undecided.
fn check(got: &[Walk], all: &[Walk], selector: Selector, finite: bool) {
    let k = usize::try_from(selector.k()).unwrap();
    let m = |w: &Walk| measure(w, selector);
    let bound = f64::from(u32::try_from(BOUND).unwrap());
    for w in got {
        assert!(all.contains(w), "{selector}: {w:?} is not a walk of the partition");
    }
    let mut seen = got.to_vec();
    seen.dedup();
    assert_eq!(seen.len(), got.len(), "{selector}: a path twice");
    let distinct: Vec<f64> = all.iter().map(m).fold(Vec::new(), |mut d, x| {
        if d.last() != Some(&x) {
            d.push(x);
        }
        d
    });
    let decided = |threshold: f64| finite || threshold < bound;
    match selector {
        Selector::All => assert_eq!(got.len(), all.len(), "{selector}"),
        Selector::AnyShortest | Selector::AnyCheapest | Selector::Any(_) | Selector::Shortest(_) | Selector::Cheapest(_) => {
            let want = k.min(all.len());
            if all.len() >= k || finite {
                assert_eq!(got.len(), want, "{selector}: {got:?}");
            }
            let ranked = !matches!(selector, Selector::Any(_));
            let last = want.checked_sub(1).map(|w| m(&all[w]));
            if ranked && last.is_some_and(decided) {
                let mut mine: Vec<f64> = got.iter().map(m).collect();
                mine.sort_by(f64::total_cmp);
                let first: Vec<f64> = all.iter().take(want).map(m).collect();
                assert_eq!(mine, first, "{selector}");
            }
        }
        Selector::AllShortest | Selector::AllCheapest | Selector::ShortestGroups(_) => {
            let groups = if matches!(selector, Selector::ShortestGroups(_)) { k } else { 1 };
            let last = distinct[groups.min(distinct.len()).saturating_sub(1)];
            if decided(last) && (distinct.len() >= groups || finite) {
                let want: Vec<&Walk> = all.iter().filter(|w| m(w) <= last).collect();
                assert_eq!(got.len(), want.len(), "{selector}: {got:?} against {want:?}");
            }
        }
    }
}

/// `binding` as a walk, its variables checked: x the start, y the end, e
/// each edge.
fn walk_of(binding: &PathBinding) -> Walk {
    assert_eq!(binding.singleton(0), binding.start());
    assert_eq!(binding.singleton(1), binding.end());
    assert_eq!(binding.group(2), binding.edges());
    Walk { vertices: binding.vertices(), edges: binding.edges(), cost: binding.cost.unwrap_or(0.0) }
}

/// `walk` with its cost, from the walks `all` if the selector is not by
/// cost and so gives none.
fn costed(mut walk: Walk, all: &[Walk], selector: Selector) -> Walk {
    if !selector.by_cost() {
        walk.cost = all.iter().find(|x| x.edges == walk.edges && x.vertices == walk.vertices).map_or(0.0_f64, |x| x.cost);
    }
    walk
}

/// The selectors checked.
const SELECTORS: [Selector; 12] = [
    Selector::All,
    Selector::Any(1),
    Selector::Any(2),
    Selector::AnyShortest,
    Selector::AllShortest,
    Selector::Shortest(1),
    Selector::Shortest(3),
    Selector::ShortestGroups(2),
    Selector::AnyCheapest,
    Selector::AllCheapest,
    Selector::Cheapest(2),
    Selector::Cheapest(3),
];

/// Verifies: [EXTENSION] [GS §7.1, §7.2 R7.1] [P1 §5.1, Fig. 7, Fig. 8]:
/// on random graphs, every restrictor with every selector gives, for
/// each (start, end) partition, what a brute-force enumeration of walks
/// filtered by the restrictor and then selected gives: restrictors during
/// the search, selectors after it. WALK with no selector is bounded,
/// `{0,8}`, as R7.3 requires.
#[test]
fn every_restrictor_and_selector_agree_with_brute_force() {
    for seed in [3, 17, 41] {
        let g = random_graph(seed, 5, 300, 10);
        let all = walks(&g);
        for restrictor in [Restrictor::Walk, Restrictor::Trail, Restrictor::Acyclic, Restrictor::Simple] {
            for selector in SELECTORS {
                let bounded = restrictor == Restrictor::Walk && selector == Selector::All;
                let max = if bounded { Some(u32::try_from(BOUND - 1).unwrap()) } else { None };
                let pattern = Pattern::Concat(vec![
                    node(Some(0), None, None),
                    times(right(Some(2), label(1), None), 0, max),
                    node(Some(1), None, None),
                ]);
                let path = PathPattern::new(variables(&["x", "y"], &["e"]), pattern).restrict(restrictor).select(selector);
                let found = search(&path, &g, &g.vertices(), None, SearchOptions::default()).unwrap();
                let mut got: HashMap<(ElementId, ElementId), Vec<Walk>> = HashMap::new();
                for b in &found {
                    let w = costed(walk_of(b), &all, selector);
                    got.entry((b.start().unwrap(), b.end().unwrap())).or_default().push(w);
                }
                let mut parts: HashMap<(ElementId, ElementId), Vec<Walk>> = HashMap::new();
                for w in all.iter().filter(|w| keeps(w, restrictor)) {
                    parts.entry((w.vertices[0], *w.vertices.last().unwrap())).or_default().push(w.clone());
                }
                for (key, mut ws) in parts {
                    ws.sort_by(|a, b| measure(a, selector).total_cmp(&measure(b, selector)));
                    let finite = restrictor != Restrictor::Walk || bounded;
                    check(got.get(&key).map_or(&[][..], Vec::as_slice), &ws, selector, finite);
                }
                for key in got.keys() {
                    assert!(
                        all.iter().any(|w| (w.vertices[0], *w.vertices.last().unwrap()) == *key),
                        "{restrictor} {selector}: a partition with no walk"
                    );
                }
            }
        }
    }
}

/// Verifies: [EXTENSION] [GS §7.2 R7.1] [P1 §5.1]: with its ends given,
/// each selector keeps for the pair what it keeps with them not given,
/// whichever algorithm then runs (BiBFS for one start and one end).
#[test]
fn selectors_with_ends_given_agree() {
    let g = random_graph(5, 7, 250, 10);
    let vertices = g.vertices();
    for selector in [Selector::AnyShortest, Selector::AllShortest, Selector::Shortest(2), Selector::AnyCheapest] {
        let pattern =
            Pattern::Concat(vec![node(Some(0), None, None), times(right(Some(2), label(1), None), 0, None), node(Some(1), None, None)]);
        let path = PathPattern::new(variables(&["x", "y"], &["e"]), pattern).select(selector);
        let every = search(&path, &g, &vertices, None, SearchOptions::default()).unwrap();
        for &s in &vertices {
            for &t in &vertices {
                let pair = search(&path, &g, &[s], Some(&[t]), SearchOptions::default()).unwrap();
                let want: Vec<&PathBinding> = every.iter().filter(|b| b.start() == Some(s) && b.end() == Some(t)).collect();
                let lengths = |bs: &mut dyn Iterator<Item = &PathBinding>| {
                    let mut l: Vec<usize> = bs.map(PathBinding::length).collect();
                    l.sort_unstable();
                    l
                };
                assert_eq!(lengths(&mut pair.iter()), lengths(&mut want.into_iter()), "{selector} from {s} to {t}");
            }
        }
    }
}

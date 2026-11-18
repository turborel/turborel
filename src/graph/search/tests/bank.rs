//! The bank graph of [P1 Fig. 1], as [GS §23.1] gives it, and the path
//! queries T1-T12 of [GS §23.2] with their expected answers.

use std::collections::HashMap;
use std::sync::Arc;

use super::{filter, group, label, node, right, search, spelled, times, variables};
use crate::error::Code;
use crate::graph::in_memory::{InMemoryGraph, Property};
use crate::graph::program::{ElementTest, PathPattern, Pattern, Read, Restrictor, Selector};
use crate::graph::search::{PathSearch, SearchOptions};
use crate::graph::view::{ElementId, Orientation};

const ACCOUNT: u32 = 1;
const COUNTRY: u32 = 2;
const CITY: u32 = 3;
const PHONE: u32 = 4;
const TRANSFER: u32 = 5;
const LOCATED: u32 = 6;
const HAS_PHONE: u32 = 7;

/// The bank graph and each element's name.
pub(super) fn bank() -> (Arc<InMemoryGraph>, HashMap<ElementId, String>) {
    let mut g = InMemoryGraph::new();
    let mut names = HashMap::new();
    let accounts = [("Scott", "no"), ("Aretha", "no"), ("Mike", "no"), ("Jay", "yes"), ("Charles", "no"), ("Dave", "no")];
    let mut a = Vec::new();
    for (i, (owner, blocked)) in (1_u32..).zip(&accounts) {
        let id = g.vertex(0, &[ACCOUNT]);
        g.set(id, "owner", Property::Text((*owner).to_owned()));
        g.set(id, "is_blocked", Property::Text((*blocked).to_owned()));
        names.insert(id, format!("a{i}"));
        a.push(id);
    }
    let c1 = g.vertex(1, &[COUNTRY]);
    let c2 = g.vertex(1, &[CITY, COUNTRY]);
    names.insert(c1, "c1".to_owned());
    names.insert(c2, "c2".to_owned());
    let p1 = g.vertex(2, &[PHONE]);
    let p2 = g.vertex(2, &[PHONE]);
    names.insert(p1, "p1".to_owned());
    names.insert(p2, "p2".to_owned());
    // Accounts by their number less one: t1 is a1 to a3.
    let transfers = [(0, 2, 8), (2, 1, 10), (1, 3, 10), (3, 5, 10), (5, 2, 10), (5, 4, 4), (2, 4, 6), (4, 0, 9)];
    for (i, &(from, to, amount)) in (1_u32..).zip(&transfers) {
        let t = g.edge(10, (a[from], a[to]), true, &[TRANSFER]);
        g.set(t, "amount", Property::Int(amount));
        names.insert(t, format!("t{i}"));
    }
    for (i, &(from, to)) in (1_u32..).zip(&[(0, c1), (1, c2), (2, c1), (3, c2), (4, c1), (5, c2)]) {
        let li = g.edge(11, (a[from], to), true, &[LOCATED]);
        names.insert(li, format!("li{i}"));
    }
    for (i, &(phone, account)) in (1_u32..).zip(&[(p1, 0), (p1, 4), (p2, 1), (p2, 2)]) {
        let hp = g.edge(12, (phone, a[account]), false, &[HAS_PHONE]);
        names.insert(hp, format!("hp{i}"));
    }
    (Arc::new(g), names)
}

/// A vertex filter: property `name` is `value`; none if no name.
fn is(g: &Arc<InMemoryGraph>, name: &'static str, value: &'static str) -> Option<crate::graph::program::Filter> {
    let g = Arc::clone(g);
    (!name.is_empty()).then(|| filter(&[], move |b| Ok(g.text(b.element().unwrap(), name) == Some(value))))
}

/// T1's pattern under `restrictor` and `selector`: Dave's transfers to Aretha.
fn dave_to_aretha(g: &Arc<InMemoryGraph>, restrictor: Restrictor, selector: Selector) -> PathPattern {
    let pattern = Pattern::Concat(vec![
        node(Some(0), None, is(g, "owner", "Dave")),
        times(right(Some(2), label(TRANSFER), None), 0, None),
        node(Some(1), None, is(g, "owner", "Aretha")),
    ]);
    PathPattern::new(variables(&["a", "b"], &["t"]), pattern).restrict(restrictor).select(selector)
}

/// Verifies: [EXTENSION] [GS §23.2 T1, T2, T3] [P1 §5.1]: TRAIL gives
/// Dave's three trails to Aretha; ACYCLIC the first two, the third
/// repeating a3; ANY SHORTEST with no restrictor the one shortest.
#[test]
fn t1_t2_t3_restrictors_and_any_shortest() {
    let (g, names) = bank();
    let trails = spelled(&dave_to_aretha(&g, Restrictor::Trail, Selector::All), &g, &names).unwrap();
    assert_eq!(trails, ["a6,t5,a3,t2,a2", "a6,t5,a3,t7,a5,t8,a1,t1,a3,t2,a2", "a6,t6,a5,t8,a1,t1,a3,t2,a2"]);
    let acyclic = spelled(&dave_to_aretha(&g, Restrictor::Acyclic, Selector::All), &g, &names).unwrap();
    assert_eq!(acyclic, ["a6,t5,a3,t2,a2", "a6,t6,a5,t8,a1,t1,a3,t2,a2"]);
    let shortest = spelled(&dave_to_aretha(&g, Restrictor::Walk, Selector::AnyShortest), &g, &names).unwrap();
    assert_eq!(shortest, ["a6,t5,a3,t2,a2"]);
}

/// Verifies: [EXTENSION] [GS §23.2 T4] [GS §7.4 R7.3]: WALK with no
/// selector is refused, GRF-0031, as a Syntax Rule.
#[test]
fn t4_walk_without_selector_is_grf_0031() {
    let (g, _) = bank();
    // Refused as it compiles, so no search, which would not end, is run.
    let err = PathSearch::new(&dave_to_aretha(&g, Restrictor::Walk, Selector::All), g.as_ref(), SearchOptions::default()).err().unwrap();
    assert_eq!(err.code, Code::SyntaxRule);
    assert!(err.msg.contains("GRF-0031"), "{}", err.msg);
}

/// Verifies: [EXTENSION] [GS §23.2 T5, T6] [P1 §4.5]: path pattern union
/// dedups c2, City and Country; multiset alternation keeps it twice.
#[test]
fn t5_t6_union_dedups_and_alternation_keeps() {
    let (g, names) = bank();
    let city = || node(Some(0), label(CITY), None);
    let country = || node(Some(0), label(COUNTRY), None);
    let union = PathPattern::new(variables(&["c"], &[]), Pattern::Union(vec![city(), country()]));
    assert_eq!(spelled(&union, &g, &names).unwrap(), ["c1", "c2"]);
    let alternation = PathPattern::new(variables(&["c"], &[]), Pattern::Alternation(vec![city(), country()]));
    assert_eq!(spelled(&alternation, &g, &names).unwrap(), ["c1", "c2", "c2"]);
}

/// T7's pattern: Scott through a blocked account to Charles, the edges
/// passing `edge`, the blocked test a prefilter or, if `post`, not.
fn scott_to_charles(g: &Arc<InMemoryGraph>, edge: Option<&crate::graph::program::Filter>, post: bool) -> PathPattern {
    let step = || times(right(None, None, edge.cloned()), 1, None);
    let q = node(Some(1), label(ACCOUNT), if post { None } else { is(g, "is_blocked", "yes") });
    let pattern = Pattern::Concat(vec![
        node(Some(0), label(ACCOUNT), is(g, "owner", "Scott")),
        step(),
        q,
        step(),
        node(Some(2), label(ACCOUNT), is(g, "owner", "Charles")),
    ]);
    let path = PathPattern::new(variables(&["p", "q", "r"], &[]), pattern).select(Selector::AllShortest);
    if !post {
        return path;
    }
    let g = Arc::clone(g);
    path.postfilter(Arc::new(move |b| Ok(g.text(b.singleton(1).unwrap(), "is_blocked") == Some("yes"))))
}

/// Verifies: [EXTENSION] [GS §23.2 T7, T8] [GS §6.8 R6.4] [P1 §5.2]: a
/// prefilter is applied before ALL SHORTEST, a postfilter after.
///
/// T7 as printed gives (a1,t1,a3,t2,a2,t3,a4,t4,a6,t5,a3,t7,a5), of 6
/// edges, but the fixture's t6 (a6 to a5) makes
/// (a1,t1,a3,t2,a2,t3,a4,t4,a6,t6,a5), of 5, the shortest through a4: it
/// is what the pattern selects. With the edges kept to amounts over 5M,
/// dropping t6, the printed path is the one result. T8: the shortest
/// path, through a3, fails the postfilter, so none is left.
#[test]
fn t7_t8_prefilter_before_and_postfilter_after_selector() {
    let (g, names) = bank();
    assert_eq!(spelled(&scott_to_charles(&g, None, false), &g, &names).unwrap(), ["a1,t1,a3,t2,a2,t3,a4,t4,a6,t6,a5"]);
    let big = {
        let g = Arc::clone(&g);
        Some(filter(&[], move |b| Ok(g.int(b.element().unwrap(), "amount").is_some_and(|a| a > 5))))
    };
    let found = search(&scott_to_charles(&g, big.as_ref(), false), &g, &g.vertices(), None, SearchOptions::default()).unwrap();
    assert_eq!(found.iter().map(|p| super::spell(p, &names)).collect::<Vec<_>>(), ["a1,t1,a3,t2,a2,t3,a4,t4,a6,t5,a3,t7,a5"]);
    assert_eq!(names[&found[0].singleton(1).unwrap()], "a4", "q is bound to Jay's a4");
    assert_eq!(spelled(&scott_to_charles(&g, None, true), &g, &names).unwrap(), Vec::<String>::new());
}

/// T9's pattern, the location's branches joined by union, or by
/// multiset alternation if `multiset`.
fn jays_loops(g: &Arc<InMemoryGraph>, multiset: bool) -> PathPattern {
    let big = {
        let g = Arc::clone(g);
        Some(filter(&[], move |b| Ok(g.int(b.element().unwrap(), "amount").is_some_and(|a| a > 5))))
    };
    let located = |l| Pattern::Concat(vec![right(None, label(LOCATED), None), node(Some(1), label(l), None)]);
    let branches = vec![located(CITY), located(COUNTRY)];
    let pattern = Pattern::Concat(vec![
        node(Some(0), None, is(g, "owner", "Jay")),
        times(group(Restrictor::Walk, right(Some(2), label(TRANSFER), big), None), 1, None),
        node(Some(0), None, None),
        if multiset { Pattern::Alternation(branches) } else { Pattern::Union(branches) },
    ]);
    PathPattern::new(variables(&["a", "c"], &["b"]), pattern).restrict(Restrictor::Trail)
}

/// Verifies: [EXTENSION] [GS §23.2 T9] [P1 §6.1-§6.5]: the running
/// example gives two reduced bindings, loops of 4 and 7 back to Jay, both
/// ending at c2; with multiset alternation, four.
#[test]
fn t9_running_example_reduces_and_deduplicates() {
    let (g, names) = bank();
    let found = spelled(&jays_loops(&g, false), &g, &names).unwrap();
    assert_eq!(found, ["a4,t4,a6,t5,a3,t2,a2,t3,a4,li4,c2", "a4,t4,a6,t5,a3,t7,a5,t8,a1,t1,a3,t2,a2,t3,a4,li4,c2"]);
    let kept = spelled(&jays_loops(&g, true), &g, &names).unwrap();
    assert_eq!(kept.len(), 4);
    assert!(kept.iter().all(|p| p.ends_with("li4,c2")));
}

/// Verifies: [EXTENSION] [GS §23.2 T10] [GS §6.4]: a repeated variable
/// joins: two accounts sharing a phone, one transferring to the other.
#[test]
fn t10_repeated_variable_joins() {
    let (g, names) = bank();
    let phone = |var| Pattern::Edge(ElementTest { var, labels: label(HAS_PHONE), filter: None }, Orientation::UNDIRECTED);
    let pattern = Pattern::Concat(vec![
        node(Some(0), label(PHONE), None),
        phone(None),
        node(Some(1), label(ACCOUNT), None),
        right(Some(3), label(TRANSFER), None),
        node(Some(2), label(ACCOUNT), None),
        phone(None),
        node(Some(0), None, None),
    ]);
    let path = PathPattern::new(variables(&["p", "s", "d"], &["t"]), pattern);
    let found = search(&path, &g, &g.vertices(), None, SearchOptions::default()).unwrap();
    let mut rows: Vec<String> =
        found.iter().map(|b| [0, 1, 3, 2].iter().map(|&v| names[&b.singleton(v).unwrap()].clone()).collect::<Vec<_>>().join(",")).collect();
    rows.sort();
    assert_eq!(rows, ["p1,a5,t8,a1", "p2,a3,t2,a2"]);
}

/// Verifies: [EXTENSION] [GS §23.2 T11] [GS §6.3 R6.1]: an edge pattern of
/// any orientation matches a directed edge once each way.
#[test]
fn t11_any_direction_matches_each_way() {
    let mut g = InMemoryGraph::new();
    let x = g.vertex(0, &[]);
    let y = g.vertex(0, &[]);
    g.edge(1, (x, y), true, &[]);
    let pattern = Pattern::Concat(vec![
        node(Some(0), None, None),
        Pattern::Edge(ElementTest { var: Some(2), labels: None, filter: None }, Orientation::ANY),
        node(Some(1), None, None),
    ]);
    let path = PathPattern::new(variables(&["x", "y"], &["e"]), pattern);
    let found = search(&path, &g, &g.vertices(), None, SearchOptions::default()).unwrap();
    let ends: Vec<(ElementId, ElementId)> = found.iter().map(|b| (b.singleton(0).unwrap(), b.singleton(1).unwrap())).collect();
    assert_eq!(ends, [(x, y), (y, x)]);
}

/// Verifies: [EXTENSION] [GS §23.2 T12] [GS §7.4 R7.4] [P1 §5.3]: an
/// aggregate of an unbounded group in a prefilter is GRF-0032; as a
/// postfilter the query is allowed and gives nothing.
#[test]
fn t12_aggregate_of_unbounded_group_in_prefilter() {
    let (g, names) = bank();
    let never = |count: usize| count == usize::MAX;
    let body = Pattern::Concat(vec![node(Some(0), None, None), times(right(Some(2), None, None), 0, None), node(Some(1), None, None)]);
    let bracketed =
        group(Restrictor::Walk, body.clone(), Some(filter(&[Read { var: 2, previous: false }], move |b| Ok(never(b.group(2).len())))));
    let err = search(
        &PathPattern::new(variables(&["x", "y"], &["e"]), bracketed).select(Selector::AllShortest),
        &g,
        &g.vertices(),
        None,
        SearchOptions::default(),
    )
    .unwrap_err();
    assert_eq!(err.code, Code::SyntaxRule);
    assert!(err.msg.contains("GRF-0032"), "{}", err.msg);
    let post = PathPattern::new(variables(&["x", "y"], &["e"]), body)
        .select(Selector::AllShortest)
        .postfilter(Arc::new(move |b| Ok(never(b.group(2).len()))));
    assert_eq!(spelled(&post, &g, &names).unwrap(), Vec::<String>::new());
}

/// Verifies: [EXTENSION] [P1 §5.3] the fixes P1 offers for T12's
/// prefilter: a restrictor in its brackets, or a bounded quantifier, so
/// the group read is bounded; and [P1 §4.4] a group's aggregate in a
/// prefilter reads the elements of its own match.
#[test]
fn bounded_group_aggregates_in_prefilters() {
    let (g, names) = bank();
    let body = |max| {
        Pattern::Concat(vec![
            node(Some(0), label(ACCOUNT), None),
            times(right(Some(2), label(TRANSFER), None), 0, max),
            node(Some(1), None, None),
        ])
    };
    let two = || Some(filter(&[Read { var: 2, previous: false }], |b| Ok(b.group(2).len() == 2)));
    let trail = group(Restrictor::Trail, body(None), two());
    let found = spelled(&PathPattern::new(variables(&["x", "y"], &["e"]), trail).select(Selector::AllShortest), &g, &names).unwrap();
    assert_eq!(found.len(), 11, "the two-transfer walks: {found:?}");
    assert!(found.iter().all(|p| p.matches(',').count() == 4));
    let bounded = group(Restrictor::Walk, body(Some(3)), two());
    let again = spelled(&PathPattern::new(variables(&["x", "y"], &["e"]), bounded).select(Selector::AllShortest), &g, &names).unwrap();
    assert_eq!(found, again);
}

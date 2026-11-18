use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;

use super::landmarks::Landmarks;
use super::*;
use crate::memory::MemoryBudget;
use crate::value::Spelled;

/// An INTEGER column.
fn int() -> DataType {
    DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric }
}

fn n(v: u64) -> Value {
    Value::Exact(Decimal::new(i128::from(v), 0))
}

/// The key of vertex `v`.
fn vertex(v: u64) -> Key {
    Key(vec![n(v)])
}

/// The next of a seeded sequence of pseudo-random numbers.
fn next(state: &mut u64) -> u64 {
    *state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
    *state >> 33
}

/// `count` seeded random edges among `vertices` vertices.
fn random_edges(seed: u64, vertices: u64, count: usize) -> Vec<(u64, u64)> {
    let mut state = seed;
    (0..count).map(|_| (next(&mut state).checked_rem(vertices).unwrap(), next(&mut state).checked_rem(vertices).unwrap())).collect()
}

/// Whether `s` reaches `t` by `edges`, by a search of them all.
fn searched(edges: &[(u64, u64)], s: u64, t: u64) -> bool {
    let mut seen = HashSet::from([s]);
    let mut stack = vec![s];
    while let Some(v) = stack.pop() {
        if v == t {
            return true;
        }
        stack.extend(edges.iter().filter(|(a, _)| *a == v).map(|(_, b)| *b).filter(|b| seen.insert(*b)));
    }
    false
}

/// The bytes of vertex `v`'s key.
fn bytes(v: u64) -> Vec<u8> {
    let mut out = Vec::new();
    encode_key(&[n(v)], &[int()], &mut out).unwrap();
    out
}

/// Checks `made` answers every pair among `vertices` as a search of `edges` does.
fn exact(made: &Landmarks, edges: &[(u64, u64)], vertices: u64) {
    for s in 0..vertices {
        for t in 0..vertices {
            let want = searched(edges, s, t);
            let got = made.reaching(&[Some(bytes(s))], &[bytes(t)], false) == [true];
            assert_eq!(got, want, "{s} reaches {t} by {edges:?}");
            let back = made.reaching(&[Some(bytes(t))], &[bytes(s)], true) == [true];
            assert_eq!(back, want, "{t} is reached from {s} backward");
        }
    }
}

/// An index of key (source, destination) of `method`.
fn index(method: &ReachabilityMethod) -> Arc<dyn Index> {
    Arc::from(method.create(&[int(), int()]).unwrap())
}

/// Verifies: [DESIGN] RULES.md D116: labels made of every edge at once
/// answer each pair as a search of the edges does, on seeded random
/// graphs, cycles and self-loops among them (normal range).
#[test]
fn labels_made_at_once_are_exact() {
    for seed in 1..=6 {
        let edges = random_edges(seed, 9, 14);
        let keyed: Vec<_> = edges.iter().map(|&(a, b)| (bytes(a), bytes(b))).collect();
        exact(&Landmarks::made(&keyed), &edges, 9);
    }
}

/// Verifies: [DESIGN] RULES.md D116: labels kept as edges are added one
/// by one stay exact after each, whether the edge joins new pairs or
/// not, each search pruned at what was reached before (normal range).
#[test]
fn labels_kept_as_edges_are_added_stay_exact() {
    for seed in 7..=40 {
        let edges = random_edges(seed, 10, 18);
        let (first, rest) = edges.split_at(usize::try_from(seed.checked_rem(5).unwrap()).unwrap());
        let keyed: Vec<_> = first.iter().map(|&(a, b)| (bytes(a), bytes(b))).collect();
        let mut made = Landmarks::made(&keyed);
        for (k, &(a, b)) in rest.iter().enumerate() {
            made.insert(&bytes(a), &bytes(b));
            exact(&made, &edges[..first.len().saturating_add(k).saturating_add(1)], 10);
        }
    }
}

/// Verifies: [DESIGN] RULES.md D116: a chain added edge by edge from
/// both ends, the case where a new edge's backward and forward searches
/// both reach far, reaches end to end, and not back (normal range).
#[test]
fn a_chain_joined_in_the_middle_reaches_end_to_end() {
    let mut made = Landmarks::made(&[]);
    for (a, b) in [(0, 1), (1, 2), (4, 5), (5, 6), (2, 3), (3, 4)] {
        made.insert(&bytes(a), &bytes(b));
    }
    let edges = [(0, 1), (1, 2), (2, 3), (3, 4), (4, 5), (5, 6)];
    exact(&made, &edges, 7);
}

/// Verifies: [DESIGN] RULES.md D116: of several starts and ends at once,
/// each start reaches an end if one of them is reached, or it is one; a
/// start no edge touches reaches only itself, and one of no key reaches
/// (normal range, boundaries).
#[test]
fn reaching_answers_each_start_of_several_ends() {
    let edges = [(1, 2), (2, 3), (5, 6)];
    let keyed: Vec<_> = edges.iter().map(|&(a, b)| (bytes(a), bytes(b))).collect();
    let made = Landmarks::made(&keyed);
    let starts = [Some(bytes(1)), Some(bytes(5)), Some(bytes(9)), Some(bytes(6)), None];
    assert_eq!(made.reaching(&starts, &[bytes(3), bytes(6)], false), [true, true, false, true, true]);
    assert_eq!(made.reaching(&starts, &[bytes(9)], false), [false, false, true, false, true]);
    assert_eq!(made.reaching(&starts, &[], false), [false, false, false, false, true]);
}

/// Verifies: [DESIGN] RULES.md D116, D34: the index answers through
/// `Reachability`; a version of an edge taken back while another holds
/// it changes nothing, the last one drops the labels, so it answers
/// nothing until maintenance makes them again, exact (normal range).
#[test]
fn a_removal_drops_the_labels_until_maintenance() {
    let method = ReachabilityMethod::in_memory();
    let ix = index(&method);
    let reach = Arc::clone(&ix).reachability().unwrap();
    for (a, b, row) in [(1, 2, 10), (2, 3, 11), (2, 3, 11)] {
        ix.insert(Key(vec![n(a), n(b)]), row).unwrap();
    }
    assert_eq!(reach.reaching(&[vertex(1)], &[vertex(3)], false).unwrap(), Some(vec![true]));
    ix.insert(Key(vec![n(3), n(4)]), 12).unwrap();
    assert_eq!(reach.reaching(&[vertex(1), vertex(4)], &[vertex(4)], false).unwrap(), Some(vec![true, true]));
    ix.remove(&Key(vec![n(2), n(3)]), 11).unwrap();
    assert!(!ix.maintenance_due());
    assert_eq!(reach.reaching(&[vertex(1)], &[vertex(4)], false).unwrap(), Some(vec![true]));
    ix.remove(&Key(vec![n(2), n(3)]), 11).unwrap();
    assert!(ix.maintenance_due());
    assert_eq!(reach.reaching(&[vertex(1)], &[vertex(4)], false).unwrap(), None);
    assert!(ix.maintain().unwrap());
    assert!(!ix.maintenance_due());
    assert_eq!(reach.reaching(&[vertex(1), vertex(3)], &[vertex(4)], false).unwrap(), Some(vec![false, true]));
}

/// Verifies: [DESIGN] RULES.md D116, D25: the index reopened of its
/// locator holds its entries, given by a search of them all, and makes
/// its labels of them (normal range).
#[test]
fn an_index_reopened_of_its_locator_holds_its_edges() {
    let method = ReachabilityMethod::in_memory();
    let ix = index(&method);
    for (a, b, row) in [(1, 2, 10), (2, 3, 11)] {
        ix.insert(Key(vec![n(a), n(b)]), row).unwrap();
    }
    let opened: Arc<dyn Index> = Arc::from(method.open(&[int(), int()], &ix.locator()).unwrap().unwrap());
    let all: BTreeSet<RowId> = Arc::clone(&opened).search(&Probe::All, KeyOrder::Ascending).map(|e| e.unwrap().1).collect();
    assert_eq!(all, BTreeSet::from([10, 11]));
    assert_eq!(opened.rows(&Key(vec![n(2), n(3)])).unwrap(), vec![11]);
    let reach = opened.reachability().unwrap();
    assert_eq!(reach.reaching(&[vertex(1), vertex(3)], &[vertex(3)], false).unwrap(), Some(vec![true, true]));
    assert!(method.open(&[int(), int()], &[9]).is_err(), "a locator of another format");
}

/// Verifies: [DESIGN] RULES.md D116, D38: labels the memory budget
/// refuses are not made, so the index answers nothing; a key whose ends
/// differ in type is refused (robustness).
#[test]
fn refused_labels_and_layouts_answer_nothing() {
    let method = ReachabilityMethod::in_memory().with_budget(MemoryBudget::new(1024));
    let ix = index(&method);
    ix.insert(Key(vec![n(1), n(2)]), 10).unwrap();
    let reach = Arc::clone(&ix).reachability().unwrap();
    assert_eq!(reach.reaching(&[vertex(1)], &[vertex(2)], false).unwrap(), None);
    assert!(!ix.maintenance_due(), "not until as many edges are added");
    let wide = DataType::Exact { precision: 18, scale: 0, spelled: Spelled::Numeric };
    assert_eq!(method.create(&[int(), wide]).err().unwrap().code, Code::Unsupported);
    assert_eq!(method.create(&[int()]).err().unwrap().code, Code::Unsupported);
}

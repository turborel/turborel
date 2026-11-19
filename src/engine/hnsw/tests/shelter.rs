//! The guard that keeps every node's last incoming edge on layer 0
//! (RULES.md D97), term by term: `Graph::shelter`, the new node's link
//! when no candidate can give up its place, repair's guard of live nodes
//! alone, and compaction's relinking of a node no path reaches.

use super::*;
use crate::engine::hnsw::graph::{Graph, Store};
use crate::engine::hnsw::insert::Build;
use crate::engine::hnsw::scratch::{Scored, Scratch};

/// A graph of one dimension, M 1 and M0 2, a node at each of `xs` on
/// layer 0, a tombstone where its flag is false, its entry node 0.
fn line(xs: &[(f32, bool)]) -> Graph {
    let mut graph = Graph::new(1, (1, 2), Store::Full(Vec::new()));
    for (i, (x, live)) in xs.iter().enumerate() {
        graph.push((row(i), row(i), u32::from(*live)), 0, &[*x]);
    }
    graph.entry = 0;
    graph
}

/// `n` live nodes of `line`, at 0, 1, 2 and so on.
fn live(n: u16) -> Graph {
    line(&(0..n).map(|x| (f32::from(x), true)).collect::<Vec<_>>())
}

/// Verifies: [DESIGN] RULES.md D97: a candidate the choice keeps is not
/// held again, though no other list leads to it. MC/DC of `shelter`'s
/// first condition, the candidate among those chosen: with it false,
/// the candidates kept would be added again, or left with no place.
#[test]
fn a_candidate_kept_is_not_held_again() {
    let mut graph = live(3);
    graph.set_links(0, 0, &[1, 2]);
    let mut ids = vec![1, 2];
    assert_eq!(graph.shelter((0, 0, None), &[1, 2], false, &mut ids), (false, vec![]));
    assert_eq!(ids, [1, 2]);
}

/// Verifies: [DESIGN] RULES.md D97: a candidate let go of that another
/// list leads to is let go of. MC/DC of `shelter`'s second condition,
/// another edge into the candidate: with it false, the candidate would
/// be held with no place left for it.
#[test]
fn a_candidate_another_list_leads_to_is_let_go() {
    let mut graph = live(5);
    graph.set_links(0, 0, &[1, 2]);
    graph.set_links(3, 0, &[1]);
    let mut ids = vec![2, 4];
    assert_eq!(graph.shelter((0, 0, Some(4)), &[1, 2], false, &mut ids), (false, vec![]));
    assert_eq!(ids, [2, 4]);
}

/// Verifies: [DESIGN] RULES.md D97: a candidate let go of that no other
/// list leads to stays, in place of the farthest chosen that another
/// list leads to; an edge from its own neighbour does not count.
#[test]
fn a_last_incoming_edge_is_kept_in_place_of_the_farthest() {
    let mut graph = live(5);
    graph.set_links(0, 0, &[1, 2]);
    graph.set_links(3, 0, &[2, 4]);
    let mut ids = vec![2, 4];
    assert_eq!(graph.shelter((0, 0, Some(4)), &[1, 2], false, &mut ids), (true, vec![]));
    assert_eq!(ids, [2, 1]);
    graph.set_links(1, 0, &[3]);
    graph.set_links(3, 0, &[1, 4]);
    let mut paired = vec![2, 4];
    assert_eq!(graph.shelter((0, 0, Some(4)), &[1, 2], false, &mut paired), (true, vec![]));
    assert_eq!(paired, [2, 1], "a pair holding only each other is not cut off");
}

/// Verifies: [DESIGN] RULES.md D97: a back edge's reselection holds a
/// tombstone too, a waypoint until repair. MC/DC of `shelter`'s third
/// condition, `live_only`: with it true, the tombstone would be let go.
#[test]
fn a_back_edge_holds_a_tombstone() {
    let mut graph = line(&[(0.0, true), (1.0, false), (2.0, true), (3.0, true)]);
    graph.set_links(0, 0, &[1, 2]);
    graph.set_links(3, 0, &[2]);
    let mut ids = vec![2];
    assert_eq!(graph.shelter((0, 0, None), &[1, 2], false, &mut ids), (true, vec![]));
    assert_eq!(ids, [2, 1]);
}

/// Verifies: [DESIGN] RULES.md D97: repair's guard holds live nodes
/// alone, letting go of a tombstone. MC/DC of `shelter`'s fourth
/// condition, the candidate a tombstone: with it false, the live node
/// would be let go, and with it true for every candidate, both would.
#[test]
fn repair_holds_live_nodes_alone() {
    let mut graph = line(&[(0.0, true), (1.0, false), (2.0, true), (3.0, true)]);
    graph.set_links(0, 0, &[1, 2]);
    graph.set_links(3, 0, &[]);
    let mut ids = vec![3];
    assert_eq!(graph.shelter((0, 0, None), &[1, 2], true, &mut ids), (true, vec![]));
    assert_eq!(ids, [3, 2]);
}

/// Verifies: [DESIGN] RULES.md D97: when no candidate chosen can give
/// up its place, the new node links to the one let go of, in place of
/// the last of its list another list leads to.
#[test]
fn the_new_node_links_to_a_node_no_place_was_left_for() {
    let mut graph = live(7);
    graph.set_links(0, 0, &[1, 2]);
    graph.set_links(3, 0, &[4, 5]);
    graph.set_links(6, 0, &[5]);
    let mut s =
        Scratch { before: vec![1, 2], chosen: [1, 3].iter().map(|id| Scored { distance: 0.0, id: *id }).collect(), ..Scratch::default() };
    assert!(graph.apply((0, 0, 3), &mut s));
    assert_eq!(graph.links(0, 0), [1, 3]);
    assert_eq!(graph.links(3, 0), [4, 2]);
    assert_eq!((graph.incoming(2), graph.incoming(5)), (1, 1));
}

/// Verifies: [DESIGN] RULES.md D97: repair's relink keeps a live node
/// its last incoming edge, which the heuristic lets go of, and lets go
/// of the tombstone.
#[test]
fn repair_keeps_a_live_nodes_last_incoming_edge() {
    let mut graph = line(&[(0.0, true), (1.0, false), (5.0, true), (1.0, true), (-1.0, true)]);
    graph.set_links(0, 0, &[1, 2]);
    graph.set_links(1, 0, &[3, 4]);
    let build = Build { ef_construction: 4, extend: false, keep_pruned: true, m: 1, m0: 2 };
    graph.repair(&euclidean(), &build, &mut Scratch::default());
    let list = graph.links(0, 0);
    assert!(list.contains(&2) && !list.contains(&1), "{list:?}");
}

/// Verifies: [DESIGN] RULES.md D97: a node reached only through a
/// tombstone, cut off as compaction drops the tombstone's edges, is
/// linked again from a neighbour it links to.
#[test]
fn compaction_links_again_a_node_cut_off() {
    let mut graph = line(&[(0.0, true), (1.0, false), (2.0, true)]);
    graph.set_links(0, 0, &[1]);
    graph.set_links(1, 0, &[2]);
    graph.set_links(2, 0, &[0]);
    let out = graph.compacted();
    assert_eq!(out.links(0, 0), [1]);
    assert_eq!(out.incoming(1), 1);
}

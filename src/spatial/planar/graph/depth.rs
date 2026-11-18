//! \[DESIGN\] Area depths of the graph's faces. Within a connected
//! component, depths follow from crossing edges. The face on a
//! half-edge's left is deeper than the one on its right by the edge's
//! depth change. The face around a component is located once, by a ray
//! from the component's least node toward positive x, summing the
//! signed depth changes of the other components' edges it crosses. A
//! block index over the edges' y-ranges finds the edges a ray can cross.

use std::cmp::Ordering;
use std::collections::VecDeque;

use super::{Graph, INPUTS, edge_of, twin};
use crate::spatial::planar::Point;
use crate::spatial::planar::predicates::orient2d;
use crate::spatial::planar::segment::{compare, lexicographic};

/// Edges per block of the y-range index. Blocks whose greatest upper
/// bound lies below a ray are skipped whole.
const BLOCK: usize = 64;

pub(super) fn assign(graph: &mut Graph) {
    let (component, count) = components(graph);
    graph.component = component;
    let least = least_nodes(graph, count);
    let index = Stabbing::new(graph);
    graph.outside =
        least.iter().enumerate().map(|(id, node)| node.map_or([0_i32; INPUTS], |node| index.winding(graph, id, node))).collect();
    propagate(graph);
}

fn find(parent: &mut [usize], mut item: usize) -> usize {
    while let Some(next) = parent.get(item).copied() {
        if next == item {
            break;
        }
        let grand = parent.get(next).copied().unwrap_or(next);
        if let Some(entry) = parent.get_mut(item) {
            *entry = grand;
        }
        item = grand;
    }
    item
}

/// Each node's component, numbered densely in node order, and the count.
fn components(graph: &Graph) -> (Vec<usize>, usize) {
    let mut parent: Vec<usize> = (0..graph.nodes.len()).collect();
    for edge in &graph.edges {
        let [low, high] = edge.ends;
        let (a, b) = (find(&mut parent, low), find(&mut parent, high));
        if let Some(entry) = parent.get_mut(a.max(b)) {
            *entry = a.min(b);
        }
    }
    let mut number = vec![usize::MAX; graph.nodes.len()];
    let mut out = Vec::with_capacity(graph.nodes.len());
    let mut count = 0_usize;
    for node in 0..graph.nodes.len() {
        let root = find(&mut parent, node);
        let id = match number.get(root).copied() {
            Some(usize::MAX) | None => {
                if let Some(entry) = number.get_mut(root) {
                    *entry = count;
                }
                count = count.saturating_add(1);
                count.saturating_sub(1)
            }
            Some(id) => id,
        };
        out.push(id);
    }
    (out, count)
}

fn least_nodes(graph: &Graph, count: usize) -> Vec<Option<usize>> {
    let mut least: Vec<Option<usize>> = vec![None; count];
    for (node, component) in graph.component.iter().enumerate() {
        let Some(entry) = least.get_mut(*component) else { continue };
        let better = entry.is_none_or(|current| lexicographic(graph.at(node), graph.at(current)) == Ordering::Less);
        if better {
            *entry = Some(node);
        }
    }
    least
}

/// Edges with a depth change, sorted by lower y, in blocks with their
/// greatest upper y.
struct Stabbing {
    edges: Vec<(f64, f64, usize)>,
    tops: Vec<f64>,
}

impl Stabbing {
    fn new(graph: &Graph) -> Self {
        let mut edges: Vec<(f64, f64, usize)> = Vec::new();
        for (id, edge) in graph.edges.iter().enumerate() {
            if edge.delta.iter().all(|delta| *delta == 0_i32) {
                continue;
            }
            let [low, high] = edge.ends;
            let ([_, ay], [_, by]) = (graph.at(low), graph.at(high));
            edges.push((ay.min(by), ay.max(by), id));
        }
        edges.sort_by(|left, right| compare(left.0, right.0));
        let tops = edges.chunks(BLOCK).map(|block| block.iter().map(|edge| edge.1).fold(f64::NEG_INFINITY, f64::max)).collect();
        Self { edges, tops }
    }

    /// The depth, per operand, of the face around `component`, seen from
    /// its least node. Other components' edges crossed by the half-open
    /// ray to positive x each add the depth change toward the node.
    fn winding(&self, graph: &Graph, component: usize, node: usize) -> [i32; INPUTS] {
        let point = graph.at(node);
        let [_, y] = point;
        let mut depth = [0_i32; INPUTS];
        for (block, top) in self.edges.chunks(BLOCK).zip(&self.tops) {
            if block.first().is_some_and(|first| first.0 > y) {
                break;
            }
            if *top < y {
                continue;
            }
            for (_, _, id) in block {
                let Some(edge) = graph.edges.get(*id) else { continue };
                let [low, _] = edge.ends;
                if graph.component.get(low) == Some(&component) {
                    continue;
                }
                add_crossing(graph, *id, point, &mut depth);
            }
        }
        depth
    }
}

/// An edge from `a` to `b` crosses the ray from `point` if one end lies
/// strictly above the ray's line and the other on or below it, and the
/// point lies left of the edge taken upward. Then the point is on the
/// edge's left when it runs upward, so it adds the edge's depth change,
/// and on its right when it runs downward, so it subtracts it.
fn add_crossing(graph: &Graph, id: usize, point: Point, depth: &mut [i32; INPUTS]) {
    let Some(edge) = graph.edges.get(id) else { return };
    let [low, high] = edge.ends;
    let (a, b) = (graph.at(low), graph.at(high));
    let ([_, ay], [_, by], [_, y]) = (a, b, point);
    if (ay > y) == (by > y) {
        return;
    }
    let upward = by > ay;
    let (bottom, top) = if upward { (a, b) } else { (b, a) };
    if orient2d(bottom, top, point) != Ordering::Greater {
        return;
    }
    for (total, delta) in depth.iter_mut().zip(edge.delta) {
        *total = if upward { total.saturating_add(delta) } else { total.saturating_sub(delta) };
    }
}

/// Sets each component's outer cycle to the depth around it, then walks
/// across edges. The cycle on a half-edge's right is shallower than the
/// one on its left by the half-edge's depth change.
fn propagate(graph: &mut Graph) {
    let mut known = vec![false; graph.cycles.len()];
    let mut queue = VecDeque::new();
    for (id, cycle) in graph.cycles.iter_mut().enumerate() {
        if !cycle.outer {
            continue;
        }
        let Some(first) = cycle.halves.first().copied() else { continue };
        let [low, _] = graph.edges.get(edge_of(first)).map_or([0, 0], |edge| edge.ends);
        let component = graph.component.get(low).copied().unwrap_or_default();
        cycle.depth = graph.outside.get(component).copied().unwrap_or_default();
        if let Some(entry) = known.get_mut(id) {
            *entry = true;
        }
        queue.push_back(id);
    }
    while let Some(id) = queue.pop_front() {
        let Some(cycle) = graph.cycles.get(id) else { continue };
        let depth = cycle.depth;
        let crossings: Vec<(usize, [i32; INPUTS])> = cycle
            .halves
            .iter()
            .map(|half| {
                let other = graph.face.get(twin(*half)).copied().unwrap_or_default();
                let mut next = depth;
                for (input, value) in next.iter_mut().enumerate() {
                    *value = value.saturating_sub(graph.delta(*half, input));
                }
                (other, next)
            })
            .collect();
        for (other, next) in crossings {
            if known.get(other).is_some_and(|seen| !seen) {
                if let Some(entry) = known.get_mut(other) {
                    *entry = true;
                }
                if let Some(reached) = graph.cycles.get_mut(other) {
                    reached.depth = next;
                }
                queue.push_back(other);
            }
        }
    }
}

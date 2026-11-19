//! \[DESIGN\] Builds a result geometry from a selection of the graph's
//! faces, edges and nodes. Surfaces come from the selected faces. Each
//! edge with a selected face on one side only is boundary, traced with
//! the selected side on its left, turning as sharply left as possible
//! at each node, so exterior rings that touch at a node stay separate.
//! A traced walk that passes a node twice is split there into simple
//! loops, so an interior ring touching its exterior ring is two rings.
//! Counterclockwise rings are exterior rings; each clockwise ring is an
//! interior ring of the innermost exterior ring containing it. Curves
//! are the selected edges merged through nodes where exactly two meet;
//! points are the selected nodes.

use std::cmp::Ordering;
use std::collections::HashMap;

use super::Point;
use super::chain::{encloses, envelope};
use super::graph::{Graph, edge_of, twin};
use super::predicates::orient2d;
use super::ring::contains;
use super::segment::lexicographic;
use super::shape::{Geometry, Parts};
use crate::error::{Code, Result, SqlError};

/// What a result keeps. Per cycle it says whether the face is in the
/// result, per edge whether it is a curve of the result, and per node
/// whether it is an isolated point of the result.
pub(crate) struct Selection {
    pub(crate) faces: Vec<bool>,
    pub(crate) lines: Vec<bool>,
    pub(crate) points: Vec<bool>,
}

impl Selection {
    fn face(&self, graph: &Graph, half: usize) -> bool {
        graph.face.get(half).and_then(|cycle| self.faces.get(*cycle)).copied().unwrap_or_default()
    }

    /// A half-edge with the result's area on its left and not its right.
    fn boundary(&self, graph: &Graph, half: usize) -> bool {
        self.face(graph, half) && !self.face(graph, twin(half))
    }

    /// Whether a node touches a selected face or curve.
    pub(crate) fn covers(&self, graph: &Graph, node: usize) -> bool {
        graph.star.get(node).is_some_and(|star| {
            star.iter().any(|half| self.face(graph, *half) || self.lines.get(edge_of(*half)).copied().unwrap_or_default())
        })
    }
}

/// The geometry of the selection.
///
/// # Errors
///
/// SQLCODE -910 if the selected faces' boundary does not close into
/// rings with an exterior ring around every interior ring.
pub(crate) fn assemble(graph: &Graph, selection: &Selection) -> Result<Geometry> {
    let polygons = polygons(graph, selection)?;
    let lines = lines(graph, selection);
    let points = graph.nodes.iter().zip(&selection.points).filter(|(_, kept)| **kept).map(|(node, _)| node.at).collect();
    Ok(Geometry::from_parts(Parts { points, lines, polygons }))
}

fn broken() -> SqlError {
    SqlError::new(Code::Api, "[DESIGN] a result's boundary does not form rings")
}

/// Traces every boundary ring as half-edges.
fn rings(graph: &Graph, selection: &Selection) -> Result<Vec<Vec<usize>>> {
    let halves = graph.edges.len().saturating_mul(2);
    let mut seen = vec![false; halves];
    let mut rings = Vec::new();
    for start in 0..halves {
        if seen.get(start).copied().unwrap_or(true) || !selection.boundary(graph, start) {
            continue;
        }
        let mut ring = Vec::new();
        let mut half = start;
        loop {
            if seen.get(half).copied().unwrap_or(true) || ring.len() > halves {
                return Err(broken());
            }
            if let Some(entry) = seen.get_mut(half) {
                *entry = true;
            }
            ring.push(half);
            half = next_boundary(graph, selection, half).ok_or_else(broken)?;
            if half == start {
                break;
            }
        }
        rings.extend(loops(graph, &ring));
    }
    Ok(rings)
}

/// Splits a closed walk into simple loops at the nodes it passes more
/// than once. Each return to a node on the open path closes the loop
/// since that node's last visit.
fn loops(graph: &Graph, walk: &[usize]) -> Vec<Vec<usize>> {
    let mut found = Vec::new();
    let mut path: Vec<usize> = Vec::with_capacity(walk.len());
    let mut open: HashMap<usize, usize> = HashMap::new();
    for half in walk {
        let origin = graph.origin(*half);
        if let Some(start) = open.get(&origin).copied() {
            let closed: Vec<usize> = path.drain(start..).collect();
            for member in &closed {
                open.remove(&graph.origin(*member));
            }
            found.push(closed);
        }
        open.insert(origin, path.len());
        path.push(*half);
    }
    if !path.is_empty() {
        found.push(path);
    }
    found
}

/// At a half-edge's head, the first boundary half-edge clockwise from
/// the way back. The selected area on the left is the sector swept.
fn next_boundary(graph: &Graph, selection: &Selection, half: usize) -> Option<usize> {
    let star = graph.star.get(graph.head(half))?;
    let back = graph.slot(twin(half));
    let count = star.len();
    (1..=count)
        .map(|step| back.saturating_add(count).saturating_sub(step).checked_rem(count).unwrap_or_default())
        .find_map(|position| star.get(position).copied().filter(|candidate| selection.boundary(graph, *candidate)))
}

/// A ring is an interior ring when, at its least vertex, the area on its
/// left is the sector holding the direction of negative x. Both
/// directions there point into the right half-plane, so the sector from
/// the outgoing to the incoming direction wraps through negative x
/// exactly when the incoming direction turns counterclockwise to the
/// outgoing one.
fn is_hole(graph: &Graph, ring: &[usize]) -> bool {
    let Some(least) = ring.iter().map(|half| graph.origin(*half)).min_by(|left, right| lexicographic(graph.at(*left), graph.at(*right)))
    else {
        return false;
    };
    let centre = graph.at(least);
    let count = ring.len();
    ring.iter().enumerate().any(|(position, out)| {
        let incoming = ring.get(position.checked_sub(1).unwrap_or(count.saturating_sub(1))).copied().unwrap_or(*out);
        graph.origin(*out) == least && orient2d(centre, graph.at(graph.origin(incoming)), graph.at(graph.head(*out))) == Ordering::Greater
    })
}

fn points(graph: &Graph, ring: &[usize]) -> Vec<Point> {
    let mut out: Vec<Point> = ring.iter().map(|half| graph.at(graph.origin(*half))).collect();
    out.extend(out.first().copied());
    out
}

fn polygons(graph: &Graph, selection: &Selection) -> Result<Vec<Vec<Vec<Point>>>> {
    let mut shells: Vec<Vec<Vec<Point>>> = Vec::new();
    let mut holes: Vec<Vec<Point>> = Vec::new();
    for ring in rings(graph, selection)? {
        if is_hole(graph, &ring) {
            holes.push(points(graph, &ring));
        } else {
            shells.push(vec![points(graph, &ring)]);
        }
    }
    let envelopes: Vec<_> = shells.iter().map(|shell| envelope(shell.first().map_or(&[][..], Vec::as_slice))).collect();
    for hole in holes {
        let bounds = envelope(&hole);
        let mut best: Option<usize> = None;
        for (index, shell) in shells.iter().enumerate() {
            let exterior = shell.first().map_or(&[][..], Vec::as_slice);
            if !envelopes.get(index).is_some_and(|outer| encloses(*outer, bounds)) || !contains(exterior, &hole) {
                continue;
            }
            let inner = best.and_then(|current| shells.get(current)).and_then(|current| current.first());
            if inner.is_none_or(|current| contains(current, exterior)) {
                best = Some(index);
            }
        }
        shells.get_mut(best.ok_or_else(broken)?).ok_or_else(broken)?.push(hole);
    }
    Ok(shells)
}

/// The selected edges as line strings, merged where exactly two meet.
fn lines(graph: &Graph, selection: &Selection) -> Vec<Vec<Point>> {
    let mut at_node: Vec<Vec<usize>> = vec![Vec::new(); graph.nodes.len()];
    for (id, edge) in graph.edges.iter().enumerate().filter(|(id, _)| selection.lines.get(*id).copied().unwrap_or_default()) {
        for end in edge.ends {
            if let Some(list) = at_node.get_mut(end) {
                list.push(id);
            }
        }
    }
    let mut used = vec![false; graph.edges.len()];
    let mut out = Vec::new();
    let through = |node: usize| at_node.get(node).is_some_and(|list| list.len() == 2);
    for (node, list) in at_node.iter().enumerate().filter(|(node, _)| !through(*node)) {
        for edge in list {
            if !used.get(*edge).copied().unwrap_or(true) {
                out.push(walk(graph, &at_node, &mut used, (node, *edge)));
            }
        }
    }
    for edge in 0..graph.edges.len() {
        if selection.lines.get(edge).copied().unwrap_or_default() && !used.get(edge).copied().unwrap_or(true) {
            let start = graph.edges.get(edge).map_or(0, |edge| {
                let [low, _] = edge.ends;
                low
            });
            out.push(walk(graph, &at_node, &mut used, (start, edge)));
        }
    }
    out
}

fn walk(graph: &Graph, at_node: &[Vec<usize>], used: &mut [bool], (start, first): (usize, usize)) -> Vec<Point> {
    let mut points = vec![graph.at(start)];
    let (mut node, mut edge) = (start, first);
    while let Some(flag) = used.get_mut(edge).filter(|flag| !**flag) {
        *flag = true;
        let [low, high] = graph.edges.get(edge).map_or([node, node], |edge| edge.ends);
        node = if low == node { high } else { low };
        points.push(graph.at(node));
        let list = at_node.get(node).map_or(&[][..], Vec::as_slice);
        if list.len() != 2 {
            break;
        }
        edge = list.iter().copied().find(|other| *other != edge).unwrap_or(edge);
    }
    points
}

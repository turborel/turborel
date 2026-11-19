//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.2.3 Description 6, 7] The
//! next left and next right edges, from the cyclic order of the edges at a
//! node. Walking an edge to its end node with its left face on the left,
//! the next edge round that face is the first edge met turning clockwise
//! from the edge just walked; walking it back to its start node, round its
//! right face, the same. So each end of an edge at a node takes as its
//! pointer the end clockwise before it in the node's star, signed positive
//! if that edge leaves the node from its start.
//!
//! Directions are ordered exactly: by half-plane, then by the orientation
//! of the node and the two first points, never by an angle computed in
//! floating point.

use std::cmp::Ordering;

use super::predicates::same;
use super::{Point, Topology, damaged, line_of};
use crate::error::Result;
use crate::spatial::planar::orient2d as orient;

/// One end of an edge at a node.
#[derive(Debug, Clone, Copy)]
pub(super) struct End {
    /// The edge.
    pub edge: i32,
    /// Whether the edge starts at the node, so it is walked forward
    /// leaving it.
    pub leaves: bool,
    /// The first point of the edge away from the node.
    pub toward: Point,
}

impl End {
    /// Its edge, signed as it is walked leaving the node.
    pub fn signed(&self) -> Result<i32> {
        if self.leaves { Ok(self.edge) } else { self.edge.checked_neg().ok_or_else(|| damaged("an edge identifier has no negation")) }
    }
}

/// The first point of `line` that is not `from`, walking forward.
fn first_away<'a>(from: Point, mut line: impl Iterator<Item = &'a Point>) -> Option<Point> {
    line.find(|p| !same(**p, from)).copied()
}

/// Which half of the plane about `at` the direction to `p` is in: 0 from
/// the positive x direction up to but not including the negative one,
/// else 1.
fn half(at: Point, p: Point) -> u8 {
    let ([ax, ay], [px, py]) = (at, p);
    u8::from(py < ay || (py.total_cmp(&ay).is_eq() && px < ax))
}

/// How the directions from `at` to `p` and to `q` compare,
/// counterclockwise from the positive x direction.
pub(super) fn by_angle(at: Point, p: Point, q: Point) -> Ordering {
    half(at, p).cmp(&half(at, q)).then_with(|| orient(at, p, q).reverse())
}

impl Topology {
    /// The ends of edges at node `node`, at `at`, counterclockwise; of two
    /// ends leaving in one direction, the lesser edge first.
    pub(super) fn star(&self, node: i32) -> Result<Vec<End>> {
        let at = self.at(node)?;
        let mut ends = Vec::new();
        for (id, e) in &self.edges {
            if e.start != node && e.end != node {
                continue;
            }
            let line = line_of(&e.geometry)?;
            let away = |toward: Option<Point>| toward.ok_or_else(|| damaged(&format!("edge {id} has no length")));
            if e.start == node {
                ends.push(End { edge: *id, leaves: true, toward: away(first_away(at, line.iter()))? });
            }
            if e.end == node {
                ends.push(End { edge: *id, leaves: false, toward: away(first_away(at, line.iter().rev()))? });
            }
        }
        ends.sort_by(|a, b| by_angle(at, a.toward, b.toward).then(a.edge.cmp(&b.edge)).then(a.leaves.cmp(&b.leaves)));
        Ok(ends)
    }

    /// [§11.3.8 Description 2f iv 9] The next left and next right edges of
    /// every edge ending at the nodes `nodes`, set again from their stars.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if an edge there has no length or ends at no node.
    pub(super) fn relink(&mut self, nodes: &[i32]) -> Result<()> {
        for node in nodes {
            let ends = self.star(*node)?;
            let Some(last) = ends.last().copied() else { continue };
            let mut before = last;
            for end in &ends {
                let pointer = before.signed()?;
                let edge = self.edges.get_mut(&end.edge).ok_or_else(|| damaged("an edge of a star is not there"))?;
                if end.leaves {
                    edge.next_right = pointer;
                } else {
                    edge.next_left = pointer;
                }
                before = *end;
            }
        }
        Ok(())
    }

    /// The face counterclockwise of the direction from node `node` toward
    /// `toward`: the face that a new edge leaving that way runs into.
    /// None if no edge ends at the node.
    pub(super) fn face_toward(&self, node: i32, toward: Point) -> Result<Option<i32>> {
        let at = self.at(node)?;
        let ends = self.star(node)?;
        // The end clockwise before the new direction: the last that sorts
        // before it, or the last of all.
        let before = ends.iter().rev().find(|e| by_angle(at, e.toward, toward) == Ordering::Less).or(ends.last());
        let Some(end) = before else { return Ok(None) };
        let edge = self.edges.get(&end.edge).ok_or_else(|| damaged("an edge of a star is not there"))?;
        // Counterclockwise of an edge leaving forward is its left; of one
        // leaving backward, its right.
        Ok(Some(if end.leaves { edge.left_face } else { edge.right_face }))
    }
}

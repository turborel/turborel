//! \[DESIGN\] A geometry as graph paths, made of its points, its line
//! strings and its rings, each ring raising the area depth on the side of its
//! polygon's interior. An exterior ring raises the depth inside itself
//! and an interior ring lowers it, whichever way either turns, so a
//! polygon's interior is exactly where the depth is positive.

use std::cmp::Ordering;

use super::graph::{Path, Role};
use super::ring::orientation;
use super::segment::same;
use super::shape::Parts;

/// The paths of one operand's parts.
pub(crate) fn paths(parts: &Parts, input: usize) -> Vec<Path> {
    let mut paths: Vec<Path> = parts.points.iter().map(|point| Path { input, role: Role::Point, vertices: vec![*point] }).collect();
    paths.extend(parts.lines.iter().map(|line| Path { input, role: Role::Line, vertices: line.clone() }));
    for polygon in &parts.polygons {
        for (index, ring) in polygon.iter().enumerate() {
            paths.push(Path { input, role: Role::Ring(raise(ring, index == 0)), vertices: closed(ring) });
        }
    }
    paths
}

/// The depth change on a ring's left, +1 where its polygon's interior
/// lies on its left, -1 where it lies on its right.
pub(crate) fn raise(ring: &[[f64; 2]], exterior: bool) -> i32 {
    let counterclockwise = orientation(ring) != Ordering::Less;
    if counterclockwise == exterior { 1 } else { -1 }
}

/// The ring with its first point repeated at the end if it was not.
pub(crate) fn closed(ring: &[[f64; 2]]) -> Vec<[f64; 2]> {
    let mut vertices = ring.to_vec();
    if let (Some(first), Some(last)) = (ring.first(), ring.last())
        && !same(*first, *last)
    {
        vertices.push(*first);
    }
    vertices
}

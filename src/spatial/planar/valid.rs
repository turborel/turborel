//! ST_IsValid [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.10], whether
//! a value is well formed, by the conditions each type states. A line
//! string needs two points [§7.2.1 Description 4]. A polygon's rings are
//! linear rings, closed and simple [§8.3.1 Description 2, 3]; two of them
//! meet in at most one point [§8.2.1 Description 11]; there are no cut
//! lines, spikes or punctures [Description 12]; the interior is connected
//! [Description 13]; and each interior ring bounds a separate component
//! of the exterior inside the exterior ring [Description 14]. A
//! multipolygon's elements are well formed, their interiors disjoint and
//! their boundaries meet in finitely many points [§10.6.1 Description 3,
//! 4, 9]. Every other collection is well formed when its elements are
//! [§10.1.1 Description 10, §10.2.1 Description 7, §10.4.1 Description
//! 3]. The standard has no routine giving a reason; the engine reports
//! one for diagnostics.

use std::collections::HashMap;

use super::Point;
use super::chain::{envelope, overlap};
use super::label::Location;
use super::meetings::hits;
use super::noding::deduplicated;
use super::relate::relate;
use super::ring::contains;
use super::segment::{Meeting, same};
use super::shape::{Dimension, Geometry};
use super::simple::{position, self_meeting};
use crate::error::Result;

/// A linear ring repeats its first point after at least three others.
const RING_POINTS: usize = 4;
/// A line string has at least two points.
const LINE_POINTS: usize = 2;

/// Why a value is not well formed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// A line string with fewer than two points, or a ring with fewer
    /// than four.
    TooFewPoints,
    /// A ring whose last point is not its first.
    RingNotClosed,
    /// A ring that meets itself.
    RingSelfIntersection,
    /// Two rings that cross or share a segment.
    RingsCross,
    /// Two rings that meet in more than one point.
    RingsTouchTwice,
    /// An interior ring outside the exterior ring.
    HoleOutsideShell,
    /// An interior ring inside another.
    NestedHoles,
    /// Rings that cut the interior into pieces.
    DisconnectedInterior,
    /// Two polygons of a multipolygon whose interiors meet.
    OverlappingPolygons,
    /// Two polygons of a multipolygon sharing a stretch of boundary.
    SharedBoundary,
}

impl Reason {
    /// The reason in words.
    pub fn text(self) -> &'static str {
        match self {
            Self::TooFewPoints => "too few points",
            Self::RingNotClosed => "ring not closed",
            Self::RingSelfIntersection => "ring self-intersection",
            Self::RingsCross => "rings cross",
            Self::RingsTouchTwice => "rings meet in more than one point",
            Self::HoleOutsideShell => "interior ring outside the exterior ring",
            Self::NestedHoles => "interior ring inside another",
            Self::DisconnectedInterior => "interior is not connected",
            Self::OverlappingPolygons => "polygon interiors intersect",
            Self::SharedBoundary => "polygon boundaries share a curve",
        }
    }
}

/// Why and where a value is not well formed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Invalid {
    /// What is wrong.
    pub reason: Reason,
    /// A position where it is wrong, if one is.
    pub at: Option<Point>,
}

fn invalid(reason: Reason, at: Option<Point>) -> Invalid {
    Invalid { reason, at }
}

/// ST_IsValid; an empty value is well formed [§5.1.10 Description 2a].
///
/// # Errors
///
/// SQLCODE -910 if noding a multipolygon's elements does not settle.
pub fn is_valid(geometry: &Geometry) -> Result<bool> {
    Ok(validate(geometry)?.is_none())
}

/// Why the value is not well formed, or nothing if it is.
///
/// # Errors
///
/// SQLCODE -910 if noding a multipolygon's elements does not settle.
pub fn validate(geometry: &Geometry) -> Result<Option<Invalid>> {
    Ok(match geometry {
        Geometry::Empty | Geometry::Point(_) | Geometry::MultiPoint(_) => None,
        Geometry::Line(line) => line_string(line),
        Geometry::MultiLine(lines) => lines.iter().find_map(|line| line_string(line)),
        Geometry::Polygon(rings) => polygon(rings),
        Geometry::MultiPolygon(polygons) => multipolygon(polygons)?,
        Geometry::Collection(items) => {
            for item in items {
                if let Some(found) = validate(item)? {
                    return Ok(Some(found));
                }
            }
            None
        }
    })
}

fn line_string(line: &[Point]) -> Option<Invalid> {
    if !line.is_empty() && line.len() < LINE_POINTS { Some(invalid(Reason::TooFewPoints, line.first().copied())) } else { None }
}

/// A linear ring has at least four points and is closed and simple.
fn linear_ring(ring: &[Point]) -> Option<Invalid> {
    let (Some(first), Some(last)) = (ring.first(), ring.last()) else { return Some(invalid(Reason::TooFewPoints, None)) };
    if ring.len() < RING_POINTS {
        return Some(invalid(Reason::TooFewPoints, Some(*first)));
    }
    if !same(*first, *last) {
        return Some(invalid(Reason::RingNotClosed, Some(*last)));
    }
    self_meeting(ring).map(|at| invalid(Reason::RingSelfIntersection, Some(at)))
}

/// Whether a ring is a linear ring.
pub(crate) fn is_linear_ring(ring: &[Point]) -> bool {
    linear_ring(ring).is_none()
}

fn polygon(rings: &[Vec<Point>]) -> Option<Invalid> {
    if rings.first().is_none_or(Vec::is_empty) {
        return None;
    }
    if let Some(found) = rings.iter().find_map(|ring| linear_ring(ring)) {
        return Some(found);
    }
    let lines: Vec<Vec<Point>> = rings.iter().map(|ring| deduplicated(ring)).collect();
    let mut touches: HashMap<(usize, usize), Vec<Point>> = HashMap::new();
    let between: Vec<_> = hits(&lines).into_iter().filter(|hit| hit.first.line != hit.second.line).collect();
    if let Some(hit) = between.iter().find(|hit| !matches!(hit.meeting, Meeting::Touch(_))) {
        return Some(invalid(Reason::RingsCross, Some(position(&lines, hit))));
    }
    for hit in between {
        let Meeting::Touch(point) = hit.meeting else { continue };
        let points = touches.entry((hit.first.line, hit.second.line)).or_default();
        if !points.iter().any(|other| same(*other, point)) {
            points.push(point);
        }
        if points.len() > 1 {
            return Some(invalid(Reason::RingsTouchTwice, Some(point)));
        }
    }
    nesting(&lines).or_else(|| connected(&touches))
}

/// Every interior ring lies inside the exterior ring and outside the
/// other interior rings; rings meeting in one point decide by the rest.
fn nesting(rings: &[Vec<Point>]) -> Option<Invalid> {
    let (shell, holes) = rings.split_first()?;
    for (index, hole) in holes.iter().enumerate() {
        if !contains(shell, hole) {
            return Some(invalid(Reason::HoleOutsideShell, hole.first().copied()));
        }
        for other in holes.iter().skip(index.saturating_add(1)) {
            if contains(hole, other) || contains(other, hole) {
                return Some(invalid(Reason::NestedHoles, other.first().copied()));
            }
        }
    }
    None
}

/// Rings meeting at single points disconnect the interior exactly when
/// the graph of rings and meeting points has a cycle.
fn connected(touches: &HashMap<(usize, usize), Vec<Point>>) -> Option<Invalid> {
    let mut at_point: HashMap<[u64; 2], Vec<usize>> = HashMap::new();
    let mut entries: Vec<_> = touches.iter().collect();
    entries.sort_by_key(|(pair, _)| **pair);
    for ((first, second), points) in entries {
        for point in points {
            let rings = at_point.entry(point.map(|value| (value + 0.0_f64).to_bits())).or_default();
            for ring in [*first, *second] {
                if !rings.contains(&ring) {
                    rings.push(ring);
                }
            }
        }
    }
    let count = touches.keys().map(|(first, second)| first.max(second).saturating_add(1)).max().unwrap_or_default();
    let mut parent: Vec<usize> = (0..count.saturating_add(at_point.len())).collect();
    let mut keys: Vec<_> = at_point.into_iter().collect();
    keys.sort_by_key(|(key, _)| *key);
    for (offset, (key, rings)) in keys.into_iter().enumerate() {
        let node = count.saturating_add(offset);
        for ring in rings {
            let (a, b) = (find(&mut parent, node), find(&mut parent, ring));
            if a == b {
                return Some(invalid(Reason::DisconnectedInterior, Some(key.map(f64::from_bits))));
            }
            if let Some(entry) = parent.get_mut(a) {
                *entry = b;
            }
        }
    }
    None
}

fn find(parent: &mut [usize], mut item: usize) -> usize {
    while let Some(next) = parent.get(item).copied().filter(|next| *next != item) {
        item = next;
    }
    item
}

fn multipolygon(polygons: &[Vec<Vec<Point>>]) -> Result<Option<Invalid>> {
    if let Some(found) = polygons.iter().find_map(|rings| polygon(rings)) {
        return Ok(Some(found));
    }
    let items: Vec<(Geometry, _)> = polygons
        .iter()
        .filter_map(|rings| rings.first().filter(|ring| !ring.is_empty()).map(|ring| (Geometry::Polygon(rings.clone()), envelope(ring))))
        .collect();
    for (index, (first, bounds)) in items.iter().enumerate() {
        for (second, _) in items.iter().skip(index.saturating_add(1)).filter(|(_, other)| overlap(*bounds, *other)) {
            let Some(matrix) = relate(first, second)? else { continue };
            if matrix.get(Location::Interior, Location::Interior) != Dimension::Empty {
                return Ok(Some(invalid(Reason::OverlappingPolygons, None)));
            }
            if matrix.get(Location::Boundary, Location::Boundary) > Dimension::Point {
                return Ok(Some(invalid(Reason::SharedBoundary, None)));
            }
        }
    }
    Ok(None)
}

//! ST_IsSimple [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.8] by the
//! conditions each type states. A point and every surface are simple
//! [§6.1.1 Description 10, §8.2.1 Description 8, §10.5.1 Description 4].
//! A multipoint is simple when no two points are equal [§10.2.1
//! Description 5]. A curve is simple when no interior point is repeated
//! and none lies on its boundary, so only a closed curve may meet itself,
//! at its start [§7.1.1 Description 6]. A multicurve is simple when its
//! elements are and they meet only at points on both their boundaries
//! [§10.3.1 Description 4]. A collection is simple when its elements are
//! and their interiors are pairwise disjoint [§10.1.1 Description 9].
//! Repeated consecutive positions add no point to a curve and are not
//! an anomaly.

use std::cmp::Ordering;

use super::Point;
use super::chain::Piece;
use super::label::Location;
use super::meetings::{Hit, hits};
use super::noding::deduplicated;
use super::relate::relate;
use super::segment::{Meeting, crossing, lexicographic, same};
use super::shape::{Dimension, Geometry};
use crate::error::Result;

/// ST_IsSimple; an empty geometry is simple [§5.1.8 Description 2a].
///
/// # Errors
///
/// SQLCODE -910 if noding a collection's elements does not settle.
pub fn is_simple(geometry: &Geometry) -> Result<bool> {
    Ok(match geometry {
        Geometry::Empty | Geometry::Point(_) | Geometry::Polygon(_) | Geometry::MultiPolygon(_) => true,
        Geometry::MultiPoint(points) => distinct(points),
        Geometry::Line(line) => self_meeting(line).is_none(),
        Geometry::MultiLine(lines) => multicurve_meeting(lines).is_none(),
        Geometry::Collection(items) => collection(items)?,
    })
}

fn distinct(points: &[Point]) -> bool {
    let mut sorted = points.to_vec();
    sorted.sort_by(|left, right| lexicographic(*left, *right));
    sorted.windows(2).all(|pair| if let [left, right] = pair { !same(*left, *right) } else { true })
}

/// Where a curve first meets itself other than its segments sharing a
/// vertex or a closed curve's ends meeting, or nothing if it is simple.
pub(crate) fn self_meeting(line: &[Point]) -> Option<Point> {
    let vertices = deduplicated(line);
    let segments = vertices.len().saturating_sub(1);
    let closed = segments > 1 && vertices.first().zip(vertices.last()).is_some_and(|(first, last)| same(*first, *last));
    let lines = [vertices];
    for hit in hits(&lines) {
        let (low, high) = (hit.first.index, hit.second.index);
        let shared = if high == low.saturating_add(1) {
            lines.first().and_then(|walk| walk.get(high)).copied()
        } else if closed && low == 0 && high == segments.saturating_sub(1) {
            lines.first().and_then(|walk| walk.first()).copied()
        } else {
            None
        };
        let allowed = if let (Meeting::Touch(point), Some(vertex)) = (hit.meeting, shared) { same(point, vertex) } else { false };
        if !allowed {
            return Some(position(&lines, &hit));
        }
    }
    None
}

/// A position of a meeting, the shared point, the start of a shared
/// stretch, or the rounded crossing.
pub(crate) fn position(lines: &[Vec<Point>], hit: &Hit) -> Point {
    let end = |piece: Piece, step: usize| {
        lines.get(piece.line).and_then(|vertices| vertices.get(piece.index.saturating_add(step))).copied().unwrap_or_default()
    };
    match hit.meeting {
        Meeting::Touch(point) | Meeting::Overlap(point, _) => point,
        Meeting::Apart | Meeting::Cross => crossing(end(hit.first, 0), end(hit.first, 1), end(hit.second, 0), end(hit.second, 1)),
    }
}

/// Where a multicurve first fails to be simple, or nothing.
fn multicurve_meeting(lines: &[Vec<Point>]) -> Option<Point> {
    for line in lines {
        if let Some(point) = self_meeting(line) {
            return Some(point);
        }
    }
    let lines: Vec<Vec<Point>> = lines.iter().map(|line| deduplicated(line)).collect();
    let boundary = |line: usize, point: Point| {
        let ends = lines.get(line).and_then(|vertices| vertices.first().zip(vertices.last()));
        ends.is_some_and(|(first, last)| !same(*first, *last) && (same(point, *first) || same(point, *last)))
    };
    for hit in hits(&lines).into_iter().filter(|hit| hit.first.line != hit.second.line) {
        let allowed = if let Meeting::Touch(point) = hit.meeting {
            boundary(hit.first.line, point) && boundary(hit.second.line, point)
        } else {
            false
        };
        if !allowed {
            return Some(position(&lines, &hit));
        }
    }
    None
}

fn collection(items: &[Geometry]) -> Result<bool> {
    for item in items {
        if !is_simple(item)? {
            return Ok(false);
        }
    }
    for (index, first) in items.iter().enumerate() {
        for second in items.iter().skip(index.saturating_add(1)) {
            let Some(matrix) = relate(first, second)? else { continue };
            if matrix.get(Location::Interior, Location::Interior).cmp(&Dimension::Empty) != Ordering::Equal {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

//! A prepared polygon's edges, filed once in a grid over its envelope
//! (`crossing/buckets.rs`), so each line or polygon tested against it probes
//! only the filed edges near its own. The decision is the closed point
//! sets' intersection: some pair of edges meets, or a first vertex of one
//! shape is not outside the other, part by part.

use std::sync::Arc;

use crate::geometry::crossing::buckets::Filed;
use crate::geometry::crossing::{self, Location};
use crate::geometry::shape::{COLLECTION, MULTIPOLYGON, Point, Shape};
use crate::memory::{MemoryBudget, Reservation};
use crate::spatial::Bounds;

/// The filed edges of a prepared polygon, and their reservation.
pub(super) struct FiledEdges {
    filed: Filed,
    _held: Reservation,
}

impl FiledEdges {
    /// Files every edge of `shape`, a polygon or multipolygon with
    /// envelope `envelope`; none for another kind, a grid that cannot be
    /// exact, or a refused reservation.
    pub(super) fn new(shape: &Shape, envelope: Bounds, budget: &Arc<MemoryBudget>) -> Option<Box<Self>> {
        let rings = rings(shape)?;
        let edges = rings.iter().map(|ring| ring.len().saturating_sub(1)).sum();
        let filed = Filed::new(&rings, envelope, edges)?;
        let held = budget.reserve(filed.bytes()?, "a prepared polygon's filed edges").ok()?;
        Some(Box::new(Self { filed, _held: held }))
    }

    /// Whether `other`, a line or polygon or a multi-part one of them,
    /// meets the prepared polygon `shape`, placing points in it by
    /// `locate`; none for another kind or where the grid declines.
    pub(super) fn meets(&self, shape: &Shape, other: &Shape, locate: &dyn Fn(Point) -> Location) -> Option<bool> {
        match other {
            Shape::Line(line) => {
                let crossed = self.filed.crosses(&[line.as_slice()])?;
                Some(crossed || line.first().is_some_and(|&point| locate(point) != Location::Exterior))
            }
            Shape::Polygon(polygon) => self.polygon(shape, polygon, locate),
            Shape::Multi(code, parts) => {
                if *code == COLLECTION {
                    return None;
                }
                for part in parts {
                    if self.meets(shape, part, locate)? {
                        return Some(true);
                    }
                }
                Some(false)
            }
            Shape::Point(_) => None,
        }
    }

    /// A polygon meets the prepared one if their edges meet, its first
    /// vertex is not outside the prepared polygon, or a first vertex of
    /// one of the prepared parts is not outside it.
    fn polygon(&self, shape: &Shape, polygon: &[Vec<Point>], locate: &dyn Fn(Point) -> Location) -> Option<bool> {
        let chains: Vec<&[Point]> = polygon.iter().map(Vec::as_slice).collect();
        if self.filed.crosses(&chains)? {
            return Some(true);
        }
        let inside_prepared = polygon.first().and_then(|ring| ring.first()).is_some_and(|&point| locate(point) != Location::Exterior);
        let inside_other = firsts(shape)?.into_iter().any(|point| crossing::in_polygon(point, polygon) != Location::Exterior);
        Some(inside_prepared || inside_other)
    }
}

/// Every ring of a polygon or multipolygon.
pub(super) fn rings(shape: &Shape) -> Option<Vec<&[Point]>> {
    match shape {
        Shape::Polygon(polygon) => Some(polygon.iter().map(Vec::as_slice).collect()),
        Shape::Multi(code, parts) => {
            if *code != MULTIPOLYGON {
                return None;
            }
            let mut rings = Vec::new();
            for part in parts {
                let Shape::Polygon(polygon) = part else { return None };
                rings.extend(polygon.iter().map(Vec::as_slice));
            }
            Some(rings)
        }
        Shape::Point(_) | Shape::Line(_) => None,
    }
}

/// The first vertex of each part of a polygon or multipolygon, as
/// the closed point sets' intersection tests them for containment.
fn firsts(shape: &Shape) -> Option<Vec<Point>> {
    let first = |polygon: &[Vec<Point>]| polygon.first().and_then(|ring| ring.first()).copied();
    match shape {
        Shape::Polygon(polygon) => Some(first(polygon).into_iter().collect()),
        Shape::Multi(code, parts) => {
            if *code != MULTIPOLYGON {
                return None;
            }
            let mut firsts = Vec::new();
            for part in parts {
                let Shape::Polygon(polygon) = part else { return None };
                firsts.extend(first(polygon));
            }
            Some(firsts)
        }
        Shape::Point(_) | Shape::Line(_) => None,
    }
}

#[cfg(test)]
mod tests;

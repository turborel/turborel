//! Immutable Y-interval trees for repeated point location in polygons.
//! Each ring keeps its own parity and boundary result. Sorting changes
//! only which edges are visited, never the exact crossing calculation.

use std::sync::Arc;

use crate::error::{Code, Result, SqlError};
use crate::geometry::MAX_PARTS;
use crate::geometry::crossing::{Location, Ray, segments};
use crate::geometry::shape::{MULTIPOLYGON, Point, Shape};
use crate::memory::{MemoryBudget, Reservation};

/// A median tree halves each slice. The representation's coordinate
/// limit therefore bounds its nonempty levels by this bit length.
const LEVELS: u32 = usize::BITS.saturating_sub(MAX_PARTS.leading_zeros());
const CHILDREN: usize = 2;
/// The 4–128-edge preparation benchmark first showed an intersection
/// gain beyond noise at 64 edges. Smaller boundaries retain linear scans.
const MIN_EDGES: usize = 64;

struct Edge {
    start: Point,
    end: Point,
    maximum: f64,
}

impl Edge {
    fn lower(&self) -> f64 {
        let [_, start] = self.start;
        let [_, end] = self.end;
        start.min(end)
    }
}

type Ring = Vec<Edge>;

struct Polygon {
    rings: Vec<Ring>,
}

pub(super) struct Locator {
    polygons: Vec<Polygon>,
    _held: Reservation,
}

impl Locator {
    pub(super) fn new(shape: &Shape, budget: &Arc<MemoryBudget>) -> Option<Box<Self>> {
        let parts = polygons(shape)?;
        let longest = parts.iter().try_fold(0_usize, |longest, part| {
            let edges = polygon(part)?.iter().map(|ring| ring.len().saturating_sub(1)).max().unwrap_or(0);
            Some(longest.max(edges))
        })?;
        if longest < MIN_EDGES {
            return None;
        }
        let mut held = budget.reserve(size_of::<Self>(), "a prepared polygon interval index").ok()?;
        let mut polygons = allocated(parts.len(), &mut held)?;
        for part in parts {
            let source = polygon(part)?;
            let mut rings = allocated(source.len(), &mut held)?;
            for ring in source {
                let mut edges = allocated(ring.len().saturating_sub(1), &mut held)?;
                for (start, end) in segments(ring) {
                    let ([_, ay], [_, by]) = (start, end);
                    edges.push(Edge { start, end, maximum: ay.max(by) });
                }
                edges.sort_unstable_by(|a, b| a.lower().total_cmp(&b.lower()));
                maxima(&mut edges, 0).ok()?;
                rings.push(edges);
            }
            polygons.push(Polygon { rings });
        }
        Some(Box::new(Self { polygons, _held: held }))
    }

    /// Where `point` lies, counting in `tally` the edges its rays test.
    pub(super) fn locate<T: Tally>(&self, point: Point, tally: &mut T) -> Result<Location> {
        let mut location = Location::Exterior;
        for polygon in &self.polygons {
            match polygon.locate(point, tally)? {
                Location::Interior => return Ok(Location::Interior),
                Location::Boundary => location = Location::Boundary,
                Location::Exterior => (),
            }
        }
        Ok(location)
    }
}

impl Polygon {
    fn locate<T: Tally>(&self, point: Point, tally: &mut T) -> Result<Location> {
        let Some((shell, holes)) = self.rings.split_first() else { return Ok(Location::Exterior) };
        let shell = in_ring(point, shell, tally)?;
        if shell != Location::Interior {
            return Ok(shell);
        }
        for hole in holes {
            match in_ring(point, hole, tally)? {
                Location::Interior => return Ok(Location::Exterior),
                Location::Boundary => return Ok(Location::Boundary),
                Location::Exterior => (),
            }
        }
        Ok(Location::Interior)
    }
}

fn polygons(shape: &Shape) -> Option<&[Shape]> {
    match shape {
        Shape::Polygon(_) => Some(std::slice::from_ref(shape)),
        Shape::Multi(code, parts) => (*code == MULTIPOLYGON).then_some(parts.as_slice()),
        Shape::Point(_) | Shape::Line(_) => None,
    }
}

fn polygon(shape: &Shape) -> Option<&[Vec<Point>]> {
    match shape {
        Shape::Polygon(rings) => Some(rings),
        Shape::Point(_) | Shape::Line(_) | Shape::Multi(_, _) => None,
    }
}

/// Charge requested storage before allocation, and any allocator-provided
/// capacity before retaining it. A refusal drops the entire optional index.
fn allocated<T>(count: usize, held: &mut Reservation) -> Option<Vec<T>> {
    let base = held.bytes();
    held.resize(base.checked_add(count.checked_mul(size_of::<T>())?)?).ok()?;
    let mut values = Vec::new();
    values.try_reserve_exact(count).ok()?;
    held.resize(base.checked_add(values.capacity().checked_mul(size_of::<T>())?)?).ok()?;
    Some(values)
}

fn maxima(edges: &mut [Edge], depth: u32) -> Result<f64> {
    if edges.is_empty() {
        return Ok(f64::NEG_INFINITY);
    }
    if depth >= LEVELS {
        return Err(malformed());
    }
    let middle = edges.len().checked_div(CHILDREN).ok_or_else(malformed)?;
    let (left, rest) = edges.split_at_mut_checked(middle).ok_or_else(malformed)?;
    let (edge, right) = rest.split_first_mut().ok_or_else(malformed)?;
    let next = depth.saturating_add(1);
    edge.maximum = edge.maximum.max(maxima(left, next)?).max(maxima(right, next)?);
    Ok(edge.maximum)
}

fn in_ring<T: Tally>(point: Point, edges: &[Edge], tally: &mut T) -> Result<Location> {
    let mut ray = Ray::new(point);
    let boundary = visit(edges, &mut ray, 0, tally)?;
    if boundary {
        return Ok(Location::Boundary);
    }
    Ok(ray.location())
}

/// Median nodes store the maximum Y of their whole subtree; the first
/// element supplies its minimum. Closed bounds retain horizontal edges
/// and vertices. No midpoint arithmetic or rounded tolerance is needed.
/// Each edge the ray tests is counted in `tally`.
fn visit<T: Tally>(edges: &[Edge], ray: &mut Ray, depth: u32, tally: &mut T) -> Result<bool> {
    let Some(first) = edges.first() else { return Ok(false) };
    if depth >= LEVELS {
        return Err(malformed());
    }
    let middle = edges.len().checked_div(CHILDREN).ok_or_else(malformed)?;
    let (left, rest) = edges.split_at_checked(middle).ok_or_else(malformed)?;
    let (edge, right) = rest.split_first().ok_or_else(malformed)?;
    let [_, y] = ray.point();
    if y < first.lower() || y > edge.maximum {
        return Ok(false);
    }
    tally.tested();
    let boundary = ray.add(edge.start, edge.end);
    if boundary {
        return Ok(true);
    }
    let next = depth.saturating_add(1);
    let left_boundary = visit(left, ray, next, tally)?;
    if left_boundary {
        return Ok(true);
    }
    visit(right, ray, next, tally)
}

/// A count of the edges a ray tests: a number, or nothing kept, so a
/// location that is not counted pays nothing for it.
pub(super) trait Tally {
    fn tested(&mut self);
}

impl Tally for usize {
    fn tested(&mut self) {
        *self = self.saturating_add(1);
    }
}

impl Tally for () {
    fn tested(&mut self) {}
}

fn malformed() -> SqlError {
    SqlError::new(Code::Api, "[DESIGN] a prepared polygon interval tree exceeds its representation bound")
}

#[cfg(test)]
mod tests;

//! \[DESIGN\] The representation shared by WKT, WKB and SQL geometries.
//! Only the readers construct it: depth and size are bounded there.

use super::{invalid, unsupported};
use crate::error::Result;
use crate::spatial::{Bounds, DIMENSIONS};

pub(super) type Point = [f64; DIMENSIONS];

/// OGC 06-103r4 §8.2.3, Table 7: the seven basic XY geometry codes.
pub(super) const POINT: u32 = 1;
pub(super) const LINE: u32 = 2;
pub(super) const POLYGON: u32 = 3;
pub(super) const MULTIPOINT: u32 = 4;
pub(super) const MULTILINE: u32 = 5;
pub(super) const MULTIPOLYGON: u32 = 6;
pub(super) const COLLECTION: u32 = 7;
pub(super) const NAMES: [&str; 7] =
    ["POINT", "LINESTRING", "POLYGON", "MULTIPOINT", "MULTILINESTRING", "MULTIPOLYGON", "GEOMETRYCOLLECTION"];
pub(super) const EMPTY: &str = "EMPTY";
/// A linear ring repeats its first point after at least three vertices.
const MIN_RING: usize = 4;

#[derive(Clone, Debug)]
pub(super) enum Shape {
    Point(Option<Point>),
    Line(Vec<Point>),
    Polygon(Vec<Vec<Point>>),
    Multi(u32, Vec<Shape>),
}

impl Shape {
    /// Reader limits bound the recursion and every retained vector. Count
    /// capacity, since unused allocation is still owned by a prepared value.
    pub(super) fn heap_bytes(&self) -> Option<usize> {
        match self {
            Self::Point(_) => Some(0),
            Self::Line(points) => points.capacity().checked_mul(size_of::<Point>()),
            Self::Polygon(rings) => {
                let outer = rings.capacity().checked_mul(size_of::<Vec<Point>>())?;
                rings.iter().try_fold(outer, |bytes, ring| bytes.checked_add(ring.capacity().checked_mul(size_of::<Point>())?))
            }
            Self::Multi(_, parts) => {
                let outer = parts.capacity().checked_mul(size_of::<Shape>())?;
                parts.iter().try_fold(outer, |bytes, part| bytes.checked_add(part.heap_bytes()?))
            }
        }
    }

    pub(super) fn code(&self) -> u32 {
        match self {
            Self::Point(_) => POINT,
            Self::Line(_) => LINE,
            Self::Polygon(_) => POLYGON,
            Self::Multi(code, _) => *code,
        }
    }

    pub(super) fn name(&self) -> &'static str {
        usize::try_from(self.code().saturating_sub(1)).ok().and_then(|i| NAMES.get(i)).copied().unwrap_or_default()
    }

    pub(super) fn bounds(&self) -> Result<Option<Bounds>> {
        match self {
            Self::Point(p) => points(p.iter()),
            Self::Line(p) => points(p.iter()),
            Self::Polygon(rings) => points(rings.iter().flatten()),
            Self::Multi(_, parts) => {
                let mut bounds = None;
                for part in parts {
                    if let Some(b) = part.bounds()? {
                        extend(&mut bounds, b);
                    }
                }
                Ok(bounds)
            }
        }
    }
}

fn points<'a>(points: impl Iterator<Item = &'a Point>) -> Result<Option<Bounds>> {
    let mut bounds = None;
    for point in points {
        extend(&mut bounds, Bounds::point(*point)?);
    }
    Ok(bounds)
}

fn extend(bounds: &mut Option<Bounds>, next: Bounds) {
    *bounds = Some(bounds.map_or(next, |was| was.union(next)));
}

pub(super) fn coordinate(point: Point) -> Result<Point> {
    if !point.iter().all(|n| n.is_finite()) {
        return Err(invalid("coordinates must be finite"));
    }
    Ok(point.map(|n| n + 0.0_f64))
}

pub(super) fn line(points: &[Point]) -> Result<()> {
    if points.len() == 1 {
        return Err(invalid("a nonempty line needs at least two points"));
    }
    Ok(())
}

pub(super) fn ring(points: &[Point]) -> Result<()> {
    if points.len() < MIN_RING {
        return Err(invalid("a polygon ring needs at least four points"));
    }
    if points.first().map(|p| p.map(f64::to_bits)) != points.last().map(|p| p.map(f64::to_bits)) {
        return Err(invalid("a polygon ring must repeat its first point"));
    }
    Ok(())
}

pub(super) fn child(code: u32) -> Result<Option<u32>> {
    match code {
        MULTIPOINT => Ok(Some(POINT)),
        MULTILINE => Ok(Some(LINE)),
        MULTIPOLYGON => Ok(Some(POLYGON)),
        COLLECTION => Ok(None),
        _ => Err(unsupported("that geometry type or coordinate dimension")),
    }
}

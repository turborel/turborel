//! \[EXTENSION\] XY WKB, OGC 06-103r4 §8, with POINT EMPTY written
//! as a pair of NaN ordinates. Counts are checked against both the
//! remaining bytes and a shared component budget before allocating.

use super::shape::{self, LINE, POINT, POLYGON, Point, Shape};
use super::{MAX_DEPTH, MAX_PARTS, invalid, limited};
use crate::error::Result;
use crate::spatial::DIMENSIONS;

/// OGC 06-103r4 §8.2.7: XDR = 0, NDR = 1; the tag and u32 type.
const XDR: u8 = 0;
const NDR: u8 = 1;
const HEADER_BYTES: usize = 5;
const COUNT_BYTES: usize = 4;
const POINT_BYTES: usize = 16;
/// An OGC 06-103r4 §8.2.2 Double has eight bytes.
const ORDINATE_BYTES: usize = 8;
/// An empty point's ordinate, a canonical IEEE quiet NaN.
const EMPTY_ORDINATE: u64 = 0x7ff8_0000_0000_0000;
/// Canonical stored coordinates use positive zero (RULES.md D85).
const NEGATIVE_ZERO: u64 = 0x8000_0000_0000_0000;

/// The writer's byte order and floating representations are checked as
/// they are read, avoiding a second allocation and encoding for comparison.
pub(super) fn read_canonical(bytes: &[u8]) -> Result<Shape> {
    checked::<true>(bytes)
}

fn checked<const CANONICAL: bool>(bytes: &[u8]) -> Result<Shape> {
    let mut reader = Binary::<CANONICAL> { rest: bytes, parts: 0 };
    let shape = reader.geometry(1)?;
    if !reader.rest.is_empty() {
        return Err(invalid("bytes follow the WKB geometry"));
    }
    Ok(shape)
}

struct Binary<'a, const CANONICAL: bool> {
    rest: &'a [u8],
    parts: usize,
}

impl<const CANONICAL: bool> Binary<'_, CANONICAL> {
    fn charge(&mut self, count: usize) -> Result<()> {
        self.parts = self.parts.saturating_add(count);
        limited(self.parts, MAX_PARTS)
    }

    fn take<const N: usize>(&mut self) -> Result<[u8; N]> {
        let (taken, rest) = self.rest.split_at_checked(N).ok_or_else(|| invalid("WKB ends too soon"))?;
        self.rest = rest;
        taken.try_into().map_err(|_| invalid("WKB ends too soon"))
    }

    fn count(&mut self, little: bool, width: usize) -> Result<usize> {
        let count = usize::try_from(self.uint(little)?).map_err(|_| invalid("WKB count is too large"))?;
        limited(count, MAX_PARTS.saturating_sub(self.parts))?;
        if count.saturating_mul(width) > self.rest.len() {
            return Err(invalid("WKB count exceeds the bytes remaining"));
        }
        Ok(count)
    }

    fn uint(&mut self, little: bool) -> Result<u32> {
        let bytes = self.take()?;
        Ok(if little { u32::from_le_bytes(bytes) } else { u32::from_be_bytes(bytes) })
    }

    fn number(&mut self, little: bool) -> Result<f64> {
        let bytes = self.take()?;
        Ok(if little { f64::from_le_bytes(bytes) } else { f64::from_be_bytes(bytes) })
    }

    fn canonical_coordinates(&self, width: usize) -> Result<()> {
        if CANONICAL {
            let ordinates = self.rest.get(..width).ok_or_else(|| invalid("WKB ends too soon"))?;
            // Geometry headers already require NDR in this mode. A
            // contiguous pass keeps the coordinate parser's hot loop free
            // of a second per-ordinate representation branch (D85).
            let (chunks, _) = ordinates.as_chunks::<ORDINATE_BYTES>();
            let negative = chunks.contains(&NEGATIVE_ZERO.to_le_bytes());
            if negative {
                return Err(invalid("noncanonical WKB coordinate"));
            }
        }
        Ok(())
    }

    fn point(&mut self, little: bool) -> Result<Point> {
        Ok([self.number(little)?, self.number(little)?])
    }

    fn points(&mut self, little: bool) -> Result<Vec<Point>> {
        let count = self.count(little, POINT_BYTES)?;
        self.charge(count)?;
        self.canonical_coordinates(count.saturating_mul(POINT_BYTES))?;
        counted(count, || shape::coordinate(self.point(little)?))
    }

    fn point_shape(&mut self, little: bool) -> Result<Shape> {
        self.canonical_coordinates(POINT_BYTES)?;
        let p = self.point(little)?;
        let empty = p.iter().all(|v| v.is_nan());
        self.charge(usize::from(!empty))?;
        if empty {
            // Other geometry coordinates must be finite. Only an EMPTY
            // point needs the extra NaN representation check (D85).
            let other_nan = p.iter().any(|v| v.to_bits() != EMPTY_ORDINATE);
            if CANONICAL && other_nan {
                return Err(invalid("noncanonical WKB empty point"));
            }
            Ok(Shape::Point(None))
        } else {
            Ok(Shape::Point(Some(shape::coordinate(p)?)))
        }
    }

    fn geometry(&mut self, depth: usize) -> Result<Shape> {
        limited(depth, MAX_DEPTH)?;
        self.charge(1)?;
        let [order] = self.take()?;
        let little = match order {
            XDR => false,
            NDR => true,
            _ => return Err(invalid("WKB byte order is neither 0 nor 1")),
        };
        if CANONICAL && !little {
            return Err(invalid("stored WKB must be little-endian"));
        }
        let code = self.uint(little)?;
        match code {
            POINT => self.point_shape(little),
            LINE => {
                let points = self.points(little)?;
                shape::line(&points)?;
                Ok(Shape::Line(points))
            }
            POLYGON => {
                let count = self.count(little, COUNT_BYTES)?;
                self.charge(count)?;
                let rings = counted(count, || {
                    let points = self.points(little)?;
                    shape::ring(&points)?;
                    Ok(points)
                })?;
                Ok(Shape::Polygon(rings))
            }
            _ => self.multi(code, little, depth),
        }
    }

    fn multi(&mut self, code: u32, little: bool, depth: usize) -> Result<Shape> {
        let child = shape::child(code)?;
        let count = self.count(little, HEADER_BYTES)?;
        let parts = counted(count, || {
            let part = self.geometry(depth.saturating_add(1))?;
            if child.is_some_and(|wanted| part.code() != wanted) {
                return Err(invalid("WKB collection holds the wrong component type"));
            }
            Ok(part)
        })?;
        Ok(Shape::Multi(code, parts))
    }
}

/// Counts already passed the input-length and shared-budget checks. A
/// fallible iterator's collect loses its exact size, otherwise retaining
/// spare capacity and repeatedly growing large decoded geometries.
fn counted<T>(count: usize, mut read: impl FnMut() -> Result<T>) -> Result<Vec<T>> {
    let mut values = Vec::with_capacity(count);
    for _ in 0..count {
        values.push(read()?);
    }
    Ok(values)
}

pub(super) fn write(shape: &Shape) -> Vec<u8> {
    let mut out = Vec::new();
    append(shape, &mut out);
    out
}

fn append(shape: &Shape, out: &mut Vec<u8>) {
    out.push(NDR);
    out.extend_from_slice(&shape.code().to_le_bytes());
    match shape {
        Shape::Point(point) => put_point(point.unwrap_or([f64::from_bits(EMPTY_ORDINATE); DIMENSIONS]), out),
        Shape::Line(points) => put_points(points, out),
        Shape::Polygon(rings) => {
            put_count(rings.len(), out);
            for ring in rings {
                put_points(ring, out);
            }
        }
        Shape::Multi(_, parts) => {
            put_count(parts.len(), out);
            for part in parts {
                append(part, out);
            }
        }
    }
}

fn put_count(count: usize, out: &mut Vec<u8>) {
    // Readers cap counts at MAX_PARTS, which fits in the WKB u32 field.
    out.extend_from_slice(&u32::try_from(count).unwrap_or(u32::MAX).to_le_bytes());
}

fn put_point(point: Point, out: &mut Vec<u8>) {
    for ordinate in point {
        out.extend_from_slice(&ordinate.to_le_bytes());
    }
}

fn put_points(points: &[Point], out: &mut Vec<u8>) {
    put_count(points.len(), out);
    for point in points {
        put_point(*point, out);
    }
}

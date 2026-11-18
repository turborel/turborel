//! A dyadic grid over a region of the plane: a side that is a power of
//! two, the integral numbers of its first column and row, and exact
//! arithmetic on them. Cell bounds and row centres are exact binary64
//! numbers; only the columns an edge crosses are estimated, and those are
//! widened by a bound on their floating error, so the cells an edge is
//! said to touch include every cell it does touch. Prepared polygons label
//! its cells (`prepared/cells.rs`); intersection tests bucket edges in it
//! (`crossing/buckets.rs`).

use std::ops::RangeInclusive;

use super::shape::Point;
use crate::spatial::Bounds;

/// The fewest cells the grid aims for in all, and the most along a side.
const MIN_CELLS: u32 = 64;
const MAX_SIDE: u32 = 1_024;
/// Doublings of the side that can bring the longer axis within
/// `MAX_SIDE` cells, from a side the grid's area allows.
const MAX_DOUBLINGS: u32 = 64;
/// A cell side at least this many units in the last place of the largest
/// coordinate keeps the floating error of an edge's columns well below a
/// cell.
const LEAST_UNITS: f64 = 256.0;
/// Below this side, half a side or a row centre could lose exactness.
const SMALLEST_SIDE: f64 = 1e-280;
/// Scaled coordinates stay below 2^44, so a cell number plus `SHIFT`, a
/// cell's lower bound, is exact.
const CELL_NUMBER_LIMIT: f64 = 17_592_186_044_416.0;
/// Cell bounds sit 2^-8 of a cell past multiples of the side, so integer
/// and other round coordinates rarely fall on a bound, where a closed cell
/// on each side would be marked.
const SHIFT: f64 = 0.003_906_25;
/// IEEE 754 binary64 layout.
const FRACTION_BITS: u32 = 52;
const EXPONENT_MASK: u64 = 0x7ff;
const FRACTION_MASK: u64 = 0x000f_ffff_ffff_ffff;
const EXPONENT_BIAS: u64 = 1_023;
/// The error bound's factors: a column computed from a band's edge is off
/// by at most 2u|dx/dy| + 2^-51|dx| + u for unit u; doubled.
const SLOPE_UNITS: f64 = 4.0;
const SPAN_FACTOR: f64 = 8.881_784_197_001_252e-16;
const FLAT_UNITS: f64 = 2.0;

pub(in crate::geometry) struct Grid {
    side: f64,
    inverse: f64,
    origin: [f64; 2],
    /// A unit in the last place of the largest magnitude in the grid.
    unit: f64,
    pub(in crate::geometry) cols: usize,
    pub(in crate::geometry) rows: usize,
}

impl Grid {
    /// A grid over `bounds` of about `per_edge` cells for each of `edges`
    /// edges, if its coordinates allow exact cells: a normal side, well
    /// above the coordinates' units in the last place, and scaled
    /// coordinates below 2^44.
    pub(in crate::geometry) fn new(bounds: Bounds, edges: usize, per_edge: u32) -> Option<Self> {
        let [x0, y0] = bounds.lower();
        let [x1, y1] = bounds.upper();
        let side = side_for(x1 - x0, y1 - y0, edges, per_edge)?;
        let inverse = 1.0_f64 / side;
        let largest = [x0, y0, x1, y1].map(f64::abs).into_iter().fold(0.0_f64, f64::max) + side * SLOPE_UNITS;
        let unit = unit_of(largest)?;
        if side < LEAST_UNITS * unit || side < SMALLEST_SIDE || !inverse.is_normal() {
            return None;
        }
        let origin = [number(x0, inverse)?, number(y0, inverse)?];
        let cols = integral(number(x1, inverse)? - origin[0])?.checked_add(1)?;
        let rows = integral(number(y1, inverse)? - origin[1])?.checked_add(1)?;
        Some(Self { side, inverse, origin, unit, cols, rows })
    }

    /// The row and column of the cell holding `point`, if the point is in
    /// the grid and its cell number is exact.
    pub(in crate::geometry) fn place(&self, [x, y]: Point) -> Option<(usize, usize)> {
        let col = integral(number(x, self.inverse)? - self.origin[0])?;
        let row = integral(number(y, self.inverse)? - self.origin[1])?;
        (col < self.cols && row < self.rows).then_some((row, col))
    }

    /// The exact x of a column's left side; the column after the last
    /// gives the grid's right side.
    pub(in crate::geometry) fn column_start(&self, col: usize) -> Option<f64> {
        Some((self.origin[0] + small(col)? + SHIFT) * self.side)
    }

    /// The exact y of a row's lower side; the row after the last gives the
    /// grid's upper side.
    pub(in crate::geometry) fn row_start(&self, row: usize) -> Option<f64> {
        Some((self.origin[1] + small(row)? + SHIFT) * self.side)
    }

    /// The rows an edge's closed y-extent touches: one more below if it
    /// begins exactly on a row's lower bound, as the row below touches it.
    pub(in crate::geometry) fn rows_of(&self, ([_, ay], [_, by]): (Point, Point)) -> Option<RangeInclusive<usize>> {
        let (low, high) = self.touched(ay.min(by), ay.max(by), self.origin[1], self.rows)?;
        Some(low..=high)
    }

    /// Calls `visit` with each row an edge may touch and the first and last
    /// column it may touch there; a superset of the cells it does touch.
    /// An edge within one row's height, or a vertical one, is covered over
    /// its exact x-extent; any other over each row band's computed extent,
    /// widened by its error bound.
    pub(in crate::geometry) fn cover(&self, edge: (Point, Point), mut visit: impl FnMut(usize, usize, usize) -> Option<()>) -> Option<()> {
        for row in self.rows_of(edge)? {
            let (first, last) = self.columns(edge, row)?;
            visit(row, first, last)?;
        }
        Some(())
    }

    /// The first and last column an edge may touch in one of the rows it
    /// may touch, as `cover` gives them.
    pub(in crate::geometry) fn columns(&self, (a, b): (Point, Point), row: usize) -> Option<(usize, usize)> {
        let ([ax, ay], [bx, by]) = (a, b);
        let (dx, dy) = ((bx - ax).abs(), (by - ay).abs());
        let exact = dy < self.side || dx <= 0.0_f64;
        let error = if exact { 0.0_f64 } else { self.error(dx, dy) };
        let (low, high) = if exact { (ax.min(bx), ax.max(bx)) } else { self.span(a, b, row)? };
        self.touched(low - error, high + error, self.origin[0], self.cols)
    }

    /// The x-range, as computed, of an edge within a row's closed band.
    fn span(&self, [ax, ay]: Point, [bx, by]: Point, row: usize) -> Option<(f64, f64)> {
        let bottom = self.row_start(row)?;
        let at = |y: f64| ax + ((y - ay) / (by - ay)).clamp(0.0_f64, 1.0_f64) * (bx - ax);
        let (p, q) = (at(bottom), at(bottom + self.side));
        Some((p.min(q), p.max(q)))
    }

    /// A bound on the error of an x computed by `span`, doubled: 2u|dx/dy|
    /// from the band's offset and the division, 2^-51|dx| from the
    /// product, and u from the sum, for u a unit in the last place.
    fn error(&self, dx: f64, dy: f64) -> f64 {
        SLOPE_UNITS * self.unit * (dx / dy) + SPAN_FACTOR * dx + FLAT_UNITS * self.unit
    }

    /// The cells a closed interval of coordinates touches along one axis,
    /// clamped to `0..count`: from the cell holding its low end, or the one
    /// before if the low end is exactly a cell bound, to the cell holding
    /// its high end.
    fn touched(&self, low: f64, high: f64, origin: f64, count: usize) -> Option<(usize, usize)> {
        let start = number(low, self.inverse)?;
        let on_bound = (low * self.inverse - SHIFT - start).abs() <= 0.0_f64;
        let first = start - origin - if on_bound { 1.0_f64 } else { 0.0_f64 };
        let last = number(high, self.inverse)? - origin;
        Some((clamped(first, count)?, clamped(last, count)?))
    }

    /// The exact y of a row's centre line.
    pub(in crate::geometry) fn centre(&self, row: usize) -> Option<f64> {
        Some((self.origin[1] + small(row)? + SHIFT + 0.5_f64) * self.side)
    }
}

/// An integral cell offset clamped to `0..count`.
fn clamped(offset: f64, count: usize) -> Option<usize> {
    let top = small(count.checked_sub(1)?)?;
    integral(offset.clamp(0.0_f64, top))
}

/// A power-of-two side giving about `per_edge` square cells per edge over a
/// `width` by `height` envelope, from `MIN_CELLS` up, with at most
/// `MAX_SIDE` cells along either axis; none for an envelope without area.
fn side_for(width: f64, height: f64, edges: usize, per_edge: u32) -> Option<f64> {
    let wanted = u32::try_from(edges).ok()?.checked_mul(per_edge)?.max(MIN_CELLS);
    // Roots first, so a wide envelope's area cannot overflow.
    let mut side = power_at_most(width.sqrt() * height.sqrt() / f64::from(wanted).sqrt())?;
    for _ in 0..MAX_DOUBLINGS {
        if width.max(height) / side <= f64::from(MAX_SIDE) {
            return Some(side);
        }
        side *= 2.0_f64;
    }
    None
}

/// The number of the cell holding `x`, `floor(x / side - SHIFT)` for
/// `inverse = 1 / side`, a power of two, if the product is exact and below
/// the grid's limit. Subtracting `SHIFT` from a negative product rounds
/// where its magnitude crosses a power of two, and can round up to an
/// integer: the floor is then one too high, as the exact lower bound of
/// its cell, above the product, shows.
fn number(x: f64, inverse: f64) -> Option<f64> {
    let scaled = x * inverse;
    let underflow = scaled.abs() < f64::MIN_POSITIVE && x.abs() > 0.0_f64;
    let fits = scaled.abs() < CELL_NUMBER_LIMIT;
    let floor = (!underflow && fits).then(|| (scaled - SHIFT).floor())?;
    Some(if floor + SHIFT > scaled { floor - 1.0_f64 } else { floor })
}

/// A count as an exact binary64 number.
fn small(count: usize) -> Option<f64> {
    Some(f64::from(u32::try_from(count).ok()?))
}

/// The largest power of two at most `r`, for a positive normal `r`.
fn power_at_most(r: f64) -> Option<f64> {
    (r.is_normal() && r > 0.0_f64).then(|| f64::from_bits(r.to_bits() & !FRACTION_MASK))
}

/// A unit in the last place of a positive finite `m`.
fn unit_of(m: f64) -> Option<f64> {
    if !m.is_finite() || m <= 0.0_f64 {
        return None;
    }
    let exponent = (m.to_bits() >> FRACTION_BITS) & EXPONENT_MASK;
    let bits = match exponent.checked_sub(u64::from(FRACTION_BITS)).filter(|&unit| unit > 0) {
        Some(unit) => unit << FRACTION_BITS,
        None => 1_u64 << exponent.saturating_sub(1),
    };
    Some(f64::from_bits(bits))
}

/// A nonnegative integral binary64 number below 2^52 as an index.
fn integral(value: f64) -> Option<usize> {
    if value.abs() <= 0.0_f64 {
        return Some(0);
    }
    let bits = value.to_bits();
    let exponent = (bits >> FRACTION_BITS) & EXPONENT_MASK;
    let shift = u64::from(FRACTION_BITS).checked_sub(exponent.checked_sub(EXPONENT_BIAS)?)?;
    let mantissa = (bits & FRACTION_MASK) | (1_u64 << FRACTION_BITS);
    let positive = value > 0.0_f64;
    let whole = mantissa.checked_shr(u32::try_from(shift).ok()?)?;
    let exact = whole.checked_shl(u32::try_from(shift).ok()?)? == mantissa;
    (positive && exact).then(|| usize::try_from(whole).ok()).flatten()
}

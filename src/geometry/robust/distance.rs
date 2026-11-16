//! Planar point-to-segment distance. Outward-rounded intervals certify
//! endpoint classification and a well-conditioned perpendicular measure.
//! Uncertain cases use exact dot products and a scaled exact determinant,
//! preserving small distances despite cancellation or overflow. Final
//! magnitudes, divisions and square roots remain approximate.

use std::cmp::Ordering;

use super::Interval;
use super::integer::Integer;
use crate::geometry::shape::Point;

/// IEEE binary64 exponent encoding; coordinates become integers at 2^1074.
const MIN_SUBNORMAL_EXPONENT: i32 = -1074;
const MIN_NORMAL_EXPONENT: i32 = -1022;
const MAX_NORMAL_EXPONENT: i32 = 1023;
const EXPONENT_BIAS: i32 = 1023;
const FRACTION_BITS: u32 = 52;
/// This bounds floating error in the fast measure, never a topological
/// tolerance. A wider interval sends the calculation to exact arithmetic.
const RELATIVE_FILTER_WIDTH: f64 = 32.0 * f64::EPSILON;

pub(in crate::geometry) fn point([ax, ay]: Point, [bx, by]: Point) -> f64 {
    norm(ax - bx, ay - by)
}

pub(in crate::geometry) fn segment(p: Point, a: Point, b: Point) -> f64 {
    quick(p, a, b).unwrap_or_else(|| exact(p, a, b))
}

fn quick([px, py]: Point, [ax, ay]: Point, [bx, by]: Point) -> Option<f64> {
    let dx = Interval::difference(bx, ax);
    let dy = Interval::difference(by, ay);
    let ox = Interval::difference(px, ax);
    let oy = Interval::difference(py, ay);
    let start = ox.multiply(dx).add(oy.multiply(dy));
    if start.high < 0.0_f64 {
        return Some(point([px, py], [ax, ay]));
    }
    let end = Interval::difference(px, bx).multiply(dx).add(Interval::difference(py, by).multiply(dy));
    if end.low > 0.0_f64 {
        return Some(point([px, py], [bx, by]));
    }
    if start.low <= 0.0_f64 || end.high >= 0.0_f64 {
        return None;
    }
    let cross = ox.multiply(dy).subtract(oy.multiply(dx));
    if !narrow(cross) {
        return None;
    }
    let length = norm(bx - ax, by - ay);
    let determinant = (px - ax) * (by - ay) - (py - ay) * (bx - ax);
    if !length.is_normal() || !determinant.is_normal() {
        return None;
    }
    Some(determinant.abs() / length)
}

fn narrow(interval: Interval) -> bool {
    if !interval.low.is_finite() || !interval.high.is_finite() {
        return false;
    }
    let absolute = interval.low.abs().min(interval.high.abs());
    let excludes_zero = interval.low > 0.0_f64 || interval.high < 0.0_f64;
    excludes_zero && interval.high - interval.low <= absolute * RELATIVE_FILTER_WIDTH
}

fn exact(p: Point, a: Point, b: Point) -> f64 {
    let [px, py] = p.map(Integer::coordinate);
    let [ax, ay] = a.map(Integer::coordinate);
    let [bx, by] = b.map(Integer::coordinate);
    let dx = bx.subtract(&ax);
    let dy = by.subtract(&ay);
    let ox = px.subtract(&ax);
    let oy = py.subtract(&ay);
    let start = ox.multiply(&dx).add(&oy.multiply(&dy));
    if start.sign() != Ordering::Greater {
        return point(p, a);
    }
    let end = px.subtract(&bx).multiply(&dx).add(&py.subtract(&by).multiply(&dy));
    if end.sign() != Ordering::Less {
        return point(p, b);
    }
    let cross = ox.multiply(&dy).subtract(&oy.multiply(&dx));
    perpendicular(&cross, &dx, &dy)
}

fn perpendicular(cross: &Integer, dx: &Integer, dy: &Integer) -> f64 {
    let (mx, ex) = dx.magnitude();
    let (my, ey) = dy.magnitude();
    let (mc, ec) = cross.magnitude();
    let common = ex.max(ey);
    let length = norm(scaled(mx, ex.saturating_sub(common)), scaled(my, ey.saturating_sub(common)));
    scaled(mc / length, ec.saturating_sub(common).saturating_add(MIN_SUBNORMAL_EXPONENT))
}

fn scaled(value: f64, exponent: i32) -> f64 {
    let first = exponent.clamp(MIN_NORMAL_EXPONENT, MAX_NORMAL_EXPONENT);
    value * power(first) * power(exponent.saturating_sub(first))
}

/// Explicit axis and outward L1 bounds make the norm's range independent
/// of a platform hypot's precision, including a nonzero denominator when
/// a scaled exact direction has a nonzero component.
fn norm(x: f64, y: f64) -> f64 {
    let (x, y) = (x.abs(), y.abs());
    x.hypot(y).max(x.max(y)).min((x + y).next_up())
}

/// Powers of two have exact IEEE encodings. Reconstructing their bits
/// avoids unspecified powi rounding in the exact determinant's scale.
fn power(exponent: i32) -> f64 {
    if exponent < MIN_SUBNORMAL_EXPONENT {
        return 0.0_f64;
    }
    if exponent > MAX_NORMAL_EXPONENT {
        return f64::INFINITY;
    }
    let bits = if exponent < MIN_NORMAL_EXPONENT {
        let shift = u32::try_from(exponent.saturating_sub(MIN_SUBNORMAL_EXPONENT)).unwrap_or_default();
        1_u64 << shift
    } else {
        let biased = u64::try_from(exponent.saturating_add(EXPONENT_BIAS)).unwrap_or_default();
        biased << FRACTION_BITS
    };
    f64::from_bits(bits)
}

#[cfg(test)]
mod tests;

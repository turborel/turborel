//! The least distance between the edges of two chains by an expanding
//! search over a packed hierarchy of one chain's edge envelopes
//! (`spatial/packed.rs`). Each edge of the other chain is measured only
//! against edges whose envelopes come within the best measure so far,
//! widened by more than any rounding of a measure. A skipped pair's true
//! distance exceeds that best by more than its measure can be below its
//! true distance, so the minimum is taken over every pair whose measure
//! could be least, of the same per-pair measures the nested loops take:
//! the result, to the bit, is theirs.

use super::{between, reach, unit_of};
use crate::geometry::crossing::segments;
use crate::geometry::shape::Point;
use crate::spatial::Bounds;
use crate::spatial::packed::{Cursor, Packed};

/// The least measure between an edge of `left` and an edge of `right`, or
/// none where the search's arithmetic cannot stay finite.
pub(super) fn nearest(left: &[Point], right: &[Point]) -> Option<f64> {
    let edges: Vec<(Point, Point)> = segments(right).collect();
    let bounds: Vec<Option<Bounds>> = edges.iter().map(|&(c, d)| envelope(c, d)).collect();
    let packed = Packed::new(&bounds).ok()?;
    let unit = unit(left.iter().chain(right))?;
    let &(first, second) = edges.first()?;
    let mut best = f64::INFINITY;
    for (a, b) in segments(left) {
        best = best.min(between(a, b, first, second));
        let window = widened(envelope(a, b)?, reach(best, unit))?;
        let mut cursor = Cursor::default();
        while let Some(at) = packed.next(window, &mut cursor).ok()? {
            let &(c, d) = edges.get(at)?;
            best = best.min(between(a, b, c, d));
        }
    }
    Some(best)
}

/// A segment's envelope.
fn envelope([ax, ay]: Point, [bx, by]: Point) -> Option<Bounds> {
    Bounds::new([ax.min(bx), ay.min(by)], [ax.max(bx), ay.max(by)]).ok()
}

/// `bounds` grown by `reach` on every side, rounded outward; none if
/// that leaves the finite range.
fn widened(bounds: Bounds, reach: f64) -> Option<Bounds> {
    let [x0, y0] = bounds.lower();
    let [x1, y1] = bounds.upper();
    let lower = [(x0 - reach).next_down(), (y0 - reach).next_down()];
    let upper = [(x1 + reach).next_up(), (y1 + reach).next_up()];
    Bounds::new(lower, upper).ok()
}

/// A unit in the last place of the largest coordinate magnitude, or none
/// for no points.
fn unit<'a>(points: impl Iterator<Item = &'a Point>) -> Option<f64> {
    unit_of(points.flatten().map(|v| v.abs()).fold(0.0_f64, f64::max))
}

#[cfg(test)]
mod tests;

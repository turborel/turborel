//! ST_Distance [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.41], the
//! null value for an empty operand, zero for operands that intersect,
//! else the least Euclidean distance between them. Intersection is
//! decided exactly. A segment or point of one meets one of the other,
//! or a part of one lies inside a surface of the other. The distance
//! itself, an implementation-defined algorithm, is the least
//! point-to-segment distance over pairs of pieces, pruned by monotone
//! chain envelopes, and accurate to a few units in the last place.

use super::Point;
use super::chain::{Candidate, Chain, candidates, chains, halves};
use super::label::Location;
use super::measure::norm;
use super::ring::locate;
use super::segment::{Meeting, meet, on_segment, same};
use super::shape::{Geometry, Parts};

/// The distance, or the null value if either operand is empty.
pub fn distance(first: &Geometry, second: &Geometry) -> Option<f64> {
    let (one, two) = (first.parts(), second.parts());
    if one.is_empty() || two.is_empty() {
        return None;
    }
    let (left, right) = (pieces(&one), pieces(&two));
    if meets(&left, &right) || inside(&one, &two) || inside(&two, &one) {
        return Some(0.0_f64);
    }
    Some(least(&left, &right))
}

/// Every point and polyline of the parts, rings included.
fn pieces(parts: &Parts) -> Vec<Vec<Point>> {
    let mut all: Vec<Vec<Point>> = parts.points.iter().map(|point| vec![*point]).collect();
    all.extend(parts.lines.iter().cloned());
    all.extend(parts.polygons.iter().flatten().cloned());
    all.into_iter().map(|line| super::noding::deduplicated(&line)).filter(|line| !line.is_empty()).collect()
}

fn segment_or_point(lines: &[Vec<Point>], line: usize, index: usize) -> (Point, Option<Point>) {
    let vertices = lines.get(line).map_or(&[][..], Vec::as_slice);
    (vertices.get(index).copied().unwrap_or_default(), vertices.get(index.saturating_add(1)).copied())
}

/// Whether a piece of one operand meets a piece of the other.
fn meets(left: &[Vec<Point>], right: &[Vec<Point>]) -> bool {
    let split = left.len();
    let all: Vec<Vec<Point>> = left.iter().chain(right).cloned().collect();
    candidates(&all, &[]).into_iter().any(|candidate| {
        let Candidate::Pair(first, second) = candidate else { return false };
        if (first.line < split) == (second.line < split) {
            return false;
        }
        match (segment_or_point(&all, first.line, first.index), segment_or_point(&all, second.line, second.index)) {
            ((a, Some(b)), (c, Some(d))) => meet(a, b, c, d) != Meeting::Apart,
            ((a, Some(b)), (point, None)) | ((point, None), (a, Some(b))) => on_segment(point, a, b),
            ((point, None), (other, None)) => same(point, other),
        }
    })
}

/// Whether some part of `inner` lies inside a polygon of `outer`; with
/// no boundaries meeting, one vertex of each part decides for all of it.
fn inside(inner: &Parts, outer: &Parts) -> bool {
    let firsts = inner
        .points
        .iter()
        .chain(inner.lines.iter().filter_map(|line| line.first()))
        .chain(inner.polygons.iter().filter_map(|rings| rings.first().and_then(|ring| ring.first())));
    firsts.into_iter().any(|point| {
        outer.polygons.iter().any(|rings| {
            let Some((shell, holes)) = rings.split_first() else { return false };
            locate(*point, shell) == Location::Interior && holes.iter().all(|hole| locate(*point, hole) == Location::Exterior)
        })
    })
}

/// The distance from `point` to the closed segment from `a` to `b`.
fn to_segment(point: Point, a: Point, b: Point) -> f64 {
    let ([px, py], [ax, ay], [bx, by]) = (point, a, b);
    let (dx, dy) = (bx - ax, by - ay);
    let squared = dx * dx + dy * dy;
    let along = if squared > 0.0_f64 { (((px - ax) * dx + (py - ay) * dy) / squared).clamp(0.0_f64, 1.0_f64) } else { 0.0_f64 };
    norm(px - (ax + along * dx), py - (ay + along * dy))
}

fn piece_distance(first: (Point, Option<Point>), second: (Point, Option<Point>)) -> f64 {
    match (first, second) {
        ((a, Some(b)), (c, Some(d))) => to_segment(a, c, d).min(to_segment(b, c, d)).min(to_segment(c, a, b)).min(to_segment(d, a, b)),
        ((a, Some(b)), (point, None)) | ((point, None), (a, Some(b))) => to_segment(point, a, b),
        (([ax, ay], None), ([bx, by], None)) => norm(ax - bx, ay - by),
    }
}

/// The least piece distance. Chain pairs go nearest first, each split
/// until single pieces, and any pair farther apart than the best so far
/// is skipped whole.
fn least(left: &[Vec<Point>], right: &[Vec<Point>]) -> f64 {
    let (first, second) = (chains(left), chains(right));
    let mut pairs: Vec<(f64, Chain, Chain)> = Vec::with_capacity(first.len().saturating_mul(second.len()));
    for one in &first {
        let bounds = one.envelope(left);
        pairs.extend(second.iter().map(|two| (gap(bounds, two.envelope(right)), *one, *two)));
    }
    pairs.sort_by(|nearer, farther| nearer.0.total_cmp(&farther.0));
    let mut best = f64::INFINITY;
    for (apart, one, two) in pairs {
        if apart >= best {
            break;
        }
        let mut stack = vec![(one, two)];
        while let Some((a, b)) = stack.pop() {
            if gap(a.envelope(left), b.envelope(right)) >= best {
                continue;
            }
            match (a.single(), b.single()) {
                (true, true) => {
                    best = best.min(piece_distance(segment_or_point(left, a.line, a.start), segment_or_point(right, b.line, b.start)));
                }
                (false, _) => {
                    let (low, high) = halves(a);
                    stack.extend([(low, b), (high, b)]);
                }
                (true, false) => {
                    let (low, high) = halves(b);
                    stack.extend([(a, low), (a, high)]);
                }
            }
        }
    }
    best
}

fn gap((low, high): (Point, Point), (other_low, other_high): (Point, Point)) -> f64 {
    let ([lx, ly], [hx, hy], [ox, oy], [px, py]) = (low, high, other_low, other_high);
    let x = (lx - px).max(ox - hx).max(0.0_f64);
    let y = (ly - py).max(oy - hy).max(0.0_f64);
    norm(x, y)
}

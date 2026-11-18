//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §4.3, §5.1.43 to §5.1.54]
//! The exact predicates topology-geometry decides by: whether a point is
//! a curve's, whether two curves meet away from given points, where a
//! point lies against a ring, and which way a ring turns. Each is built on
//! the exact orientation of three points (`geometry/robust.rs`), so a
//! decision never depends on rounding.

use std::cmp::Ordering;

use super::Point;
use crate::spatial::planar::orient2d as orient;

/// Where a point lies against a closed ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Side {
    /// Inside it.
    Inside,
    /// On it.
    On,
    /// Outside it.
    Outside,
}

/// Whether `a` and `b` are the same position, exactly. Stored coordinates
/// have no negative zero, so bits are values.
pub(super) fn same(a: Point, b: Point) -> bool {
    a.iter().zip(&b).all(|(x, y)| x.total_cmp(y) == Ordering::Equal)
}

/// Whether `v` lies between `a` and `b` inclusive.
fn within(v: f64, a: f64, b: f64) -> bool {
    a.min(b) <= v && v <= a.max(b)
}

/// Whether `p` is a point of the closed segment from `a` to `b`.
pub(super) fn on_segment(p: Point, a: Point, b: Point) -> bool {
    let [px, py] = p;
    let ([ax, ay], [bx, by]) = (a, b);
    within(px, ax, bx) && within(py, ay, by) && orient(a, b, p) == Ordering::Equal
}

/// Whether `p` is a point of the line through `points`.
pub(super) fn on_line(p: Point, points: &[Point]) -> bool {
    points.windows(2).any(|w| if let [a, b] = w { on_segment(p, *a, *b) } else { false })
}

/// Whether `p` is a point of the line through `points` other than one of
/// its two ends.
pub(super) fn inside_line(p: Point, points: &[Point]) -> bool {
    let end = |q: Option<&Point>| q.is_some_and(|q| same(*q, p));
    on_line(p, points) && !end(points.first()) && !end(points.last())
}

/// The points the closed segments `a`-`b` and `c`-`d` share: none if they
/// do not meet, one if they meet in a point, and both of its ends if they
/// overlap along a part of positive length.
fn meeting(a: Point, b: Point, c: Point, d: Point) -> Vec<Point> {
    let (o1, o2) = (orient(a, b, c), orient(a, b, d));
    let (o3, o4) = (orient(c, d, a), orient(c, d, b));
    if proper(o1, o2) && proper(o3, o4) {
        // A crossing inside both, which is a point no end of either.
        return vec![crossing([a, b], [c, d])];
    }
    let mut shared: Vec<Point> = Vec::new();
    for (p, on) in [(c, on_segment(c, a, b)), (d, on_segment(d, a, b)), (a, on_segment(a, c, d)), (b, on_segment(b, c, d))] {
        if on && !shared.iter().any(|q| same(*q, p)) {
            shared.push(p);
        }
    }
    shared
}

/// Whether two orientations are strictly opposite.
fn proper(x: Ordering, y: Ordering) -> bool {
    matches!((x, y), (Ordering::Less, Ordering::Greater) | (Ordering::Greater, Ordering::Less))
}

/// The point where the segments `a`-`b` and `c`-`d` cross, rounded; only a
/// witness for a message, never decided on.
fn crossing(first: [Point; 2], second: [Point; 2]) -> Point {
    let [[ax, ay], [bx, by]] = first;
    let [[cx, cy], [dx, dy]] = second;
    let denominator = (bx - ax) * (dy - cy) - (by - ay) * (dx - cx);
    let along = ((cx - ax) * (dy - cy) - (cy - ay) * (dx - cx)) / denominator;
    [ax + along * (bx - ax), ay + along * (by - ay)]
}

/// Whether the closed boxes of two segments overlap: a cheap test first.
fn boxes_meet([a, b]: [Point; 2], [c, d]: [Point; 2]) -> bool {
    let ([ax, ay], [bx, by], [cx, cy], [dx, dy]) = (a, b, c, d);
    ax.max(bx) >= cx.min(dx) && cx.max(dx) >= ax.min(bx) && ay.max(by) >= cy.min(dy) && cy.max(dy) >= ay.min(by)
}

/// A point the lines through `first` and `second` share that is not one
/// of `allowed`; none if every point they share is one of them.
pub(super) fn meet_except(first: &[Point], second: &[Point], allowed: &[Point]) -> Option<Point> {
    for w in first.windows(2) {
        let [a, b] = w else { continue };
        for v in second.windows(2) {
            let [c, d] = v else { continue };
            if !boxes_meet([*a, *b], [*c, *d]) {
                continue;
            }
            let shared = meeting(*a, *b, *c, *d);
            if let Some(p) = shared.iter().find(|p| shared.len() > 1 || !allowed.iter().any(|q| same(**p, *q))) {
                return Some(*p);
            }
        }
    }
    None
}

/// [§5.1.8] Whether the line through `points` is simple: it passes through
/// no point twice, but for a closed line's end, and has a segment.
pub(super) fn is_simple(points: &[Point]) -> bool {
    let segments: Vec<[Point; 2]> = points.windows(2).filter_map(|w| if let [a, b] = w { Some([*a, *b]) } else { None }).collect();
    if segments.iter().all(|[a, b]| same(*a, *b)) {
        return false;
    }
    let closed = matches!((points.first(), points.last()), (Some(f), Some(l)) if same(*f, *l));
    let count = segments.len();
    segments
        .iter()
        .enumerate()
        .all(|(i, s)| segments.iter().enumerate().skip(i.saturating_add(1)).all(|(j, t)| simple_pair((i, *s), (j, *t), (count, closed))))
}

/// Whether segments `s`, number `i`, and `t`, number `j > i`, of a line of
/// `count` segments, `closed` or not, meet only where the line allows.
fn simple_pair((i, first): (usize, [Point; 2]), (j, second): (usize, [Point; 2]), (count, closed): (usize, bool)) -> bool {
    let [start, end] = first;
    let [other_start, other_end] = second;
    if !boxes_meet(first, second) {
        return true;
    }
    let shared = meeting(start, end, other_start, other_end);
    let adjacent = j == i.saturating_add(1);
    let wraps = closed && i == 0 && j.saturating_add(1) == count;
    match shared.as_slice() {
        [] => true,
        [p] => (adjacent && same(*p, end)) || (wraps && same(*p, start)),
        _ => false,
    }
}

/// [§5.1.47] Where `p` lies against the closed ring `ring`, decided by
/// counting crossings of a ray to the right of it, each decided exactly.
pub(super) fn side(p: Point, ring: &[Point]) -> Side {
    let mut inside = false;
    for w in ring.windows(2) {
        let [a, b] = w else { continue };
        if on_segment(p, *a, *b) {
            return Side::On;
        }
        let ([_, ay], [_, by], [_, py]) = (*a, *b, p);
        if (ay > py) != (by > py) {
            // Upward, p is left of a-b where the ray crosses it; downward,
            // right of it.
            let turn = orient(*a, *b, p);
            let crosses = if by > ay { turn == Ordering::Greater } else { turn == Ordering::Less };
            inside ^= crosses;
        }
    }
    if inside { Side::Inside } else { Side::Outside }
}

/// Whether the closed ring `ring` turns counterclockwise: whether the area
/// it bounds is on its left. A ring of no area turns neither way.
pub(super) fn counterclockwise(ring: &[Point]) -> bool {
    let Some((at, low)) = ring.iter().enumerate().skip(1).min_by(|(_, [px, py]), (_, [qx, qy])| py.total_cmp(qy).then(px.total_cmp(qx)))
    else {
        return false;
    };
    let count = ring.iter().skip(1).filter(|p| same(**p, *low)).count();
    if count == 1 {
        let before = ring.iter().take(at).rev().chain(ring.iter().skip(1).rev()).find(|p| !same(**p, *low));
        let after = ring.iter().skip(at.saturating_add(1)).chain(ring.iter().skip(1)).find(|p| !same(**p, *low));
        if let (Some(b), Some(a)) = (before, after) {
            match orient(*b, *low, *a) {
                Ordering::Greater => return true,
                Ordering::Less => return false,
                Ordering::Equal => {}
            }
        }
    }
    area(ring) > 0.0_f64
}

/// The signed area of the closed ring `ring`, positive of one that turns
/// counterclockwise, summed about its first point.
pub(super) fn area(ring: &[Point]) -> f64 {
    let Some([ox, oy]) = ring.first().copied() else { return 0.0_f64 };
    let twice: f64 = ring
        .windows(2)
        .filter_map(|w| if let [[ax, ay], [bx, by]] = w { Some((ax - ox) * (by - oy) - (bx - ox) * (ay - oy)) } else { None })
        .sum();
    twice / 2.0_f64
}

/// The box of `points`: its least and greatest x and y.
pub(super) fn bounds(points: &[Point]) -> Option<[f64; 4]> {
    let [x, y] = *points.first()?;
    Some(points.iter().fold([x, y, x, y], |[a, b, c, d], [px, py]| [a.min(*px), b.min(*py), c.max(*px), d.max(*py)]))
}

#[cfg(test)]
mod tests;

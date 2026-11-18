//! \[DESIGN\] Exact classification of how two line segments meet, and the
//! rounded position of a proper crossing. Only the crossing's position
//! is approximate; whether and how segments meet is decided exactly.

use std::cmp::Ordering;

use super::Point;
use super::dyadic::{Dyadic, quotient};
use super::predicates::orient2d;

/// How two closed segments meet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Meeting {
    /// They have no point in common.
    Apart,
    /// They share exactly this point, a vertex of one or both.
    Touch(Point),
    /// Collinear, they share the segment between these points.
    Overlap(Point, Point),
    /// They cross at one point interior to both.
    Cross,
}

/// Whether two points are the same position; signed zeros are equal.
pub(crate) fn same(left: Point, right: Point) -> bool {
    let [lx, ly] = left;
    let [rx, ry] = right;
    equal(lx, rx) && equal(ly, ry)
}

/// Numeric equality of two finite coordinates without `==` on floats.
pub(crate) fn equal(left: f64, right: f64) -> bool {
    left.partial_cmp(&right) == Some(Ordering::Equal)
}

/// The lexicographic order of positions, x first. Along a line it is the
/// order of collinear points.
pub(crate) fn lexicographic(left: Point, right: Point) -> Ordering {
    let [lx, ly] = left;
    let [rx, ry] = right;
    compare(lx, rx).then_with(|| compare(ly, ry))
}

/// Numeric order of finite coordinates, signed zeros equal.
pub(crate) fn compare(left: f64, right: f64) -> Ordering {
    if left < right {
        Ordering::Less
    } else if left > right {
        Ordering::Greater
    } else {
        Ordering::Equal
    }
}

/// Whether `point` lies in the closed axis-aligned box of `a` and `b`.
pub(crate) fn within([x, y]: Point, [ax, ay]: Point, [bx, by]: Point) -> bool {
    x >= ax.min(bx) && x <= ax.max(bx) && y >= ay.min(by) && y <= ay.max(by)
}

/// Whether `point` lies on the closed segment from `a` to `b`.
pub(crate) fn on_segment(point: Point, a: Point, b: Point) -> bool {
    within(point, a, b) && orient2d(a, b, point) == Ordering::Equal
}

/// How the segments `a`–`b` and `c`–`d`, each of nonzero length, meet.
pub(crate) fn meet(a: Point, b: Point, c: Point, d: Point) -> Meeting {
    if !boxes_meet(a, b, c, d) {
        return Meeting::Apart;
    }
    let first = orient2d(a, b, c);
    let second = orient2d(a, b, d);
    if first == Ordering::Equal && second == Ordering::Equal {
        return collinear(a, b, c, d);
    }
    let third = orient2d(c, d, a);
    let fourth = orient2d(c, d, b);
    if first == second || third == fourth {
        return Meeting::Apart;
    }
    let touches = [(first, c, a, b), (second, d, a, b), (third, a, c, d), (fourth, b, c, d)];
    for (order, point, start, end) in touches {
        if order == Ordering::Equal {
            return if within(point, start, end) { Meeting::Touch(point) } else { Meeting::Apart };
        }
    }
    Meeting::Cross
}

fn boxes_meet([ax, ay]: Point, [bx, by]: Point, [cx, cy]: Point, [dx, dy]: Point) -> bool {
    ax.min(bx) <= cx.max(dx) && cx.min(dx) <= ax.max(bx) && ay.min(by) <= cy.max(dy) && cy.min(dy) <= ay.max(by)
}

/// Collinear segments share the stretch between the greater of their
/// least points and the lesser of their greatest, if it is not reversed.
fn collinear(a: Point, b: Point, c: Point, d: Point) -> Meeting {
    let order = |p: Point, q: Point| if lexicographic(p, q) == Ordering::Greater { (q, p) } else { (p, q) };
    let (ab_low, ab_high) = order(a, b);
    let (cd_low, cd_high) = order(c, d);
    let low = if lexicographic(ab_low, cd_low) == Ordering::Less { cd_low } else { ab_low };
    let high = if lexicographic(ab_high, cd_high) == Ordering::Less { ab_high } else { cd_high };
    match lexicographic(low, high) {
        Ordering::Less => Meeting::Overlap(low, high),
        Ordering::Equal => Meeting::Touch(low),
        Ordering::Greater => Meeting::Apart,
    }
}

/// The crossing point of segments that cross, the exact rational
/// solution `a + t (b - a)`, `t` the ratio of two exact determinants,
/// rounded to the nearest binary64 position on each axis, so the hot
/// pixel of the result holds the exact crossing.
pub(crate) fn crossing(a: Point, b: Point, c: Point, d: Point) -> Point {
    let [ax, ay] = a.map(Dyadic::of);
    let [bx, by] = b.map(Dyadic::of);
    let [cx, cy] = c.map(Dyadic::of);
    let [dx, dy] = d.map(Dyadic::of);
    let (abx, aby) = (bx.minus(&ax), by.minus(&ay));
    let (cdx, cdy) = (dx.minus(&cx), dy.minus(&cy));
    let (acx, acy) = (cx.minus(&ax), cy.minus(&ay));
    let numerator = acx.times(&cdy).minus(&acy.times(&cdx));
    let mut denominator = abx.times(&cdy).minus(&aby.times(&cdx));
    let mut across = ax.times(&denominator).plus(&abx.times(&numerator));
    let mut up = ay.times(&denominator).plus(&aby.times(&numerator));
    if denominator.sign() == Ordering::Less {
        let zero = Dyadic::of(0.0_f64);
        (denominator, across, up) = (zero.minus(&denominator), zero.minus(&across), zero.minus(&up));
    }
    let point = [nearest(&across, &denominator), nearest(&up, &denominator)];
    clamp(point, [a, b, c, d])
}

/// Steps before the nearest search stops; the first guess is within a
/// few units in the last place.
const NEAREST_STEPS: usize = 16;

/// The binary64 value nearest the exact quotient of `numerator` by a
/// positive `denominator`. The guess is stepped until the quotient lies
/// between the midpoints to its neighbours.
fn nearest(numerator: &Dyadic, denominator: &Dyadic) -> f64 {
    let mut value = quotient(numerator, denominator);
    let beyond = |bound: f64, other: f64| numerator.minus(&Dyadic::of(bound).plus(&Dyadic::of(other)).halved().times(denominator)).sign();
    for _ in 0..NEAREST_STEPS {
        if beyond(value, value.next_up()) == Ordering::Greater {
            value = value.next_up();
        } else if beyond(value, value.next_down()) == Ordering::Less {
            value = value.next_down();
        } else {
            break;
        }
    }
    value
}

fn clamp([across, up]: Point, [a, b, c, d]: [Point; 4]) -> Point {
    let [ax, ay] = a;
    let [bx, by] = b;
    let [cx, cy] = c;
    let [dx, dy] = d;
    let low_x = ax.min(bx).max(cx.min(dx));
    let high_x = ax.max(bx).min(cx.max(dx));
    let low_y = ay.min(by).max(cy.min(dy));
    let high_y = ay.max(by).min(cy.max(dy));
    [across.max(low_x).min(high_x), up.max(low_y).min(high_y)]
}

/// A closed box enclosing a point's hot pixel, from its neighbouring
/// binary64 values, for envelope tests.
pub(crate) fn pixel([x, y]: Point) -> (Point, Point) {
    ([x.next_down(), y.next_down()], [x.next_up(), y.next_up()])
}

/// The exact corners of a point's hot pixel. On each axis it is the closed
/// interval of reals nearer to the coordinate than to its neighbours,
/// bounded by the midpoints. Pixels tile the plane, meeting only on
/// their edges.
fn corners([x, y]: Point) -> [[Dyadic; 2]; 4] {
    let middle = |left: f64, right: f64| Dyadic::of(left).plus(&Dyadic::of(right)).halved();
    let (low_x, high_x) = (middle(x.next_down(), x), middle(x, x.next_up()));
    let (low_y, high_y) = (middle(y.next_down(), y), middle(y, y.next_up()));
    [[low_x.clone(), low_y.clone()], [high_x.clone(), low_y], [high_x, high_y.clone()], [low_x, high_y]]
}

/// Whether the closed segment from `a` to `b` meets the hot pixel of
/// `centre`, decided exactly. A binary64 coordinate lies in a pixel's
/// interval only if it is the centre's, so the segment's box meets the
/// pixel's exactly when it holds the centre; then the segment meets it
/// unless all four corners lie strictly on one side of its line.
pub(crate) fn meets_pixel(a: Point, b: Point, centre: Point) -> bool {
    let ([ax, ay], [bx, by], [cx, cy]) = (a, b, centre);
    let spans = |low: f64, high: f64, value: f64| low.min(high) <= value && low.max(high) >= value;
    if !spans(ax, bx, cx) || !spans(ay, by, cy) {
        return false;
    }
    if same(a, b) {
        return same(a, centre);
    }
    let ([start_x, start_y], [end_x, end_y]) = (a.map(Dyadic::of), b.map(Dyadic::of));
    let (run, rise) = (end_x.minus(&start_x), end_y.minus(&start_y));
    let sides = corners(centre).map(|[x, y]| run.times(&y.minus(&start_y)).minus(&rise.times(&x.minus(&start_x))).sign());
    !(sides.iter().all(|side| *side == Ordering::Greater) || sides.iter().all(|side| *side == Ordering::Less))
}

#[cfg(test)]
mod tests;

//! A brute-force exact evaluator for geometries with small integer
//! coordinates. It shares no code with the engine. Positions are
//! rational, kept as integer triples over a common positive
//! denominator, and every test is against a segment or point with
//! integer coordinates, so integer arithmetic decides it exactly.
//!
//! The arrangement of both operands' segments splits the plane into
//! vertices, open edges and open faces, each with one location in each
//! operand. The oracle samples every one of them. It takes each vertex
//! and crossing, and the middle of each piece of each segment between
//! them. For faces it takes, on the vertical line midway between each two
//! consecutive vertex abscissae, the middle between each two consecutive
//! segment heights, and points beyond the extremes.

use std::cmp::Ordering;

use super::super::Geometry;
use super::super::label::Location;

/// The scale of a nudge, 2^30.
const NUDGE: i128 = 1 << 30;

/// A rational position `(x / d, y / d)`, `d` positive.
#[derive(Clone, Copy, Debug)]
pub(super) struct Rational {
    pub(super) x: i128,
    pub(super) y: i128,
    pub(super) d: i128,
}

/// An integer position.
type Whole = [i128; 2];

fn gcd(left: i128, right: i128) -> i128 {
    let (mut a, mut b) = (left.checked_abs().unwrap(), right.checked_abs().unwrap());
    while b != 0 {
        (a, b) = (b, a.checked_rem(b).unwrap());
    }
    a
}

impl Rational {
    pub(super) fn new(x: i128, y: i128, d: i128) -> Self {
        let sign = if d < 0 { -1 } else { 1 };
        let divisor = gcd(gcd(x, y), d).max(1).checked_mul(sign).unwrap();
        Self { x: x.checked_div(divisor).unwrap(), y: y.checked_div(divisor).unwrap(), d: d.checked_div(divisor).unwrap() }
    }

    fn whole([x, y]: Whole) -> Self {
        Self { x, y, d: 1 }
    }

    fn same(self, other: Self) -> bool {
        self.x.checked_mul(other.d) == other.x.checked_mul(self.d) && self.y.checked_mul(other.d) == other.y.checked_mul(self.d)
    }

    fn order(self, other: Self) -> Ordering {
        let x = self.x.checked_mul(other.d).unwrap().cmp(&other.x.checked_mul(self.d).unwrap());
        x.then_with(|| self.y.checked_mul(other.d).unwrap().cmp(&other.y.checked_mul(self.d).unwrap()))
    }

    pub(super) fn middle(self, other: Self) -> Self {
        let x = self.x.checked_mul(other.d).unwrap().checked_add(other.x.checked_mul(self.d).unwrap()).unwrap();
        let y = self.y.checked_mul(other.d).unwrap().checked_add(other.y.checked_mul(self.d).unwrap()).unwrap();
        Self::new(x, y, self.d.checked_mul(other.d).unwrap().checked_mul(2).unwrap())
    }

    /// The position moved by `direction / 2^30`.
    pub(super) fn nudged(self, [dx, dy]: [i128; 2]) -> Self {
        let x = self.x.checked_mul(NUDGE).unwrap().checked_add(dx.checked_mul(self.d).unwrap()).unwrap();
        let y = self.y.checked_mul(NUDGE).unwrap().checked_add(dy.checked_mul(self.d).unwrap()).unwrap();
        Self::new(x, y, self.d.checked_mul(NUDGE).unwrap())
    }

    /// The direction to an integer position, scaled by the denominator
    /// so it is whole.
    pub(super) fn toward(self, [vx, vy]: [i128; 2]) -> [i128; 2] {
        [vx.checked_mul(self.d).unwrap().checked_sub(self.x).unwrap(), vy.checked_mul(self.d).unwrap().checked_sub(self.y).unwrap()]
    }

    /// The position moved by `direction / (d 2^30)`, a direction from
    /// [`Self::toward`] or a sum of them.
    pub(super) fn along(self, [dx, dy]: [i128; 2]) -> Self {
        let x = self.x.checked_mul(NUDGE).unwrap().checked_add(dx).unwrap();
        let y = self.y.checked_mul(NUDGE).unwrap().checked_add(dy).unwrap();
        Self::new(x, y, self.d.checked_mul(NUDGE).unwrap())
    }

    /// The nearest binary64 position, near enough for sampling.
    pub(super) fn approximate(self) -> [f64; 2] {
        let real = |value: i128| value.to_string().parse::<f64>().unwrap();
        [real(self.x) / real(self.d), real(self.y) / real(self.d)]
    }
}

/// The sign of the turn from `a` to `b` to `p`.
fn orient(a: Whole, b: Whole, p: Rational) -> Ordering {
    let ([ax, ay], [bx, by]) = (a, b);
    let px = p.x.checked_sub(ax.checked_mul(p.d).unwrap()).unwrap();
    let py = p.y.checked_sub(ay.checked_mul(p.d).unwrap()).unwrap();
    let left = bx.checked_sub(ax).unwrap().checked_mul(py).unwrap();
    let right = by.checked_sub(ay).unwrap().checked_mul(px).unwrap();
    left.cmp(&right)
}

fn between(low: i128, high: i128, value: i128, d: i128) -> bool {
    low.min(high).checked_mul(d).unwrap() <= value && value <= low.max(high).checked_mul(d).unwrap()
}

fn on_segment(p: Rational, a: Whole, b: Whole) -> bool {
    let ([ax, ay], [bx, by]) = (a, b);
    orient(a, b, p) == Ordering::Equal && between(ax, bx, p.x, p.d) && between(ay, by, p.y, p.d)
}

/// The exact meeting point of two segments that meet in one point, or
/// nothing.
fn meeting(first: (Whole, Whole), second: (Whole, Whole)) -> Vec<Rational> {
    let ((start, end), (other_start, other_end)) = (first, second);
    let ([ax, ay], [bx, by], [cx, cy], [dx, dy]) = (start, end, other_start, other_end);
    let (run, rise) = (bx.checked_sub(ax).unwrap(), by.checked_sub(ay).unwrap());
    let (other_run, other_rise) = (dx.checked_sub(cx).unwrap(), dy.checked_sub(cy).unwrap());
    let denominator = run.checked_mul(other_rise).unwrap().checked_sub(rise.checked_mul(other_run).unwrap()).unwrap();
    if denominator == 0 {
        return Vec::new();
    }
    let (gap_x, gap_y) = (cx.checked_sub(ax).unwrap(), cy.checked_sub(ay).unwrap());
    let along = gap_x.checked_mul(other_rise).unwrap().checked_sub(gap_y.checked_mul(other_run).unwrap()).unwrap();
    let across = ax.checked_mul(denominator).unwrap().checked_add(run.checked_mul(along).unwrap()).unwrap();
    let up = ay.checked_mul(denominator).unwrap().checked_add(rise.checked_mul(along).unwrap()).unwrap();
    let point = Rational::new(across, up, denominator);
    if on_segment(point, start, end) && on_segment(point, other_start, other_end) { vec![point] } else { Vec::new() }
}

/// One operand as integer points, polylines and polygons.
pub(super) struct Shape {
    points: Vec<Whole>,
    lines: Vec<Vec<Whole>>,
    polygons: Vec<Vec<Vec<Whole>>>,
}

fn whole(value: f64) -> i128 {
    format!("{value}").parse().unwrap()
}

impl Shape {
    pub(super) fn of(geometry: &Geometry) -> Self {
        let parts = geometry.parts();
        let convert = |line: &Vec<[f64; 2]>| line.iter().map(|point| point.map(whole)).collect::<Vec<Whole>>();
        Self {
            points: parts.points.iter().map(|point| point.map(whole)).collect(),
            lines: parts.lines.iter().map(convert).collect(),
            polygons: parts.polygons.iter().map(|rings| rings.iter().map(convert).collect()).collect(),
        }
    }

    fn segments(&self) -> Vec<(Whole, Whole)> {
        let polylines = self.lines.iter().chain(self.polygons.iter().flatten());
        polylines.flat_map(|line| line.windows(2).map(|pair| (pair[0], pair[1]))).filter(|(a, b)| a != b).collect()
    }

    fn vertices(&self) -> Vec<Whole> {
        let polylines = self.lines.iter().chain(self.polygons.iter().flatten()).flatten();
        self.points.iter().chain(polylines).copied().collect()
    }

    /// Where a position lies, by the mod 2 rule for curve ends and the
    /// even-odd rule for rings.
    pub(super) fn locate(&self, p: Rational) -> Location {
        let rings: Vec<&Vec<Whole>> = self.polygons.iter().flatten().collect();
        if rings.iter().any(|ring| ring.windows(2).any(|pair| on_segment(p, pair[0], pair[1]))) {
            return Location::Boundary;
        }
        if inside(&rings, p) {
            return Location::Interior;
        }
        if self.lines.iter().any(|line| line.windows(2).any(|pair| on_segment(p, pair[0], pair[1]))) {
            let ends = self
                .lines
                .iter()
                .filter(|line| line.first() != line.last())
                .flat_map(|line| [line[0], line[line.len().checked_sub(1).unwrap()]]);
            let count = ends.filter(|end| Rational::whole(*end).same(p)).count();
            return if count.checked_rem(2) == Some(1) { Location::Boundary } else { Location::Interior };
        }
        if self.points.iter().any(|point| Rational::whole(*point).same(p)) {
            return Location::Interior;
        }
        Location::Exterior
    }
}

fn inside(rings: &[&Vec<Whole>], p: Rational) -> bool {
    let mut odd = false;
    for pair in rings.iter().flat_map(|ring| ring.windows(2)) {
        let ([_, ay], [_, by]) = (pair[0], pair[1]);
        let (above_a, above_b) = (ay.checked_mul(p.d).unwrap() > p.y, by.checked_mul(p.d).unwrap() > p.y);
        if above_a != above_b {
            let side = orient(pair[0], pair[1], p);
            let rightward = if by > ay { side == Ordering::Greater } else { side == Ordering::Less };
            odd ^= rightward;
        }
    }
    odd
}

/// Beyond every coordinate of the small inputs.
const FAR: i128 = 1_000;

/// A sample of one stratum of the arrangement, with its dimension.
#[derive(Clone, Copy, Debug)]
pub(super) struct Sample {
    pub(super) at: Rational,
    pub(super) dimension: i8,
}

/// The vertices and crossings of both operands' segments.
fn critical(segments: &[(Whole, Whole)], vertices: &[Whole]) -> Vec<Rational> {
    let mut points: Vec<Rational> = vertices.iter().copied().map(Rational::whole).collect();
    for (index, (a, b)) in segments.iter().enumerate() {
        for (c, d) in segments.iter().skip(index.checked_add(1).unwrap()) {
            points.extend(meeting((*a, *b), (*c, *d)));
        }
    }
    points.sort_by(|left, right| left.order(*right));
    points.dedup_by(|left, right| left.same(*right));
    points
}

/// A sample of every vertex, open edge and open face of the arrangement
/// of both operands, and the arrangement's integer positions, which
/// give the directions for nudging.
pub(super) fn samples(first: &Shape, second: &Shape) -> (Vec<Sample>, Vec<Whole>) {
    let mut segments = first.segments();
    segments.extend(second.segments());
    let mut vertices = first.vertices();
    vertices.extend(second.vertices());
    let points = critical(&segments, &vertices);
    let mut out: Vec<Sample> = points.iter().map(|at| Sample { at: *at, dimension: 0 }).collect();
    for (a, b) in &segments {
        let mut on: Vec<Rational> = points.iter().copied().filter(|point| on_segment(*point, *a, *b)).collect();
        on.sort_by(|left, right| left.order(*right));
        out.extend(on.windows(2).map(|pair| Sample { at: pair[0].middle(pair[1]), dimension: 1 }));
    }
    out.extend(faces(&segments, &points));
    (out, vertices)
}

fn faces(segments: &[(Whole, Whole)], points: &[Rational]) -> Vec<Sample> {
    let mut abscissae: Vec<Rational> = points.iter().map(|point| Rational::new(point.x, 0, point.d)).collect();
    abscissae.sort_by(|left, right| left.order(*right));
    abscissae.dedup_by(|left, right| left.same(*right));
    let (Some(first), Some(last)) = (abscissae.first().copied(), abscissae.last().copied()) else { return Vec::new() };
    let mut lines: Vec<Rational> = abscissae.windows(2).map(|pair| pair[0].middle(pair[1])).collect();
    lines.push(Rational::new(first.x.checked_sub(first.d).unwrap(), 0, first.d));
    lines.push(Rational::new(last.x.checked_add(last.d).unwrap(), 0, last.d));
    let mut out = Vec::new();
    for line in lines {
        let mut heights: Vec<Rational> = segments.iter().flat_map(|(a, b)| height(*a, *b, line)).collect();
        let far = line.d.checked_mul(FAR).unwrap();
        heights.push(Rational::new(line.x, far.checked_neg().unwrap(), line.d));
        heights.push(Rational::new(line.x, far, line.d));
        heights.sort_by(|left, right| left.order(*right));
        heights.dedup_by(|left, right| left.same(*right));
        out.extend(heights.windows(2).map(|pair| Sample { at: pair[0].middle(pair[1]), dimension: 2 }));
    }
    out
}

/// Where a segment that is not vertical crosses the vertical line at
/// `line.x / line.d`, strictly between its ends.
fn height(a: Whole, b: Whole, line: Rational) -> Vec<Rational> {
    let ([ax, ay], [bx, by]) = (a, b);
    if ax == bx || !between(ax, bx, line.x, line.d) {
        return Vec::new();
    }
    let width = bx.checked_sub(ax).unwrap();
    let offset = line.x.checked_sub(ax.checked_mul(line.d).unwrap()).unwrap();
    let rise = by.checked_sub(ay).unwrap().checked_mul(offset).unwrap();
    let y = ay.checked_mul(line.d).unwrap().checked_mul(width).unwrap().checked_add(rise).unwrap();
    let d = line.d.checked_mul(width).unwrap();
    vec![Rational::new(line.x.checked_mul(width).unwrap(), y, d)]
}

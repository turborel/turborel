//! NURBS evaluation, ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.7.1: the
//! knot vector is the knots repeated by multiplicity, its length the
//! control points plus the degree plus one (D16); control points hold
//! weighted coordinates (D17), so de Boor's algorithm runs on the
//! homogeneous points (wx, wy, wz, w) and divides once at the end.

use super::fault::{Fault, fault};
use super::numeric::V3;
use super::types::Nurbs;
use crate::error::Result;

/// The citation of the type's definition.
const CLAUSE: &str = "§7.7.1";
/// The largest degree accepted: the WKB degree is one byte (§5.1.68).
pub const MAX_DEGREE: u32 = 255;

/// A validated NURBS curve ready to evaluate.
#[derive(Clone, Debug)]
pub struct Spline {
    degree: usize,
    knots: Vec<f64>,
    homogeneous: Vec<[f64; 4]>,
    /// The homogeneous derivative, a spline of one degree less.
    derivative: Option<Box<Spline>>,
}

impl Spline {
    /// The spline of `nurbs`, its knot vector expanded and checked.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF74 when the degree, knots, weights or control point
    /// count do not form a NURBS curve (§7.7.1 D16, D18, D21).
    pub fn new(nurbs: &Nurbs) -> Result<Spline> {
        let degree = usize::try_from(nurbs.degree).map_err(|_| invalid("its degree is too large"))?;
        if degree == 0 || nurbs.degree > MAX_DEGREE || nurbs.points.len() <= degree {
            return Err(invalid("it needs a degree of at least one and more control points than its degree"));
        }
        let knots = expand(nurbs, degree)?;
        let needed = nurbs.points.len().saturating_add(degree).saturating_add(1);
        if knots.len() != needed {
            return Err(invalid("its knot count is not the control points plus the degree plus one"));
        }
        let mut homogeneous = Vec::with_capacity(nurbs.points.len());
        for point in &nurbs.points {
            let w = point.effective_weight();
            if !w.is_finite() || w <= 0.0_f64 {
                return Err(invalid("its weights must be positive"));
            }
            homogeneous.push([point.point.x, point.point.y, point.point.z, w]);
        }
        let derivative = Some(Box::new(differentiate(degree, &knots, &homogeneous)));
        Ok(Spline { degree, knots, homogeneous, derivative })
    }

    /// The parameter interval [t_p, t_n] on which the curve is defined.
    pub fn domain(&self) -> (f64, f64) {
        let lo = self.knots.get(self.degree).copied().unwrap_or_default();
        let hi = self.knots.get(self.homogeneous.len()).copied().unwrap_or_default();
        (lo, hi)
    }

    /// The point at parameter `u`, clamped to the domain.
    pub fn point(&self, u: f64) -> V3 {
        let [hx, hy, hz, weight] = self.homogeneous_at(u);
        [hx / weight, hy / weight, hz / weight]
    }

    /// The derivative of the point by `u`: (H' - C w') / w for the
    /// homogeneous point H = (w C, w).
    pub fn velocity(&self, u: f64) -> V3 {
        let [hx, hy, hz, weight] = self.homogeneous_at(u);
        let [dx, dy, dz, dw] = self.derivative.as_ref().map(|d| d.homogeneous_at(u)).unwrap_or_default();
        let (px, py, pz) = (hx / weight, hy / weight, hz / weight);
        [(-px).mul_add(dw, dx) / weight, (-py).mul_add(dw, dy) / weight, (-pz).mul_add(dw, dz) / weight]
    }

    /// The distinct knot values strictly inside the domain, where the
    /// derivative may be discontinuous.
    pub fn breaks(&self) -> Vec<f64> {
        let (lo, hi) = self.domain();
        let mut out: Vec<f64> = self.knots.iter().copied().filter(|k| *k > lo && *k < hi).collect();
        out.dedup_by(|a, b| a.total_cmp(b).is_eq());
        out
    }

    fn homogeneous_at(&self, u: f64) -> [f64; 4] {
        let (lo, hi) = self.domain();
        let u = u.clamp(lo, hi);
        let span = self.span(u);
        let start = span.saturating_sub(self.degree);
        let mut d: Vec<[f64; 4]> = self.homogeneous.iter().skip(start).take(self.degree.saturating_add(1)).copied().collect();
        for r in 1..=self.degree {
            for j in (r..=self.degree).rev() {
                let i = start.saturating_add(j);
                let left = self.knot(i);
                let right = self.knot(i.saturating_add(self.degree).saturating_add(1).saturating_sub(r));
                let alpha = if right > left { (u - left) / (right - left) } else { 0.0_f64 };
                let previous = d.get(j.saturating_sub(1)).copied().unwrap_or_default();
                if let Some(slot) = d.get_mut(j) {
                    *slot = blend(previous, *slot, alpha);
                }
            }
        }
        d.get(self.degree).copied().unwrap_or_default()
    }

    fn knot(&self, i: usize) -> f64 {
        self.knots.get(i).copied().unwrap_or_default()
    }

    /// The span k with t_k ≤ u < t_(k+1), p ≤ k < n; the domain's end
    /// belongs to the last span.
    fn span(&self, u: f64) -> usize {
        let last = self.homogeneous.len().saturating_sub(1);
        let mut k = self.degree;
        while k < last && self.knot(k.saturating_add(1)) <= u {
            k = k.saturating_add(1);
        }
        k
    }
}

/// The derivative spline: control points p (H_(i+1) - H_i) / (t_(i+p+1) - t_(i+1)),
/// the knot vector without its ends, one degree less.
fn differentiate(degree: usize, knots: &[f64], homogeneous: &[[f64; 4]]) -> Spline {
    let p = f64::from(u32::try_from(degree).unwrap_or(MAX_DEGREE));
    let mut points = Vec::with_capacity(homogeneous.len().saturating_sub(1));
    for (i, pair) in homogeneous.windows(2).enumerate() {
        let lo = knots.get(i.saturating_add(1)).copied().unwrap_or_default();
        let hi = knots.get(i.saturating_add(degree).saturating_add(1)).copied().unwrap_or_default();
        let factor = if hi > lo { p / (hi - lo) } else { 0.0_f64 };
        let mut q = [0.0_f64; 4];
        if let [a, b] = pair {
            for (out, (x, y)) in q.iter_mut().zip(a.iter().zip(b)) {
                *out = factor * (y - x);
            }
        }
        points.push(q);
    }
    let inner = knots.get(1..knots.len().saturating_sub(1)).unwrap_or_default().to_vec();
    Spline { degree: degree.saturating_sub(1), knots: inner, homogeneous: points, derivative: None }
}

fn blend(a: [f64; 4], b: [f64; 4], alpha: f64) -> [f64; 4] {
    let mut out = a;
    for (o, (x, y)) in out.iter_mut().zip(a.iter().zip(b)) {
        *o = alpha.mul_add(y - x, *x);
    }
    out
}

/// The knot vector, values nondecreasing and finite, interior
/// multiplicities at most the degree and the ends' at most one more
/// (§7.7.1 D19, D21: clamped ends need degree + 1).
fn expand(nurbs: &Nurbs, degree: usize) -> Result<Vec<f64>> {
    let mut knots = Vec::new();
    let last = nurbs.knots.len().saturating_sub(1);
    let mut previous = f64::NEG_INFINITY;
    for (at, knot) in nurbs.knots.iter().enumerate() {
        let count = usize::try_from(knot.multiplicity).map_err(|_| invalid("a knot multiplicity is too large"))?;
        let end = at == 0 || at == last;
        let limit = if end { degree.saturating_add(1) } else { degree };
        if count == 0 || count > limit || !knot.value.is_finite() || knot.value <= previous {
            return Err(invalid("its knots must increase, each repeated between once and the degree"));
        }
        previous = knot.value;
        knots.extend(std::iter::repeat_n(knot.value, count));
    }
    Ok(knots)
}

fn invalid(detail: &str) -> crate::error::SqlError {
    fault(Fault::InvalidGeometry, CLAUSE, &format!("the NURBS curve is not well formed: {detail}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spatial::curves::types::{Coord, Knot, NurbsPoint};

    fn nurbs(degree: u32, points: &[(f64, f64, Option<f64>)], knots: &[(f64, u32)]) -> Nurbs {
        Nurbs {
            degree,
            points: points.iter().map(|&(x, y, weight)| NurbsPoint { point: Coord::xy(x, y), weight }).collect(),
            knots: knots.iter().map(|&(value, multiplicity)| Knot { value, multiplicity }).collect(),
            measures: None,
        }
    }

    /// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.7.1 D16, D19]
    /// A clamped quadratic B-spline passes through its end control points
    /// and its midpoint is the Bézier blend of its controls.
    #[test]
    fn clamped_quadratic_is_a_bezier_curve() {
        let n = nurbs(2, &[(0.0_f64, 0.0_f64, None), (1.0_f64, 2.0_f64, None), (2.0_f64, 0.0_f64, None)], &[(0.0_f64, 3), (1.0_f64, 3)]);
        let spline = Spline::new(&n).unwrap();
        assert_eq!(spline.domain(), (0.0_f64, 1.0_f64));
        assert_eq!(spline.point(0.0), [0.0_f64, 0.0_f64, 0.0_f64]);
        assert_eq!(spline.point(1.0), [2.0_f64, 0.0_f64, 0.0_f64]);
        let mid = spline.point(0.5);
        assert!((mid[0] - 1.0).abs() < 1e-15_f64 && (mid[1] - 1.0).abs() < 1e-15_f64);
    }

    /// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.7.1 D17] A
    /// rational quadratic with weights 1, √2/2, 1 on weighted controls
    /// traces a quarter of the unit circle exactly.
    #[test]
    fn rational_quadratic_traces_a_circle() {
        let w = std::f64::consts::FRAC_1_SQRT_2;
        let n = nurbs(
            2,
            &[(1.0_f64, 0.0_f64, Some(1.0_f64)), (w, w, Some(w)), (0.0_f64, 1.0_f64, Some(1.0_f64))],
            &[(0.0_f64, 3), (1.0_f64, 3)],
        );
        let spline = Spline::new(&n).unwrap();
        for step in 0_i32..=10_i32 {
            let [x, y, _] = spline.point(f64::from(step) / 10.0);
            assert!((x.hypot(y) - 1.0).abs() < 1e-15_f64, "{x} {y}");
        }
    }

    /// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.7.1 D19] An
    /// unclamped uniform cubic is defined on [t_p, t_n] and starts at the
    /// blend (P0 + 4 P1 + P2) / 6 of its first controls.
    #[test]
    fn unclamped_uniform_cubic() {
        let points = [
            (0.0_f64, 0.0_f64, None),
            (1.0_f64, 1.0_f64, None),
            (2.0_f64, 0.0_f64, None),
            (3.0_f64, 1.0_f64, None),
            (4.0_f64, 0.0_f64, None),
        ];
        let knots: Vec<(f64, u32)> = (0_i32..9_i32).map(|k| (f64::from(k), 1)).collect();
        let spline = Spline::new(&nurbs(3, &points, &knots)).unwrap();
        assert_eq!(spline.domain(), (3.0_f64, 5.0_f64));
        let [x, y, _] = spline.point(3.0);
        assert!((x - 1.0).abs() < 1e-15_f64 && (y - 4.0 / 6.0).abs() < 1e-15_f64);
    }

    /// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.7.1 D16, D21]
    /// Malformed curves are refused with SQLSTATE 2FF74: a wrong knot
    /// count, decreasing knots, an interior multiplicity above the
    /// degree, a zero weight, a zero degree, too few control points.
    #[test]
    fn malformed_curves_are_refused() {
        let line = [(0.0_f64, 0.0_f64, None), (1.0_f64, 1.0_f64, None)];
        let cases = [
            nurbs(1, &line, &[(0.0_f64, 2), (1.0_f64, 1)]),
            nurbs(1, &line, &[(1.0_f64, 2), (0.0_f64, 2)]),
            nurbs(
                1,
                &[(0.0_f64, 0.0_f64, None), (1.0_f64, 1.0_f64, None), (2.0_f64, 0.0_f64, None)],
                &[(0.0_f64, 2), (0.5_f64, 2), (1.0_f64, 1)],
            ),
            nurbs(1, &[(0.0_f64, 0.0_f64, Some(0.0_f64)), (1.0_f64, 1.0_f64, None)], &[(0.0_f64, 2), (1.0_f64, 2)]),
            nurbs(0, &line, &[(0.0_f64, 1), (1.0_f64, 1)]),
            nurbs(2, &line, &[(0.0_f64, 3), (1.0_f64, 2)]),
            nurbs(1, &line, &[(0.0_f64, 0), (1.0_f64, 4)]),
        ];
        for case in cases {
            let error = Spline::new(&case).unwrap_err();
            assert_eq!(error.sqlstate().as_str(), "2FF74");
        }
        assert!(Spline::new(&nurbs(1, &line, &[(0.0_f64, 2), (1.0_f64, 2)])).is_ok());
    }
}

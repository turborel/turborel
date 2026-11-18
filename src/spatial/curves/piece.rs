//! \[DESIGN\] One smooth piece of a curve on its natural parameter:
//! a segment, a conic arc o + U cos a + V sin a (circular arcs, circles
//! and ellipses of ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.3, §7.4,
//! §7.6), a NURBS span set (§7.7), a clothoid (§7.8), a spiral (§7.9) or
//! a geodesic (§7.5). Lengths integrate the speed; a conic's speed is
//! constant for a circle, so its length is exact to rounding.

use super::geodesy::Geodesy;
use super::numeric::{HALF, V3, axpy, integrate, lerp, norm, norm2, scale, sub};
use super::nurbs::Spline;
use super::spiral::{Transition, clothoid_heading, clothoid_point};
use super::types::{Coord, Placement};
use crate::error::Result;

/// Which coordinates a length or distance considers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Metric {
    /// x and y only (ST_Length, ST_PointAtDistance).
    Plane,
    /// x, y and z (ST_3DLength, ST_3DPtAtDistance).
    Space,
}

impl Metric {
    /// The length of `v` under this metric.
    pub fn size(self, v: V3) -> f64 {
        match self {
            Metric::Plane => norm2(v),
            Metric::Space => norm(v),
        }
    }
}

/// The geometry of a piece.
#[derive(Clone, Debug)]
pub enum Shape {
    /// From `a` to `b`.
    Line {
        /// Start.
        a: V3,
        /// End.
        b: V3,
    },
    /// o + U cos θ + V sin θ for θ from `a0` to `a1`.
    Conic {
        /// Center.
        o: V3,
        /// The axis at θ = 0.
        u: V3,
        /// The axis at θ = π/2.
        v: V3,
        /// The start angle.
        a0: f64,
        /// The end angle.
        a1: f64,
    },
    /// A NURBS curve over its parameter domain.
    Spline(Box<Spline>),
    /// A clothoid of scale `scale` from distance `s0` to `s1`.
    Clothoid {
        /// The placement of the Fresnel plane.
        placement: Placement,
        /// The scale factor A.
        scale: f64,
        /// The start distance.
        s0: f64,
        /// The end distance.
        s1: f64,
    },
    /// A spiral of the transition's length.
    Spiral {
        /// The placement: origin at the start, heading along the first direction.
        placement: Placement,
        /// The curvature function.
        transition: Transition,
    },
    /// The geodesic from `a` to `b`.
    Geodesic {
        /// Start.
        a: Coord,
        /// End.
        b: Coord,
    },
}

/// A piece and its m at each end; m is linear in 3D length between.
#[derive(Clone, Debug)]
pub struct Piece {
    /// The geometry.
    pub shape: Shape,
    /// The m values at start and end.
    pub m: [f64; 2],
}

impl Piece {
    /// The natural parameter's range, start to end; it may decrease.
    ///
    /// # Errors
    ///
    /// As the geodesic problem's.
    pub fn range<G: Geodesy>(&self, geo: &G) -> Result<(f64, f64)> {
        Ok(match &self.shape {
            Shape::Line { .. } => (0.0, 1.0),
            Shape::Conic { a0, a1, .. } => (*a0, *a1),
            Shape::Spline(spline) => spline.domain(),
            Shape::Clothoid { s0, s1, .. } => (*s0, *s1),
            Shape::Spiral { transition, .. } => (0.0, transition.length),
            Shape::Geodesic { a, b } => (0.0, geo.distance(*a, *b)?),
        })
    }

    /// The point at parameter p.
    ///
    /// # Errors
    ///
    /// As the geodesic problem's.
    pub fn point<G: Geodesy>(&self, p: f64, geo: &G) -> Result<V3> {
        Ok(match &self.shape {
            Shape::Line { a, b } => axpy(*a, p, sub(*b, *a)),
            Shape::Conic { o, u, v, .. } => {
                let (sin, cos) = p.sin_cos();
                axpy(axpy(*o, cos, *u), sin, *v)
            }
            Shape::Spline(spline) => spline.point(p),
            Shape::Clothoid { placement, scale, .. } => {
                let (x, y) = clothoid_point(*scale, p);
                placement.map(x, y)
            }
            Shape::Spiral { placement, transition } => {
                let (x, y) = transition.point(p);
                placement.map(x, y)
            }
            Shape::Geodesic { a, b } => {
                let total = geo.distance(*a, *b)?;
                let at = geo.along(*a, *b, p)?;
                let t = if total > 0.0 { p / total } else { 0.0 };
                [at.x, at.y, lerp(a.z, b.z, t)]
            }
        })
    }

    /// The derivative of the point by the parameter; a geodesic's is
    /// taken by central differences of its direct problem.
    ///
    /// # Errors
    ///
    /// As the geodesic problem's.
    pub fn velocity<G: Geodesy>(&self, p: f64, geo: &G) -> Result<V3> {
        Ok(match &self.shape {
            Shape::Line { a, b } => sub(*b, *a),
            Shape::Conic { u, v, .. } => {
                let (sin, cos) = p.sin_cos();
                axpy(scale(*u, -sin), cos, *v)
            }
            Shape::Spline(spline) => spline.velocity(p),
            Shape::Clothoid { placement, scale: a, .. } => turn(placement, clothoid_heading(*a, p)),
            Shape::Spiral { placement, transition } => turn(placement, transition.heading(p)),
            Shape::Geodesic { .. } => {
                let step = DIFFERENCE_STEP * p.abs().max(1.0);
                let ahead = self.point(p + step, geo)?;
                let behind = self.point(p - step, geo)?;
                scale(sub(ahead, behind), HALF / step)
            }
        })
    }

    /// Parameters strictly inside the range where the speed may jump:
    /// a spline's interior knots.
    fn breaks<G: Geodesy>(&self, geo: &G) -> Result<Vec<f64>> {
        let (lo, hi) = self.range(geo)?;
        Ok(match &self.shape {
            Shape::Spline(spline) => spline.breaks().into_iter().filter(|k| *k > lo.min(hi) && *k < lo.max(hi)).collect(),
            Shape::Line { .. } | Shape::Conic { .. } | Shape::Clothoid { .. } | Shape::Spiral { .. } | Shape::Geodesic { .. } => Vec::new(),
        })
    }

    /// The length from the range's start to parameter p.
    ///
    /// # Errors
    ///
    /// As the geodesic problem's.
    pub fn length_to<G: Geodesy>(&self, p: f64, metric: Metric, geo: &G) -> Result<f64> {
        let (lo, _) = self.range(geo)?;
        match &self.shape {
            Shape::Line { a, b } => Ok(metric.size(sub(*b, *a)) * p),
            Shape::Geodesic { a, b } => geodesic_length(*a, *b, p, metric, geo),
            Shape::Conic { .. } | Shape::Spline(_) | Shape::Clothoid { .. } | Shape::Spiral { .. } => {
                let mut stops = vec![lo];
                stops.extend(self.breaks(geo)?.into_iter().filter(|k| (*k - lo) * (p - *k) > 0.0_f64));
                stops.push(p);
                let mut total = 0.0;
                for pair in stops.windows(2) {
                    if let [from, to] = pair {
                        total += self.speed_integral(*from, *to, metric, geo);
                    }
                }
                Ok(total)
            }
        }
    }

    fn speed_integral<G: Geodesy>(&self, from: f64, to: f64, metric: Metric, geo: &G) -> f64 {
        let (low, high) = if from <= to { (from, to) } else { (to, from) };
        let magnitude = self.velocity(low, geo).map_or(1.0_f64, |v| metric.size(v)) * (high - low);
        integrate(|p| self.velocity(p, geo).map_or(0.0_f64, |v| metric.size(v)), low, high, magnitude)
    }

    /// The piece's length.
    ///
    /// # Errors
    ///
    /// As the geodesic problem's.
    pub fn length<G: Geodesy>(&self, metric: Metric, geo: &G) -> Result<f64> {
        let (_, hi) = self.range(geo)?;
        self.length_to(hi, metric, geo)
    }

    /// The parameter `distance` along the piece from its start.
    ///
    /// # Errors
    ///
    /// As the geodesic problem's.
    pub fn locate<G: Geodesy>(&self, distance: f64, metric: Metric, geo: &G) -> Result<f64> {
        let (lo, hi) = self.range(geo)?;
        let total = self.length(metric, geo)?;
        if total <= 0.0_f64 {
            return Ok(lo);
        }
        let fraction = (distance / total).clamp(0.0, 1.0);
        match &self.shape {
            Shape::Line { .. } => return Ok(fraction),
            Shape::Geodesic { .. } => return Ok(lerp(lo, hi, fraction)),
            Shape::Conic { .. } | Shape::Spline(_) | Shape::Clothoid { .. } | Shape::Spiral { .. } => {}
        }
        let mut failure = None;
        let found = super::numeric::invert(
            |t| match self.length_to(lerp(lo, hi, t), metric, geo) {
                Ok(length) => length,
                Err(e) => {
                    failure = Some(e);
                    total
                }
            },
            distance,
            0.0,
            1.0,
        );
        match failure {
            Some(e) => Err(e),
            None => Ok(lerp(lo, hi, found)),
        }
    }

    /// The m at parameter p, linear in 3D length.
    ///
    /// # Errors
    ///
    /// As the geodesic problem's.
    pub fn measure_at<G: Geodesy>(&self, p: f64, geo: &G) -> Result<f64> {
        let [m0, m1] = self.m;
        if (m1 - m0).abs() <= 0.0_f64 {
            return Ok(m0);
        }
        let total = self.length(Metric::Space, geo)?;
        let part = self.length_to(p, Metric::Space, geo)?;
        Ok(if total > 0.0 { lerp(m0, m1, part / total) } else { m0 })
    }
}

/// The relative step of a geodesic's numerical derivative.
const DIFFERENCE_STEP: f64 = 1e-6;

/// d1 cos θ + d2 sin θ: the placed unit tangent at heading θ.
fn turn(placement: &Placement, heading: f64) -> V3 {
    let (sin, cos) = heading.sin_cos();
    let origin = placement.map(0.0, 0.0);
    sub(placement.map(cos, sin), origin)
}

fn geodesic_length<G: Geodesy>(a: Coord, b: Coord, s: f64, metric: Metric, geo: &G) -> Result<f64> {
    let total = geo.distance(a, b)?;
    let rise = b.z - a.z;
    Ok(match metric {
        Metric::Plane => s,
        Metric::Space => {
            if total > 0.0 {
                s * total.hypot(rise) / total
            } else {
                0.0
            }
        }
    })
}

#[cfg(test)]
mod tests;

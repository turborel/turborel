//! Plain coordinate types for the curve engines: the coordinate
//! dimensions of ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §4.2.1.6, points,
//! vectors (§17.2), affine placements (§17.3), NURBS points (§17.4),
//! knots (§17.5) and the nine instantiable curve types of §7.2 to §7.10.
//! The geometry model adapts onto these; they hold no SRID.

pub use crate::geometry::model::Dims;
pub use crate::geometry::model::SpiralType as SpiralKind;

/// The dimensions with z and m as `flags` say, in that order.
pub fn dims_of(flags: (bool, bool)) -> Dims {
    match flags {
        (false, false) => Dims::Xy,
        (true, false) => Dims::Xyz,
        (false, true) => Dims::Xym,
        (true, true) => Dims::Xyzm,
    }
}

/// `dims` without m, as a placement, a vector or a NURBS control point
/// has.
pub fn without_m(dims: Dims) -> Dims {
    dims_of((dims.has_z(), false))
}

/// A position. Coordinates the value's [`Dims`] lack are zero and are
/// never read; keeping four fields avoids a branch per access.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Coord {
    /// The x coordinate.
    pub x: f64,
    /// The y coordinate.
    pub y: f64,
    /// The z coordinate, zero when absent.
    pub z: f64,
    /// The m coordinate, zero when absent.
    pub m: f64,
}

impl Coord {
    /// The point (x, y).
    pub fn xy(x: f64, y: f64) -> Coord {
        Coord { x, y, z: 0.0, m: 0.0 }
    }

    /// The point (x, y, z).
    pub fn xyz(x: f64, y: f64, z: f64) -> Coord {
        Coord { x, y, z, m: 0.0 }
    }

    /// The point (x, y, z, m).
    pub fn xyzm(x: f64, y: f64, z: f64, m: f64) -> Coord {
        Coord { x, y, z, m }
    }

    /// The x, y and z coordinates as an array, z zero when absent.
    pub fn space(self) -> [f64; 3] {
        [self.x, self.y, self.z]
    }

    /// The point at `space` with measure `m`.
    pub fn at(space: [f64; 3], m: f64) -> Coord {
        let [x, y, z] = space;
        Coord { x, y, z, m }
    }

    /// Whether every coordinate `dims` names is finite.
    pub fn is_finite(self, dims: Dims) -> bool {
        let z = !dims.has_z() || self.z.is_finite();
        let m = !dims.has_m() || self.m.is_finite();
        self.x.is_finite() && self.y.is_finite() && z && m
    }
}

/// A direction and length (§17.2): x and y, and z when 3D.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vector {
    /// The x component.
    pub x: f64,
    /// The y component.
    pub y: f64,
    /// The z component, zero when 2D.
    pub z: f64,
}

impl Vector {
    /// The components as an array.
    pub fn space(self) -> [f64; 3] {
        [self.x, self.y, self.z]
    }

    /// The vector of `space`.
    pub fn of(space: [f64; 3]) -> Vector {
        let [x, y, z] = space;
        Vector { x, y, z }
    }
}

/// An affine mapping of a parameter space into coordinates (§17.3): the
/// image of the origin and of each basis vector. The curves of §7.6,
/// §7.8 and §7.9 use two reference directions.
#[derive(Clone, Debug, PartialEq)]
pub struct Placement {
    /// The target of the parameter space origin; it has no m (§17.3.1 D6).
    pub location: Coord,
    /// The targets of the parameter space's basis vectors.
    pub directions: Vec<Vector>,
}

impl Placement {
    /// The coordinates of parameter point (u, v): location + u d1 + v d2
    /// (§17.3.7). Missing directions contribute nothing.
    pub fn map(&self, u: f64, v: f64) -> [f64; 3] {
        let [lx, ly, lz] = self.location.space();
        let mut out = [lx, ly, lz];
        for (weight, direction) in [u, v].into_iter().zip(&self.directions) {
            for (o, d) in out.iter_mut().zip(direction.space()) {
                *o = weight.mul_add(d, *o);
            }
        }
        out
    }
}

/// The m values at a parametric curve's start and end, when measured.
pub type Measures = Option<[f64; 2]>;

/// An elliptical arc (§7.6): (u, v) = (U cos a, V sin a) in the
/// placement's parameter space, a from the start to the end angle.
#[derive(Clone, Debug, PartialEq)]
pub struct Elliptical {
    /// The reference location; its origin is the ellipse's center.
    pub placement: Placement,
    /// The axis length along u.
    pub u_axis: f64,
    /// The axis length along v.
    pub v_axis: f64,
    /// The start angle, in radians.
    pub start: f64,
    /// The end angle, in radians.
    pub end: f64,
    /// The start and end measures.
    pub measures: Measures,
}

/// A control point of a NURBS curve (§17.4): the weighted point and its
/// weight. The point's coordinates are already multiplied by the weight
/// (§7.7.1 D17); a null weight is one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NurbsPoint {
    /// The weighted point (w x, w y, w z).
    pub point: Coord,
    /// The weight, if one was given.
    pub weight: Option<f64>,
}

impl NurbsPoint {
    /// The weight in effect: one when none was given.
    pub fn effective_weight(&self) -> f64 {
        self.weight.unwrap_or(1.0)
    }
}

/// A knot value and its multiplicity (§17.5).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Knot {
    /// The knot value.
    pub value: f64,
    /// How many times it repeats.
    pub multiplicity: u32,
}

/// A non-uniform rational B-spline (§7.7).
#[derive(Clone, Debug, PartialEq)]
pub struct Nurbs {
    /// The polynomial degree.
    pub degree: u32,
    /// The control points.
    pub points: Vec<NurbsPoint>,
    /// The knots with multiplicities.
    pub knots: Vec<Knot>,
    /// The start and end measures.
    pub measures: Measures,
}

/// A clothoid (§7.8): the Cornu spiral of scale factor A, from the
/// start to the end distance along it measured from its inflection point.
#[derive(Clone, Debug, PartialEq)]
pub struct Clothoid {
    /// Places the Fresnel integrals' plane.
    pub placement: Placement,
    /// The scale factor A; curvature at distance s is s / A².
    pub scale: f64,
    /// The start distance from the inflection point.
    pub start: f64,
    /// The end distance from the inflection point.
    pub end: f64,
    /// The start and end measures.
    pub measures: Measures,
}

/// A spiral (§7.9): from the placement's origin along its first
/// direction, its curvature moving from the start to the end curvature.
#[derive(Clone, Debug, PartialEq)]
pub struct Spiral {
    /// Places the spiral: origin at its start, initial direction along x.
    pub placement: Placement,
    /// The length of the curve.
    pub length: f64,
    /// The curvature at the start.
    pub start_curvature: f64,
    /// The curvature at the end.
    pub end_curvature: f64,
    /// The curvature function.
    pub kind: SpiralKind,
    /// The start and end measures.
    pub measures: Measures,
}

/// A curve of any instantiable type of §7. Point lists may be empty, the
/// empty set of their type; a compound curve holds its member curves.
#[derive(Clone, Debug, PartialEq)]
pub enum Curve {
    /// Linear interpolation (§7.2).
    LineString(Vec<Coord>),
    /// Circular arcs through three points each, sharing ends (§7.3).
    CircularString(Vec<Coord>),
    /// The full circle through three points (§7.4).
    Circle(Vec<Coord>),
    /// Geodesic interpolation (§7.5).
    GeodesicString(Vec<Coord>),
    /// An elliptical arc (§7.6), absent when empty.
    Elliptical(Option<Elliptical>),
    /// A NURBS curve (§7.7), absent when empty.
    Nurbs(Option<Nurbs>),
    /// A clothoid (§7.8), absent when empty.
    Clothoid(Option<Clothoid>),
    /// A spiral (§7.9), absent when empty.
    Spiral(Option<Spiral>),
    /// Curves joined end to start (§7.10).
    Compound(Vec<Curve>),
}

impl Curve {
    /// Whether the curve is the empty set.
    pub fn is_empty(&self) -> bool {
        match self {
            Curve::LineString(p) | Curve::CircularString(p) | Curve::Circle(p) | Curve::GeodesicString(p) => p.is_empty(),
            Curve::Elliptical(e) => e.is_none(),
            Curve::Nurbs(n) => n.is_none(),
            Curve::Clothoid(c) => c.is_none(),
            Curve::Spiral(s) => s.is_none(),
            Curve::Compound(c) => c.iter().all(Curve::is_empty),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §4.2.1.6] The four
    /// coordinate dimensions follow the z and m flags, and a placement's
    /// dimensions drop m.
    #[test]
    fn dimensions_follow_the_z_and_m_flags() {
        assert_eq!(dims_of((false, false)), Dims::Xy);
        assert_eq!(dims_of((true, false)), Dims::Xyz);
        assert_eq!(dims_of((false, true)), Dims::Xym);
        assert_eq!(dims_of((true, true)), Dims::Xyzm);
        assert_eq!(without_m(Dims::Xyzm), Dims::Xyz);
        assert_eq!(without_m(Dims::Xym), Dims::Xy);
    }

    /// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §4.2.1.6] A point is
    /// finite when every coordinate its dimensions name is; a coordinate
    /// the dimensions lack is never read. MC/DC: z alone and m alone each
    /// decide the outcome with the other held finite.
    #[test]
    fn a_point_is_finite_in_the_coordinates_its_dimensions_name() {
        let bad_z = Coord::xyzm(1.0_f64, 2.0_f64, f64::NAN, 3.0_f64);
        let bad_m = Coord::xyzm(1.0_f64, 2.0_f64, 3.0_f64, f64::INFINITY);
        assert!(Coord::xyzm(1.0_f64, 2.0_f64, 3.0_f64, 4.0_f64).is_finite(Dims::Xyzm));
        assert!(!bad_z.is_finite(Dims::Xyz));
        assert!(bad_z.is_finite(Dims::Xym));
        assert!(!bad_m.is_finite(Dims::Xym));
        assert!(bad_m.is_finite(Dims::Xyz));
        assert!(!Coord::xy(f64::NAN, 0.0_f64).is_finite(Dims::Xy));
        assert!(!Coord::xy(0.0_f64, f64::NAN).is_finite(Dims::Xy));
    }
}

//! Geodesic interpolation for ST_GeodesicString, ISO/IEC 13249-3:2016
//! (DIS, SC32 N2593) §4.2.8: control points are joined by geodesics of
//! the coordinate reference system's geoid or ellipsoid. The engines take
//! the geodesic problem as a parameter, so an ellipsoidal solver plugs in
//! without changing them. Two are built in: straight lines, the geodesics
//! of a Cartesian system, and great circles of a sphere.

use super::fault::{Fault, fault};
use super::types::Coord;
use crate::error::Result;

/// The direct and inverse geodesic problems between two control points.
pub trait Geodesy {
    /// The length of the geodesic from `a` to `b`, x and y only.
    ///
    /// # Errors
    ///
    /// When the points are outside the system's domain.
    fn distance(&self, a: Coord, b: Coord) -> Result<f64>;

    /// The point `s` along the geodesic from `a` toward `b`, its x and y;
    /// `s` lies between zero and their distance.
    ///
    /// # Errors
    ///
    /// When the points are outside the system's domain.
    fn along(&self, a: Coord, b: Coord, s: f64) -> Result<Coord>;
}

/// Straight segments: geodesics of a Cartesian coordinate system.
#[derive(Clone, Copy, Debug, Default)]
pub struct Planar;

impl Geodesy for Planar {
    fn distance(&self, a: Coord, b: Coord) -> Result<f64> {
        Ok((b.x - a.x).hypot(b.y - a.y))
    }

    fn along(&self, a: Coord, b: Coord, s: f64) -> Result<Coord> {
        let length = (b.x - a.x).hypot(b.y - a.y);
        let t = if length > 0.0_f64 { s / length } else { 0.0_f64 };
        Ok(Coord::xy(t.mul_add(b.x - a.x, a.x), t.mul_add(b.y - a.y, a.y)))
    }
}

/// Great circles of a sphere; x is longitude and y latitude, in degrees.
#[derive(Clone, Copy, Debug)]
pub struct Sphere {
    /// The sphere's radius, in the unit lengths are reported in.
    pub radius: f64,
}

/// Latitudes beyond ±90 degrees are outside a sphere's domain.
const RIGHT_ANGLE: f64 = 90.0;

impl Sphere {
    fn unit(p: Coord) -> Result<[f64; 3]> {
        if !p.y.is_finite() || !p.x.is_finite() || p.y.abs() > RIGHT_ANGLE {
            return Err(fault(Fault::InvalidArgument, "§4.2.8", "a geodesic control point is not a longitude and latitude"));
        }
        let (lat, lon) = (p.y.to_radians(), p.x.to_radians());
        Ok([lat.cos() * lon.cos(), lat.cos() * lon.sin(), lat.sin()])
    }

    /// The central angle between the points, by the atan2 form that stays
    /// accurate for near and antipodal points.
    fn angle(a: [f64; 3], b: [f64; 3]) -> f64 {
        let c = super::numeric::cross(a, b);
        super::numeric::norm(c).atan2(super::numeric::dot(a, b))
    }
}

impl Geodesy for Sphere {
    fn distance(&self, a: Coord, b: Coord) -> Result<f64> {
        Ok(self.radius * Sphere::angle(Sphere::unit(a)?, Sphere::unit(b)?))
    }

    fn along(&self, a: Coord, b: Coord, s: f64) -> Result<Coord> {
        let (from, to) = (Sphere::unit(a)?, Sphere::unit(b)?);
        let total = Sphere::angle(from, to);
        if total <= 0.0_f64 || self.radius <= 0.0_f64 {
            return Ok(Coord::xy(a.x, a.y));
        }
        let fraction = (s / self.radius) / total;
        // Spherical linear interpolation along the great circle.
        let weights = (((1.0 - fraction) * total).sin() / total.sin(), (fraction * total).sin() / total.sin());
        let [px, py, pz] = super::numeric::add(super::numeric::scale(from, weights.0), super::numeric::scale(to, weights.1));
        Ok(Coord::xy(py.atan2(px).to_degrees(), pz.atan2(px.hypot(py)).to_degrees()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §4.2.8] In a
    /// Cartesian system the geodesic is the straight segment.
    #[test]
    fn planar_geodesics_are_segments() {
        let (a, b) = (Coord::xy(0.0, 0.0), Coord::xy(3.0, 4.0));
        assert_eq!(Planar.distance(a, b).unwrap(), 5.0_f64);
        let mid = Planar.along(a, b, 2.5).unwrap();
        assert_eq!((mid.x, mid.y), (1.5_f64, 2.0_f64));
        assert_eq!(Planar.along(a, a, 0.0).unwrap(), a);
    }

    /// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §4.2.8] On a
    /// sphere a quarter of the equator has length πR/2 and its midpoint
    /// is at 45 degrees; a meridian's midpoint keeps its longitude; a
    /// latitude beyond 90 degrees is refused.
    #[test]
    fn sphere_geodesics_are_great_circles() {
        let sphere = Sphere { radius: 2.0 };
        let (a, b) = (Coord::xy(0.0, 0.0), Coord::xy(90.0, 0.0));
        assert!((sphere.distance(a, b).unwrap() - std::f64::consts::PI).abs() < 1e-15_f64);
        let mid = sphere.along(a, b, std::f64::consts::FRAC_PI_2).unwrap();
        assert!((mid.x - 45.0).abs() < 1e-12_f64 && mid.y.abs() < 1e-12_f64);
        let north = sphere
            .along(Coord::xy(10.0, 0.0), Coord::xy(10.0, 60.0), sphere.distance(Coord::xy(10.0, 0.0), Coord::xy(10.0, 30.0)).unwrap())
            .unwrap();
        assert!((north.x - 10.0).abs() < 1e-12_f64 && (north.y - 30.0).abs() < 1e-12_f64);
        assert!(sphere.distance(Coord::xy(0.0, 91.0), b).is_err());
        assert_eq!(sphere.along(a, a, 0.0).unwrap(), a);
    }
}

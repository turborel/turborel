//! Geodesics on an ellipsoid of revolution, after Karney, "Algorithms for
//! geodesics" (J. Geodesy 87, 2013): the inverse problem (distance and
//! azimuths between two points, `solve`), the direct problem (`line`),
//! the area and perimeter of a geodesic polygon (`area`), and the
//! distance from a point to a geodesic segment (`nearest`). Errors are a few nanometres for the
//! Earth, and the inverse problem converges for every pair of points,
//! antipodes included.
//!
//! They give the geodetic ST_Distance, ST_Length, ST_Perimeter and
//! ST_Area of ISO/IEC 13249-3:2016 (DIS, SC32 N2593): §5.1.41 Description
//! 2a iv lets the distance between points be an implementation-defined
//! algorithm, and on a \<geographic cs\> this one is the length of the
//! shortest geodesic, in metres. Points are longitude, latitude pairs in
//! degrees, the default axis order of a geographic system.

mod area;
mod line;
mod math;
mod nearest;
mod series;
mod solve;

pub use area::Polygon;
pub use line::{Line, Position};

use super::ellipsoid::Ellipsoid;
use super::{HALF, TWO};
use series::{C3_SIZE, C4_SIZE, ORDER};

/// The result of the inverse problem.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Inverse {
    /// Distance s12, metres.
    pub distance: f64,
    /// Azimuth at the first point, degrees clockwise from north.
    pub azimuth1: f64,
    /// Azimuth at the second point, degrees.
    pub azimuth2: f64,
    /// Arc length on the auxiliary sphere, degrees.
    pub arc: f64,
    /// Reduced length m12, metres.
    pub reduced: f64,
    /// Geodesic scale M12.
    pub scale12: f64,
    /// Geodesic scale M21.
    pub scale21: f64,
    /// Area S12 between the geodesic and the equator, square metres.
    pub area: f64,
}

/// An ellipsoid prepared for geodesic computations.
#[derive(Clone, Debug, PartialEq)]
pub struct Geodesic {
    a: f64,
    f: f64,
    /// 1 − f.
    f1: f64,
    e2: f64,
    ep2: f64,
    n: f64,
    b: f64,
    /// The authalic radius squared.
    c2: f64,
    /// A tolerance of the inverse problem's first guess.
    etol2: f64,
    a3x: [f64; ORDER],
    c3x: [f64; C3_SIZE],
    c4x: [f64; C4_SIZE],
}

/// Tuning of Karney 2013 §5: a tenth of √ε relative to the flattening.
const ETOL_SCALE: f64 = 0.1;
/// The flattening below which the tolerance stops shrinking.
const FLAT_FLOOR: f64 = 0.001;

impl Geodesic {
    /// Geodesics on `ellipsoid`. A sphere is the limit f = 0.
    pub fn new(ellipsoid: &Ellipsoid) -> Geodesic {
        let (a, f) = (ellipsoid.a, ellipsoid.f);
        let f1 = 1.0_f64 - f;
        let e2 = ellipsoid.e2;
        let n = ellipsoid.n;
        let b = a * f1;
        let authalic = if e2 > 0.0_f64 { e2.sqrt().atanh() / e2.sqrt() } else { 1.0_f64 };
        let tol2 = f64::EPSILON.sqrt();
        let etol2 = ETOL_SCALE * tol2 / (f.abs().max(FLAT_FLOOR) * (1.0_f64 - f * HALF).min(1.0_f64) * HALF).sqrt();
        Geodesic {
            a,
            f,
            f1,
            e2,
            ep2: e2 / (f1 * f1),
            n,
            b,
            c2: (a * a + b * b * authalic) * HALF,
            etol2,
            a3x: series::a3_coefficients(n),
            c3x: series::c3_coefficients(n),
            c4x: series::c4_coefficients(n),
        }
    }

    /// Geodesics on WGS 84.
    pub fn wgs84() -> Geodesic {
        Geodesic::new(&Ellipsoid::wgs84())
    }

    /// The area of the whole ellipsoid, square metres.
    pub fn ellipsoid_area(&self) -> f64 {
        std::f64::consts::TAU * TWO * self.c2
    }

    /// The shortest geodesic from (`lon1`, `lat1`) to (`lon2`, `lat2`),
    /// degrees. Latitudes beyond ±90° give NaN results.
    pub fn inverse(&self, lon1: f64, lat1: f64, lon2: f64, lat2: f64) -> Inverse {
        solve::inverse(self, [lat1, lon1], [lat2, lon2])
    }

    /// The point `distance` metres from (`lon1`, `lat1`) along azimuth
    /// `azimuth` degrees: its longitude, latitude and azimuth there.
    pub fn direct(&self, lon1: f64, lat1: f64, azimuth: f64, distance: f64) -> [f64; 3] {
        let position = Line::new(self, lat1, lon1, azimuth).position(distance);
        [position.lon, position.lat, position.azimuth]
    }

    /// The length, metres, of the path through `points`, longitude and
    /// latitude pairs in degrees: the geodetic ST_Length of a linestring.
    pub fn length(&self, points: &[[f64; 2]]) -> f64 {
        points.iter().zip(points.iter().skip(1)).map(|([lon1, lat1], [lon2, lat2])| self.inverse(*lon1, *lat1, *lon2, *lat2).distance).sum()
    }

    /// The perimeter, metres, and signed area, square metres, of the
    /// geodesic polygon through `ring`, which need not repeat its first
    /// point. Counter-clockwise rings have positive area. ST_Area of a
    /// polygon is its exterior's magnitude less its holes'.
    pub fn polygon(&self, ring: &[[f64; 2]]) -> (f64, f64) {
        let mut polygon = Polygon::new(self);
        for [lon, lat] in ring {
            polygon.add(*lon, *lat);
        }
        polygon.compute()
    }

    /// The least distance, metres, from the point `p` to the geodesic
    /// segment from `a` to `b`, all longitude and latitude in degrees.
    pub fn point_segment(&self, p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
        nearest::point_segment(self, p, [a, b])
    }

    /// The least distance, metres, between the geodesic segments `first`
    /// and `second`: zero if they cross, else the least distance from an
    /// endpoint of one to the other.
    pub fn segment_segment(&self, first: [[f64; 2]; 2], second: [[f64; 2]; 2]) -> f64 {
        nearest::segment_segment(self, first, second)
    }
}

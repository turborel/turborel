//! The perimeter and area of a geodesic polygon, Karney 2013 §6: the sum
//! of the areas S12 between each edge and the equator, corrected by a
//! multiple of the ellipsoid's area when the polygon encircles a pole.

use super::super::HALF;
use super::Geodesic;
use super::math::{self, Accumulator};

/// A polygon built up a vertex at a time.
#[derive(Clone, Debug)]
pub struct Polygon<'g> {
    geodesic: &'g Geodesic,
    first: Option<[f64; 2]>,
    last: [f64; 2],
    perimeter: Accumulator,
    area: Accumulator,
    /// The parity of the edges that cross the prime meridian.
    odd: bool,
}

/// Whether an edge from `lon1` to `lon2` crosses the meridian 0°, which
/// counts the times the polygon encircles a pole.
fn transit(lon1: f64, lon2: f64) -> bool {
    let (span, _) = math::difference(lon1, lon2);
    let (lon1, lon2) = (math::normalize(lon1), math::normalize(lon2));
    let eastward = span > 0.0_f64 && (lon1 < 0.0_f64 && lon2 >= 0.0_f64 || lon1 > 0.0_f64 && lon2 == 0.0_f64);
    let westward = span < 0.0_f64 && lon1 >= 0.0_f64 && lon2 < 0.0_f64;
    eastward || westward
}

impl<'g> Polygon<'g> {
    /// An empty polygon on `geodesic`.
    pub fn new(geodesic: &'g Geodesic) -> Polygon<'g> {
        Polygon { geodesic, first: None, last: [0.0_f64; 2], perimeter: Accumulator::default(), area: Accumulator::default(), odd: false }
    }

    /// Adds the vertex (`lon`, `lat`), degrees.
    pub fn add(&mut self, lon: f64, lat: f64) {
        if self.first.is_some() {
            let [lon0, lat0] = self.last;
            let edge = self.geodesic.inverse(lon0, lat0, lon, lat);
            self.perimeter.add(edge.distance);
            self.area.add(edge.area);
            self.odd ^= transit(lon0, lon);
        } else {
            self.first = Some([lon, lat]);
        }
        self.last = [lon, lat];
    }

    /// The perimeter, metres, and signed area, square metres, closing the
    /// polygon back to its first vertex. Counter-clockwise is positive.
    pub fn compute(&self) -> (f64, f64) {
        let Some([lon0, lat0]) = self.first else {
            return (0.0_f64, 0.0_f64);
        };
        let [lon, lat] = self.last;
        let edge = self.geodesic.inverse(lon, lat, lon0, lat0);
        let mut perimeter = self.perimeter;
        perimeter.add(edge.distance);
        let mut area = self.area;
        area.add(edge.area);
        let total = self.geodesic.ellipsoid_area();
        area.remainder(total);
        if self.odd ^ transit(lon, lon0) {
            area.add(if area.value() < 0.0_f64 { total * HALF } else { -total * HALF });
        }
        area.negate();
        if area.value() > total * HALF {
            area.add(-total);
        } else if area.value() <= -total * HALF {
            area.add(total);
        }
        (perimeter.value(), area.value() + 0.0_f64)
    }
}

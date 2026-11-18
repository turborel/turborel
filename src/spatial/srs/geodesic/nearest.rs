//! The least distance from a point to a geodesic segment. Along the
//! segment the distance to the point falls and then rises, and its rate
//! of change is cos(α − β), α the segment's azimuth and β the azimuth at
//! which the geodesic from the point arrives (Karney 2013 eq. 3 read for
//! a moving endpoint). The least distance is at an endpoint when that
//! rate does not change sign, else where it vanishes, found by the
//! Illinois variant of regula falsi.

use super::super::HALF;
use super::Geodesic;
use super::line::Line;

/// Iterations of the root search and its tolerance in metres.
const ITERATIONS: u32 = 60;
const TOLERANCE: f64 = 1e-6;

/// The rate of change of the distance from `p` at `s` metres along `line`,
/// and that distance.
fn slope(g: &Geodesic, line: &Line, p: [f64; 2], s: f64) -> (f64, f64) {
    let x = line.position(s);
    let [lon, lat] = p;
    let to = g.inverse(lon, lat, x.lon, x.lat);
    ((x.azimuth - to.azimuth2).to_radians().cos(), to.distance)
}

/// The least distance from `p` to the segment from `ends[0]` to `ends[1]`.
pub(super) fn point_segment(g: &Geodesic, p: [f64; 2], ends: [[f64; 2]; 2]) -> f64 {
    let [[lon1, lat1], [lon2, lat2]] = ends;
    let edge = g.inverse(lon1, lat1, lon2, lat2);
    let line = Line::new(g, lat1, lon1, edge.azimuth1);
    let (mut lo, mut hi) = (0.0_f64, edge.distance);
    let (mut g_lo, d_lo) = slope(g, &line, p, lo);
    let (mut g_hi, d_hi) = slope(g, &line, p, hi);
    if g_lo >= 0.0_f64 || g_hi <= 0.0_f64 || edge.distance == 0.0_f64 {
        return d_lo.min(d_hi);
    }
    let mut best = d_lo.min(d_hi);
    let mut side = 0_i8;
    for _ in 0..ITERATIONS {
        let s = (lo * g_hi - hi * g_lo) / (g_hi - g_lo);
        let (rate, distance) = slope(g, &line, p, s);
        best = best.min(distance);
        if hi - lo < TOLERANCE || rate == 0.0_f64 {
            break;
        }
        if rate < 0.0_f64 {
            (lo, g_lo) = (s, rate);
            g_hi *= if side < 0 { HALF } else { 1.0_f64 };
            side = -1;
        } else {
            (hi, g_hi) = (s, rate);
            g_lo *= if side > 0 { HALF } else { 1.0_f64 };
            side = 1;
        }
    }
    best
}

/// The side of `x` from the geodesic `c` to `d`: positive to the right.
fn side(g: &Geodesic, c: [f64; 2], d: [f64; 2], x: [f64; 2]) -> f64 {
    let along = g.inverse(c[0], c[1], d[0], d[1]).azimuth1;
    let toward = g.inverse(c[0], c[1], x[0], x[1]).azimuth1;
    (toward - along).to_radians().sin()
}

/// Whether segments `first` and `second` cross or touch.
fn cross(g: &Geodesic, first: [[f64; 2]; 2], second: [[f64; 2]; 2]) -> bool {
    let ([start, end], [other_start, other_end]) = (first, second);
    let straddle_second = side(g, other_start, other_end, start) * side(g, other_start, other_end, end) <= 0.0_f64;
    let straddle_first = side(g, start, end, other_start) * side(g, start, end, other_end) <= 0.0_f64;
    straddle_second && straddle_first
}

/// The least distance between segments `first` and `second`.
pub(super) fn segment_segment(g: &Geodesic, first: [[f64; 2]; 2], second: [[f64; 2]; 2]) -> f64 {
    if cross(g, first, second) {
        return 0.0_f64;
    }
    let ([start, end], [other_start, other_end]) = (first, second);
    let ends = point_segment(g, start, second).min(point_segment(g, end, second));
    ends.min(point_segment(g, other_start, first)).min(point_segment(g, other_end, first))
}

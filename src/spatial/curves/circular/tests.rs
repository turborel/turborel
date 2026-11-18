use super::*;
use std::f64::consts::{FRAC_PI_2, PI};

fn xy(x: f64, y: f64) -> Coord {
    Coord::xy(x, y)
}

fn near(a: Coord, b: Coord) -> bool {
    norm(sub(a.space(), b.space())) < 1e-12_f64
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.3.1 D13, §7.3.2
/// D16] Bulges and normals give each arc's middle point; the bulge and
/// normal read back from the arc rebuild the same arc, in 2D and 3D.
#[test]
fn bulges_round_trip() {
    let points = from_bulges(
        &[xy(-1.0, 0.0), xy(1.0, 0.0), xy(3.0, 0.0)],
        &[1.0_f64, -1.0_f64],
        &[[0.0_f64, 1.0_f64, 0.0_f64], [0.0_f64, 1.0_f64, 0.0_f64]],
    )
    .unwrap();
    assert_eq!(points, vec![xy(-1.0, 0.0), xy(0.0, 1.0), xy(1.0, 0.0), xy(2.0, -1.0), xy(3.0, 0.0)]);
    let one: [Coord; 3] = points[..3].try_into().unwrap();
    let (b, n) = bulge(one, false);
    assert!((b - 1.0).abs() < 1e-12_f64 && (n[1] - 1.0).abs() < 1e-12_f64);
    let (second, _) = bulge(points[2..].try_into().unwrap(), false);
    assert!((second + 1.0).abs() < 1e-12_f64);
    let lifted = [Coord::xyz(-1.0, 0.0, 0.0), Coord::xyz(0.0, 0.0, 1.0), Coord::xyz(1.0, 0.0, 0.0)];
    let (height, toward) = bulge(lifted, true);
    let rebuilt = from_bulges(&[lifted[0], lifted[2]], &[height], &[toward]).unwrap();
    assert!(near(rebuilt[1], lifted[1]));
    let (flat, _) = bulge([xy(0.0, 0.0), xy(1.0, 0.0), xy(2.0, 0.0)], true);
    assert_eq!(flat, 0.0_f64);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.3.1 D13b, D6]
/// Bulge arrays of the wrong length, normals along the chord and equal
/// control points are refused.
#[test]
fn bad_bulges_are_refused() {
    let two = [xy(0.0, 0.0), xy(2.0, 0.0)];
    let state = |r: Result<Vec<Coord>>| r.unwrap_err().sqlstate().as_str().to_owned();
    assert_eq!(state(from_bulges(&two, &[], &[])), "2FF02");
    assert_eq!(state(from_bulges(&[], &[], &[])), "2FF02");
    assert_eq!(state(from_bulges(&two, &[1.0_f64], &[[1.0_f64, 0.0_f64, 0.0_f64]])), "2FF02");
    assert_eq!(state(from_bulges(&[xy(1.0, 1.0), xy(1.0, 1.0)], &[1.0_f64], &[[0.0_f64, 1.0_f64, 0.0_f64]])), "2FF74");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.3.1 D14, D15]
/// The four sweeps of the standard's example from 0 and 90 degrees, and
/// their angles read back with the sweep's direction.
#[test]
fn center_radius_angles() {
    let origin = xy(1.0, 1.0);
    let cases = [
        (0.0_f64, FRAC_PI_2, 1.0_f64),
        (FRAC_PI_2, 0.0_f64, -1.0_f64),
        (FRAC_PI_2, 2.0_f64 * PI, 1.0_f64),
        (2.0_f64 * PI, FRAC_PI_2, -1.0_f64),
    ];
    for (start, end, turn) in cases {
        let points = from_center(origin, 2.0, start, end).unwrap();
        let arc: [Coord; 3] = points.try_into().unwrap();
        let (first, last) = angles(arc).unwrap();
        assert!((last - first - (end - start)).abs() < 1e-12_f64, "{start} {end}: {first} {last}");
        assert!(normal(arc).unwrap()[2] * turn > 0.0_f64);
        let (middle, radius) = center(arc).unwrap();
        assert!(near(middle, origin) && (radius - 2.0).abs() < 1e-12_f64);
    }
    for (radius, end) in [(0.0_f64, 1.0_f64), (1.0_f64, 0.0_f64), (1.0_f64, 2.0_f64 * PI), (f64::NAN, 1.0_f64)] {
        assert_eq!(from_center(origin, radius, 0.0, end).unwrap_err().sqlstate().as_str(), "2FF02");
    }
    assert!(angles([xy(0.0, 0.0), xy(1.0, 0.0), xy(2.0, 0.0)]).is_none());
    assert!(center([xy(0.0, 0.0), xy(1.0, 0.0), xy(2.0, 0.0)]).is_none());
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.4.2 D16, §7.4.7]
/// A circle from center, radius and normal has its control points on the
/// circle and turns about the normal, in space and in the plane.
#[test]
fn circle_from_center_radius_normal() {
    let o = Coord::xyz(1.0, 2.0, 3.0);
    let points = circle_from_center(o, 2.0, [1.0_f64, 1.0_f64, 1.0_f64], true).unwrap();
    let three: [Coord; 3] = points.try_into().unwrap();
    for p in three {
        assert!((norm(sub(p.space(), o.space())) - 2.0).abs() < 1e-12_f64);
    }
    let n = normal(three).unwrap();
    assert!((dot(n, [1.0_f64, 1.0_f64, 1.0_f64]) - 3.0_f64.sqrt()).abs() < 1e-12_f64);
    let flat: [Coord; 3] = circle_from_center(xy(0.0, 0.0), 1.0, [0.0_f64, 0.0_f64, -1.0_f64], false).unwrap().try_into().unwrap();
    assert!((normal(flat).unwrap()[2] + 1.0).abs() < 1e-12_f64);
    assert_eq!(flat[0].z, 0.0_f64);
    assert!(circle_from_center(o, 1.0, [0.0_f64; 3], true).is_err());
    assert!(circle_from_center(o, -1.0, [0.0_f64, 0.0_f64, 1.0_f64], true).is_err());
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.3.8, §7.3.9]
/// ST_MidPointRep's middle points lie halfway along the arc; observers
/// of a single arc refuse strings of several arcs with 2FF76.
#[test]
fn midpoints_and_single_arcs() {
    let mid = midpoint([xy(1.0, 0.0), Coord { m: 4.0, ..xy(0.0, -1.0) }, Coord { m: 2.0, ..xy(-1.0, 0.0) }]);
    assert!(near(mid, xy(0.0, -1.0)) && (mid.m - 1.0).abs() < 1e-12_f64);
    let line = midpoint([xy(0.0, 0.0), xy(3.0, 0.0), xy(4.0, 0.0)]);
    assert!(near(line, xy(2.0, 0.0)));
    let five = [xy(0.0, 0.0), xy(1.0, 1.0), xy(2.0, 0.0), xy(3.0, -1.0), xy(4.0, 0.0)];
    assert_eq!(single(&five).unwrap_err().sqlstate().as_str(), "2FF76");
    assert_eq!(single(&five[..2]).unwrap_err().sqlstate().as_str(), "2FF74");
    assert!(single(&[]).unwrap().is_none());
    assert!(single(&five[..3]).unwrap().is_some());
}

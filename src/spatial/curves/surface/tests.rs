use super::*;
use crate::spatial::curves::geodesy::Planar;
use crate::spatial::curves::types::{Elliptical, Knot, Nurbs, NurbsPoint, Placement, Vector};
use std::f64::consts::{FRAC_1_SQRT_2, PI};

fn xy(x: f64, y: f64) -> Coord {
    Coord::xy(x, y)
}

fn circle(cx: f64, cy: f64, r: f64) -> Curve {
    Curve::Circle(vec![xy(cx + r, cy), xy(cx, cy + r), xy(cx - r, cy)])
}

fn square(side: f64) -> Curve {
    Curve::LineString(vec![xy(0.0, 0.0), xy(side, 0.0), xy(side, side), xy(0.0, side), xy(0.0, 0.0)])
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.1.2, §8.1.4]
/// A disk's area is exactly πr² and its perimeter 2πr, far from the
/// origin too; a square with a round hole subtracts the hole either way
/// round.
#[test]
fn disks_and_holes() {
    let disk = [circle(1e6, -2e6, 3.0)];
    let a = area(&disk, Metric::Plane, &Planar).unwrap().unwrap();
    assert!((a - 9.0 * PI).abs() < 1e-8_f64, "{a}");
    assert!((perimeter(&disk, Metric::Plane, &Planar).unwrap().unwrap() - 6.0 * PI).abs() < 1e-9_f64);
    let holed = [square(10.0), circle(5.0, 5.0, 2.0)];
    let expected = 100.0_f64 - 4.0_f64 * PI;
    assert!((area(&holed, Metric::Plane, &Planar).unwrap().unwrap() - expected).abs() < 1e-12_f64);
    let reversed =
        [Curve::LineString(vec![xy(0.0, 0.0), xy(0.0, 10.0), xy(10.0, 10.0), xy(10.0, 0.0), xy(0.0, 0.0)]), circle(5.0, 5.0, 2.0)];
    assert!((area(&reversed, Metric::Plane, &Planar).unwrap().unwrap() - expected).abs() < 1e-12_f64);
    let c = centroid(&holed, Metric::Plane, &Planar).unwrap().unwrap();
    assert!((c.x - 5.0).abs() < 1e-12_f64 && (c.y - 5.0).abs() < 1e-12_f64);
    assert_eq!(area(&[], Metric::Plane, &Planar).unwrap(), None);
    assert_eq!(perimeter(&[Curve::LineString(Vec::new())], Metric::Plane, &Planar).unwrap(), None);
    assert_eq!(centroid(&[Curve::CircularString(Vec::new())], Metric::Plane, &Planar).unwrap(), None);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.1.6] A half
/// disk's centroid lies 4r/(3π) from its diameter, exactly, from an arc
/// closed by a segment.
#[test]
fn half_disk_centroid() {
    let ring = Curve::Compound(vec![
        Curve::CircularString(vec![xy(2.0, 0.0), xy(0.0, 2.0), xy(-2.0, 0.0)]),
        Curve::LineString(vec![xy(-2.0, 0.0), xy(2.0, 0.0)]),
    ]);
    let c = centroid(std::slice::from_ref(&ring), Metric::Plane, &Planar).unwrap().unwrap();
    assert!(c.x.abs() < 1e-15_f64 && (c.y - 8.0 / (3.0 * PI)).abs() < 1e-15_f64, "{c:?}");
    assert!((area(&[ring], Metric::Plane, &Planar).unwrap().unwrap() - 2.0 * PI).abs() < 1e-14_f64);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.1.2, §7.6.1,
/// §7.7.1] An ellipse encloses πab; a quarter disk bounded by a rational
/// NURBS arc encloses π/4 with its centroid at 4/(3π), by quadrature.
#[test]
fn ellipses_and_nurbs_regions() {
    let placement = Placement {
        location: xy(1.0, 1.0),
        directions: vec![Vector::of([1.0_f64, 0.0_f64, 0.0_f64]), Vector::of([0.0_f64, 1.0_f64, 0.0_f64])],
    };
    let ellipse = Curve::Elliptical(Some(Elliptical { placement, u_axis: 3.0, v_axis: 2.0, start: 0.0, end: 2.0 * PI, measures: None }));
    assert!((area(&[ellipse], Metric::Plane, &Planar).unwrap().unwrap() - 6.0 * PI).abs() < 1e-13_f64);
    let w = FRAC_1_SQRT_2;
    let arc = Nurbs {
        degree: 2,
        points: vec![
            NurbsPoint { point: xy(1.0, 0.0), weight: Some(1.0_f64) },
            NurbsPoint { point: xy(w, w), weight: Some(w) },
            NurbsPoint { point: xy(0.0, 1.0), weight: Some(1.0_f64) },
        ],
        knots: vec![Knot { value: 0.0, multiplicity: 3 }, Knot { value: 1.0, multiplicity: 3 }],
        measures: None,
    };
    let sector = Curve::Compound(vec![
        Curve::LineString(vec![xy(0.0, 0.0), xy(1.0, 0.0)]),
        Curve::Nurbs(Some(arc)),
        Curve::LineString(vec![xy(0.0, 1.0), xy(0.0, 0.0)]),
    ]);
    let a = area(std::slice::from_ref(&sector), Metric::Plane, &Planar).unwrap().unwrap();
    assert!((a - PI / 4.0).abs() < 1e-12_f64, "{a}");
    let c = centroid(&[sector], Metric::Plane, &Planar).unwrap().unwrap();
    assert!((c.x - 4.0 / (3.0 * PI)).abs() < 1e-12_f64 && (c.y - 4.0 / (3.0 * PI)).abs() < 1e-12_f64, "{c:?}");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.1.3, §8.1.5,
/// §8.1.7] A circle tilted in space has 3D area πr² and plane area
/// πr² cos(tilt); its 3D centroid is its center.
#[test]
fn tilted_disk_in_space() {
    let (c, s) = (0.6_f64, 0.8_f64);
    let ring = Curve::Circle(vec![Coord::xyz(c, 0.0, s), Coord::xyz(0.0, 1.0, 0.0), Coord::xyz(-c, 0.0, -s)]);
    let rings = [ring];
    assert!((area(&rings, Metric::Space, &Planar).unwrap().unwrap() - PI).abs() < 1e-13_f64);
    assert!((area(&rings, Metric::Plane, &Planar).unwrap().unwrap() - PI * c).abs() < 1e-13_f64);
    assert!((perimeter(&rings, Metric::Space, &Planar).unwrap().unwrap() - 2.0 * PI).abs() < 1e-13_f64);
    let center = centroid(&rings, Metric::Space, &Planar).unwrap().unwrap();
    assert!(norm(center.space()) < 1e-13_f64, "{center:?}");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.2.1] Rings that
/// do not close are refused with 2FF74; a ring without area has its
/// boundary's centroid.
#[test]
fn open_and_flat_rings() {
    let open = [Curve::LineString(vec![xy(0.0, 0.0), xy(1.0, 0.0), xy(1.0, 1.0)])];
    assert_eq!(area(&open, Metric::Plane, &Planar).unwrap_err().sqlstate().as_str(), "2FF74");
    let flat = [Curve::LineString(vec![xy(0.0, 0.0), xy(2.0, 0.0), xy(0.0, 0.0)])];
    assert_eq!(area(&flat, Metric::Plane, &Planar).unwrap(), Some(0.0_f64));
    let c = centroid(&flat, Metric::Plane, &Planar).unwrap().unwrap();
    assert_eq!((c.x, c.y), (1.0_f64, 0.0_f64));
    let holed = [square(1.0), Curve::LineString(Vec::new())];
    assert_eq!(area(&holed, Metric::Plane, &Planar).unwrap_err().sqlstate().as_str(), "2FF74");
}

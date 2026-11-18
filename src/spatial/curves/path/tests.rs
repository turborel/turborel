use super::*;
use crate::spatial::curves::geodesy::Planar;
use crate::spatial::curves::piece::Metric;
use crate::spatial::curves::types::{Knot, NurbsPoint, SpiralKind, Vector};
use std::f64::consts::PI;

fn total(path: &Path, metric: Metric) -> f64 {
    path.pieces.iter().map(|p| p.length(metric, &Planar).unwrap()).sum()
}

fn placement(directions: usize) -> Placement {
    let all = [Vector::of([1.0_f64, 0.0_f64, 0.0_f64]), Vector::of([0.0_f64, 1.0_f64, 0.0_f64]), Vector::of([0.0_f64, 0.0_f64, 1.0_f64])];
    Placement { location: Coord::xy(0.0, 0.0), directions: all.into_iter().take(directions).collect() }
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.3.1 D7, D8]
/// A circular string of two half circles is 2πr long, each arc split in
/// two at its middle point; collinear points make a segment.
#[test]
fn circular_string_pieces() {
    let points =
        [(0.0_f64, 0.0_f64), (1.0_f64, 1.0_f64), (2.0_f64, 0.0_f64), (3.0_f64, -1.0_f64), (4.0_f64, 0.0_f64)].map(|(x, y)| Coord::xy(x, y));
    let path = path(&Curve::CircularString(points.to_vec()), &Planar).unwrap().unwrap();
    assert_eq!(path.pieces.len(), 4);
    assert!((total(&path, Metric::Plane) - 2.0 * PI).abs() < 1e-13_f64);
    assert_eq!((path.start, path.end), (points[0], points[4]));
    let straight = [(0.0_f64, 0.0_f64), (1.0_f64, 1.0_f64), (2.0_f64, 2.0_f64)].map(|(x, y)| Coord::xy(x, y));
    let segment = super::path(&Curve::CircularString(straight.to_vec()), &Planar).unwrap().unwrap();
    assert_eq!(segment.pieces.len(), 1);
    for bad in [&points[..2], &points[..4]] {
        let error = super::path(&Curve::CircularString(bad.to_vec()), &Planar).unwrap_err();
        assert_eq!(error.sqlstate().as_str(), "2FF74");
    }
    let closed = [(0.0_f64, 0.0_f64), (1.0_f64, 1.0_f64), (0.0_f64, 0.0_f64)].map(|(x, y)| Coord::xy(x, y));
    assert!(super::path(&Curve::CircularString(closed.to_vec()), &Planar).is_err());
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.4.1 D6, D8] A
/// circle closes on its first point with length 2πr; two or four points
/// raise 2FF77 and collinear ones 2FF78.
#[test]
fn circle_pieces_and_faults() {
    let points = [(1.0_f64, 0.0_f64), (0.0_f64, 1.0_f64), (-1.0_f64, 0.0_f64)].map(|(x, y)| Coord::xy(x, y));
    let path = path(&Curve::Circle(points.to_vec()), &Planar).unwrap().unwrap();
    assert!((total(&path, Metric::Plane) - 2.0 * PI).abs() < 1e-13_f64);
    assert!(same(path.start.space(), path.end.space()));
    let error = super::path(&Curve::Circle(points[..2].to_vec()), &Planar).unwrap_err();
    assert_eq!(error.sqlstate().as_str(), "2FF77");
    let line = [(0.0_f64, 0.0_f64), (1.0_f64, 1.0_f64), (2.0_f64, 2.0_f64)].map(|(x, y)| Coord::xy(x, y));
    let collinear = super::path(&Curve::Circle(line.to_vec()), &Planar).unwrap_err();
    assert_eq!(collinear.sqlstate().as_str(), "2FF78");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.6.1, §7.8.1,
/// §7.9.1] A full ellipse's length matches Ramanujan's second formula;
/// placements without two directions raise 2FF86; bad parameters 2FF02.
#[test]
fn placed_curves() {
    let ellipse = Elliptical { placement: placement(2), u_axis: 3.0, v_axis: 2.0, start: 0.0, end: 2.0 * PI, measures: None };
    let path = path(&Curve::Elliptical(Some(ellipse.clone())), &Planar).unwrap().unwrap();
    let h = (1.0_f64 / 5.0).powi(2);
    let ramanujan = PI * 5.0_f64 * (1.0_f64 + 3.0_f64 * h / (10.0_f64 + (4.0 - 3.0 * h).sqrt()));
    assert!((total(&path, Metric::Plane) - ramanujan).abs() < 1e-5_f64);
    let flat = Elliptical { placement: placement(1), ..ellipse.clone() };
    assert_eq!(super::path(&Curve::Elliptical(Some(flat)), &Planar).unwrap_err().sqlstate().as_str(), "2FF86");
    let zero = Elliptical { u_axis: 0.0, ..ellipse };
    assert_eq!(super::path(&Curve::Elliptical(Some(zero)), &Planar).unwrap_err().sqlstate().as_str(), "2FF02");
    let clothoid = Clothoid { placement: placement(2), scale: 0.0, start: 0.0, end: 1.0, measures: None };
    assert_eq!(super::path(&Curve::Clothoid(Some(clothoid)), &Planar).unwrap_err().sqlstate().as_str(), "2FF02");
    let spiral =
        Spiral { placement: placement(3), length: 1.0, start_curvature: 0.0, end_curvature: 1.0, kind: SpiralKind::Sine, measures: None };
    assert_eq!(super::path(&Curve::Spiral(Some(spiral.clone())), &Planar).unwrap_err().sqlstate().as_str(), "2FF86");
    let negative = Spiral { placement: placement(2), length: -1.0, ..spiral };
    assert_eq!(super::path(&Curve::Spiral(Some(negative)), &Planar).unwrap_err().sqlstate().as_str(), "2FF02");
    let nan = Clothoid { placement: placement(2), scale: 1.0, start: f64::NAN, end: 1.0, measures: None };
    assert!(super::path(&Curve::Clothoid(Some(nan)), &Planar).is_err());
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.10.1 D7] A
/// compound curve's members must meet end to start, else 2FF11; empty
/// curves have no path; a NURBS member contributes its pieces.
#[test]
fn compound_contiguity() {
    let line = Curve::LineString(vec![Coord::xy(0.0, 0.0), Coord::xy(1.0, 0.0)]);
    let arc = Curve::CircularString(vec![Coord::xy(1.0, 0.0), Coord::xy(2.0, 1.0), Coord::xy(3.0, 0.0)]);
    let nurbs = Curve::Nurbs(Some(Nurbs {
        degree: 1,
        points: vec![NurbsPoint { point: Coord::xy(3.0, 0.0), weight: None }, NurbsPoint { point: Coord::xy(3.0, 4.0), weight: None }],
        knots: vec![Knot { value: 0.0, multiplicity: 2 }, Knot { value: 1.0, multiplicity: 2 }],
        measures: None,
    }));
    let path = path(&Curve::Compound(vec![line.clone(), arc, nurbs]), &Planar).unwrap().unwrap();
    assert!((total(&path, Metric::Plane) - (1.0 + PI + 4.0)).abs() < 1e-12_f64);
    assert_eq!(path.end, Coord::xy(3.0, 4.0));
    let gap = Curve::LineString(vec![Coord::xy(5.0, 0.0), Coord::xy(6.0, 0.0)]);
    let error = super::path(&Curve::Compound(vec![line.clone(), gap]), &Planar).unwrap_err();
    assert_eq!(error.sqlstate().as_str(), "2FF11");
    assert!(super::path(&Curve::Compound(Vec::new()), &Planar).unwrap().is_none());
    assert!(super::path(&Curve::LineString(Vec::new()), &Planar).unwrap().is_none());
    let one = super::path(&Curve::LineString(vec![Coord::xy(1.0, 2.0)]), &Planar).unwrap().unwrap();
    assert_eq!(one.start, Coord::xy(1.0, 2.0));
    let geo = super::path(&Curve::GeodesicString(vec![Coord::xy(0.0, 0.0), Coord::xy(3.0, 4.0)]), &Planar).unwrap().unwrap();
    assert_eq!(total(&geo, Metric::Plane), 5.0_f64);
}

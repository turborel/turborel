use super::*;
use crate::spatial::curves::geodesy::Planar;
use crate::spatial::curves::types::{Elliptical, Placement, Vector};
use std::f64::consts::{FRAC_PI_2, PI};

fn xy(x: f64, y: f64) -> Coord {
    Coord::xy(x, y)
}

fn arcs() -> Curve {
    Curve::CircularString(vec![xy(0.0, 0.0), xy(1.0, 1.0), xy(2.0, 0.0), xy(3.0, -1.0), xy(4.0, 0.0)])
}

fn ellipse(end: f64) -> Curve {
    let placement = Placement {
        location: xy(0.0, 0.0),
        directions: vec![Vector::of([1.0_f64, 0.0_f64, 0.0_f64]), Vector::of([0.0_f64, 1.0_f64, 0.0_f64])],
    };
    Curve::Elliptical(Some(Elliptical { placement, u_axis: 2.0, v_axis: 1.0, start: 0.0, end, measures: None }))
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.1.2 to §7.1.5]
/// Lengths and end points; the 3D length counts z; empty curves give
/// null.
#[test]
fn lengths_and_ends() {
    assert!((length(&arcs(), Metric::Plane, &Planar).unwrap().unwrap() - 2.0 * PI).abs() < 1e-13_f64);
    let rising = Curve::LineString(vec![Coord::xyz(0.0, 0.0, 0.0), Coord::xyz(3.0, 4.0, 12.0)]);
    assert_eq!(length(&rising, Metric::Plane, &Planar).unwrap(), Some(5.0_f64));
    assert_eq!(length(&rising, Metric::Space, &Planar).unwrap(), Some(13.0_f64));
    assert_eq!(start_point(&arcs(), &Planar).unwrap(), Some(xy(0.0, 0.0)));
    assert_eq!(end_point(&arcs(), &Planar).unwrap(), Some(xy(4.0, 0.0)));
    let empty = Curve::LineString(Vec::new());
    assert_eq!(length(&empty, Metric::Plane, &Planar).unwrap(), None);
    assert_eq!(start_point(&empty, &Planar).unwrap(), None);
    assert_eq!(end_point(&Curve::Nurbs(None), &Planar).unwrap(), None);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.1.6 to §7.1.9,
/// §7.4.1 D9] Closedness ignores or counts z; a circle and a full
/// ellipse are rings; a figure eight is closed but not a ring; empty
/// curves are neither.
#[test]
fn closed_and_rings() {
    let lifted =
        Curve::LineString(vec![Coord::xyz(0.0, 0.0, 0.0), Coord::xyz(1.0, 0.0, 0.0), Coord::xyz(1.0, 1.0, 0.0), Coord::xyz(0.0, 0.0, 5.0)]);
    assert!(is_closed(&lifted, Metric::Plane, &Planar).unwrap());
    assert!(!is_closed(&lifted, Metric::Space, &Planar).unwrap());
    let circle = Curve::Circle(vec![xy(1.0, 0.0), xy(0.0, 1.0), xy(-1.0, 0.0)]);
    assert!(is_ring(&circle, Metric::Plane, &Planar).unwrap());
    assert!(is_ring(&ellipse(2.0 * PI), Metric::Plane, &Planar).unwrap());
    assert!(!is_closed(&ellipse(PI), Metric::Plane, &Planar).unwrap());
    let eight = Curve::CircularString(vec![
        xy(0.0, 0.0),
        xy(1.0, 1.0),
        xy(2.0, 0.0),
        xy(3.0, -1.0),
        xy(4.0, 0.0),
        xy(3.0, 1.0),
        xy(2.0, 0.0),
        xy(1.0, -1.0),
        xy(0.0, 0.0),
    ]);
    assert!(is_closed(&eight, Metric::Plane, &Planar).unwrap());
    assert!(!is_ring(&eight, Metric::Plane, &Planar).unwrap());
    assert!(!is_closed(&Curve::LineString(Vec::new()), Metric::Plane, &Planar).unwrap());
    assert!(!is_ring(&Curve::Circle(Vec::new()), Metric::Plane, &Planar).unwrap());
    assert!(is_simple(&Curve::Spiral(None), Metric::Plane, &Planar).unwrap());
    assert!(is_simple(&arcs(), Metric::Plane, &Planar).unwrap());
    assert_eq!(curve_to_line(&Curve::Clothoid(None), &Tolerance::DEFAULT, &Planar).unwrap(), Vec::new());
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.1.11, §7.1.13]
/// The point a distance along a measured arc string carries its m; the
/// distance to that point reads back; distances beyond the length raise
/// 2FF79 and points off the curve 2FF80.
#[test]
fn distances_along_curves() {
    let measured =
        Curve::CircularString(vec![Coord::xyzm(1.0, 0.0, 0.0, 0.0), Coord::xyzm(0.0, 1.0, 0.0, 5.0), Coord::xyzm(-1.0, 0.0, 0.0, 10.0)]);
    let at = point_at_distance(&measured, FRAC_PI_2, Metric::Plane, &Planar).unwrap().unwrap();
    assert!(at.x.abs() < 1e-12_f64 && (at.y - 1.0).abs() < 1e-12_f64 && (at.m - 5.0).abs() < 1e-9_f64);
    let quarter = point_at_distance(&arcs(), PI / 4.0, Metric::Plane, &Planar).unwrap().unwrap();
    let back = distance_to_point(&arcs(), quarter, Metric::Plane, &Planar).unwrap().unwrap();
    assert!((back - PI / 4.0).abs() < 1e-9_f64, "{back}");
    assert_eq!(
        distance_to_point(&arcs(), xy(4.0, 0.0), Metric::Plane, &Planar).unwrap().map(|d| (d - 2.0 * PI).abs() < 1e-12_f64),
        Some(true)
    );
    assert_eq!(point_at_distance(&arcs(), 7.0, Metric::Plane, &Planar).unwrap_err().sqlstate().as_str(), "2FF79");
    assert_eq!(point_at_distance(&arcs(), -1.0, Metric::Plane, &Planar).unwrap_err().sqlstate().as_str(), "2FF02");
    assert_eq!(distance_to_point(&arcs(), xy(1.0, 0.0), Metric::Plane, &Planar).unwrap_err().sqlstate().as_str(), "2FF80");
    assert_eq!(point_at_distance(&Curve::LineString(Vec::new()), 1.0, Metric::Plane, &Planar).unwrap(), None);
    assert_eq!(distance_to_point(&Curve::LineString(Vec::new()), xy(0.0, 0.0), Metric::Plane, &Planar).unwrap(), None);
    let rising = Curve::LineString(vec![Coord::xyz(0.0, 0.0, 0.0), Coord::xyz(3.0, 4.0, 12.0)]);
    let top = point_at_distance(&rising, 13.0, Metric::Space, &Planar).unwrap().unwrap();
    assert_eq!(top, Coord::xyz(3.0, 4.0, 12.0));
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.1.15] A point
/// off a half circle projects on it once; the center projects on the
/// whole arc; a point on the curve projects on itself.
#[test]
fn perpendicular_projections() {
    let half = Curve::CircularString(vec![xy(1.0, 0.0), xy(0.0, 1.0), xy(-1.0, 0.0)]);
    let feet = perp_points(&half, xy(0.0, 3.0), &Planar).unwrap().unwrap();
    assert_eq!(feet.len(), 1);
    let Foot::Point(foot) = feet[0] else { panic!("{feet:?}") };
    assert!(foot.x.abs() < 1e-9_f64 && (foot.y - 1.0).abs() < 1e-9_f64);
    let center = perp_points(&half, xy(0.0, 0.0), &Planar).unwrap().unwrap();
    assert!(matches!(center.as_slice(), [Foot::Arc(_), Foot::Arc(_)]));
    assert_eq!(perp_points(&half, xy(1.0, 0.0), &Planar).unwrap().unwrap(), vec![Foot::Point(xy(1.0, 0.0))]);
    let line = Curve::LineString(vec![xy(0.0, 0.0), xy(4.0, 0.0)]);
    let on_line = perp_points(&line, xy(1.0, 2.0), &Planar).unwrap().unwrap();
    assert_eq!(on_line.len(), 1);
    assert_eq!(perp_points(&line, xy(9.0, 2.0), &Planar).unwrap(), Some(Vec::new()));
    assert_eq!(perp_points(&Curve::LineString(Vec::new()), xy(0.0, 0.0), &Planar).unwrap(), None);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.3.5 to §7.3.8]
/// Point and segment accessors count from one and report positions
/// outside the value; ST_MidPointRep gives each arc's middle.
#[test]
fn accessors() {
    let Curve::CircularString(points) = arcs() else { panic!() };
    assert_eq!(nth(&points, 1), Position::Found(xy(0.0, 0.0)));
    assert_eq!(nth(&points, 6), Position::Invalid);
    assert_eq!(nth(&points, 0), Position::Invalid);
    assert_eq!(nth::<Coord>(&[], 1), Position::Empty);
    assert_eq!(num_segments(&points), Some(2));
    assert_eq!(num_segments(&[]), None);
    assert_eq!(segment_n(&points, 2), Position::Found(points[2..].to_vec()));
    assert_eq!(segment_n(&points, 3), Position::Invalid);
    assert_eq!(segment_n(&[], 1), Position::Empty);
    let rep = mid_point_rep(&points).unwrap();
    assert_eq!(rep.len(), 5);
    assert!((rep[1].y - 1.0).abs() < 1e-12_f64 && (rep[3].y + 1.0).abs() < 1e-12_f64);
    assert_eq!(mid_point_rep(&[]), None);
    assert!(points_of(&arcs()));
}

fn points_of(curve: &Curve) -> bool {
    points(curve).is_some() && points(&Curve::Compound(Vec::new())).is_none()
}

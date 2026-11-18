use super::*;
use crate::spatial::curves::geodesy::{Planar, Sphere};
use crate::spatial::curves::types::{SpiralKind, Vector};
use std::f64::consts::{FRAC_PI_2, PI, TAU};

fn conic(a1: f64) -> Piece {
    Piece { shape: Shape::Conic { o: [0.0; 3], u: [2.0, 0.0, 0.0], v: [0.0, 2.0, 0.0], a0: 0.0, a1 }, m: [0.0, 10.0] }
}

fn placement() -> Placement {
    Placement { location: Coord::xy(1.0, 1.0), directions: vec![Vector::of([1.0, 0.0, 0.0]), Vector::of([0.0, 1.0, 0.0])] }
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.1.2] A circular
/// arc's length is r times its angle, its m runs linearly with length,
/// and the point at a distance lies at the matching angle.
#[test]
fn conic_length_and_location() {
    let arc = conic(PI);
    assert!((arc.length(Metric::Plane, &Planar).unwrap() - TAU).abs() < 1e-14_f64);
    let p = arc.locate(PI, Metric::Plane, &Planar).unwrap();
    assert!((p - FRAC_PI_2).abs() < 1e-12_f64);
    assert!((arc.measure_at(FRAC_PI_2, &Planar).unwrap() - 5.0).abs() < 1e-12_f64);
    let backward = Piece {
        shape: Shape::Conic { o: [0.0_f64; 3], u: [1.0_f64, 0.0_f64, 0.0_f64], v: [0.0_f64, 1.0_f64, 0.0_f64], a0: PI, a1: 0.0_f64 },
        m: [0.0_f64; 2],
    };
    assert!((backward.length(Metric::Plane, &Planar).unwrap() - PI).abs() < 1e-14_f64);
    let at = backward.locate(FRAC_PI_2, Metric::Plane, &Planar).unwrap();
    assert!((at - FRAC_PI_2).abs() < 1e-12_f64);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.1.2, §7.1.13]
/// Lines measure x and y for ST_Length and x, y and z for ST_3DLength.
#[test]
fn line_metrics() {
    let line = Piece { shape: Shape::Line { a: [0.0_f64; 3], b: [3.0_f64, 4.0_f64, 12.0_f64] }, m: [1.0_f64, 1.0_f64] };
    assert_eq!(line.length(Metric::Plane, &Planar).unwrap(), 5.0_f64);
    assert_eq!(line.length(Metric::Space, &Planar).unwrap(), 13.0_f64);
    assert_eq!(line.locate(2.5, Metric::Plane, &Planar).unwrap(), 0.5_f64);
    assert_eq!(line.measure_at(0.3, &Planar).unwrap(), 1.0_f64);
    assert_eq!(line.point(0.5, &Planar).unwrap(), [1.5_f64, 2.0_f64, 6.0_f64]);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.8.1, §7.9.1]
/// Clothoids and spirals are parametrized by arc length, so their length
/// is the parameter span and their speed is one.
#[test]
fn transition_curves_have_unit_speed() {
    let clothoid = Piece { shape: Shape::Clothoid { placement: placement(), scale: 10.0_f64, s0: 2.0_f64, s1: 7.0_f64 }, m: [0.0_f64; 2] };
    assert!((clothoid.length(Metric::Plane, &Planar).unwrap() - 5.0).abs() < 1e-12_f64);
    let transition = Transition { kind: SpiralKind::Cosine, length: 4.0, start: 0.0, end: 0.5 };
    let spiral = Piece { shape: Shape::Spiral { placement: placement(), transition }, m: [0.0_f64; 2] };
    assert!((spiral.length(Metric::Plane, &Planar).unwrap() - 4.0).abs() < 1e-12_f64);
    assert_eq!(spiral.point(0.0, &Planar).unwrap(), [1.0_f64, 1.0_f64, 0.0_f64]);
    let s = spiral.locate(1.5, Metric::Plane, &Planar).unwrap();
    assert!((s - 1.5).abs() < 1e-9_f64);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §4.2.8] A geodesic
/// piece's plane length is the geodesic distance and its 3D length adds
/// the rise; on a sphere its midpoint is on the great circle.
#[test]
fn geodesic_pieces() {
    let piece = Piece { shape: Shape::Geodesic { a: Coord::xyz(0.0, 0.0, 0.0), b: Coord::xyz(3.0, 4.0, 12.0) }, m: [0.0_f64, 2.0_f64] };
    assert_eq!(piece.length(Metric::Plane, &Planar).unwrap(), 5.0_f64);
    assert!((piece.length(Metric::Space, &Planar).unwrap() - 13.0).abs() < 1e-12_f64);
    assert!((piece.measure_at(2.5, &Planar).unwrap() - 1.0).abs() < 1e-12_f64);
    let sphere = Sphere { radius: 1.0 };
    let equator = Piece { shape: Shape::Geodesic { a: Coord::xy(0.0, 0.0), b: Coord::xy(90.0, 0.0) }, m: [0.0_f64; 2] };
    let p = equator.locate(FRAC_PI_2 / 2.0, Metric::Plane, &sphere).unwrap();
    let [x, _, _] = equator.point(p, &sphere).unwrap();
    assert!((x - 45.0).abs() < 1e-9_f64);
    let [vx, _, _] = equator.velocity(0.3, &sphere).unwrap();
    assert!(vx > 0.0_f64);
}

use super::*;
use crate::spatial::curves::geodesy::Planar;
use crate::spatial::curves::path::path;
use crate::spatial::curves::types::{Clothoid, Curve, Placement, Vector};

fn circle() -> Curve {
    Curve::Circle(vec![Coord::xy(1.0, 0.0), Coord::xy(0.0, 1.0), Coord::xy(-1.0, 0.0)])
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.1.10] A circle's
/// linearization closes on its start, every vertex lies on the circle,
/// and every chord's middle is within the tolerance of it.
#[test]
fn circle_chords_stay_within_tolerance() {
    let p = path(&circle(), &Planar).unwrap().unwrap();
    for deviation in [1e-1_f64, 1e-3_f64, 1e-6_f64] {
        let tolerance = Tolerance { deviation: Some(deviation), ..Tolerance::DEFAULT };
        let points = linearize(&p, &tolerance, &Planar).unwrap();
        assert_eq!(points.first(), points.last());
        for pair in points.windows(2) {
            let mid = [pair[0].x.midpoint(pair[1].x), pair[0].y.midpoint(pair[1].y)];
            let gap = 1.0_f64 - mid[0].hypot(mid[1]);
            assert!(gap <= deviation * (1.0_f64 + 1e-9_f64) && gap >= 0.0_f64, "{gap} > {deviation}");
            assert!((pair[1].x.hypot(pair[1].y) - 1.0).abs() < 1e-14_f64);
        }
    }
    let default = linearize(&p, &Tolerance::DEFAULT, &Planar).unwrap();
    assert!(default.len() > 40 && default.len() < 80, "{}", default.len());
    let capped = linearize(&p, &Tolerance { deviation: Some(1e-12_f64), relative: 0.0, max_segments: 5 }, &Planar).unwrap();
    assert!(capped.len() <= 16);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.1.10 D4] A
/// measured arc's linear points carry m interpolated by length.
#[test]
fn measures_follow_length() {
    let arc =
        Curve::CircularString(vec![Coord::xyzm(1.0, 0.0, 0.0, 0.0), Coord::xyzm(0.0, 1.0, 0.0, 10.0), Coord::xyzm(-1.0, 0.0, 0.0, 20.0)]);
    let p = path(&arc, &Planar).unwrap().unwrap();
    let points = linearize(&p, &Tolerance::DEFAULT, &Planar).unwrap();
    for q in &points {
        let angle = q.y.atan2(q.x);
        assert!((q.m - 20.0 * angle / PI).abs() < 1e-9_f64, "{q:?}");
    }
    assert_eq!(points.last().unwrap().m, 20.0_f64);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §7.1.10, §7.8.1]
/// Adaptive cutting keeps a clothoid's chords near the curve.
#[test]
fn clothoid_is_cut_adaptively() {
    let placement = Placement {
        location: Coord::xy(0.0, 0.0),
        directions: vec![Vector::of([1.0_f64, 0.0_f64, 0.0_f64]), Vector::of([0.0_f64, 1.0_f64, 0.0_f64])],
    };
    let curve = Curve::Clothoid(Some(Clothoid { placement, scale: 10.0, start: 0.0, end: 30.0, measures: None }));
    let p = path(&curve, &Planar).unwrap().unwrap();
    let tolerance = Tolerance { deviation: Some(1e-4_f64), ..Tolerance::DEFAULT };
    let samples = parameters(&p.pieces[0], &tolerance, &Planar).unwrap();
    for pair in samples.windows(2) {
        let (a, b) = (p.pieces[0].point(pair[0], &Planar).unwrap(), p.pieces[0].point(pair[1], &Planar).unwrap());
        let m = p.pieces[0].point(pair[0].midpoint(pair[1]), &Planar).unwrap();
        assert!(stray(a, b, m) <= 1e-4_f64, "{pair:?}");
    }
    assert_eq!(count(f64::NAN, 9), 1);
    assert_eq!(count(1e30, 9), 9);
    assert_eq!(count(2.2, 9), 3);
    assert_eq!(stray([0.0_f64; 3], [0.0_f64; 3], [3.0_f64, 4.0_f64, 0.0_f64]), 5.0_f64);
    assert_eq!(stray([0.0_f64; 3], [1.0_f64, 0.0_f64, 0.0_f64], [3.0_f64, 4.0_f64, 0.0_f64]), 20.0_f64.sqrt());
}

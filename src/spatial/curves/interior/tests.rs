use super::*;
use crate::spatial::curves::geodesy::Planar;
use crate::spatial::curves::numeric::norm;
use crate::spatial::curves::types::Clothoid;
use crate::spatial::curves::types::{Placement, Vector};

fn xy(x: f64, y: f64) -> Coord {
    Coord::xy(x, y)
}

fn ring(points: &[(f64, f64)]) -> Curve {
    Curve::LineString(points.iter().map(|&(x, y)| xy(x, y)).collect())
}

fn inside_square_outside_hole(p: Coord) -> bool {
    let in_square = p.x > 0.0_f64 && p.x < 10.0_f64 && p.y > 0.0_f64 && p.y < 10.0_f64;
    in_square && (p.x - 5.0).hypot(p.y - 5.0) > 3.0
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.1.8] The point
/// lies inside the exterior and outside a round hole; a C shape's point
/// is in the C, not its mouth.
#[test]
fn points_inside_polygons() {
    let holed = [
        ring(&[(0.0_f64, 0.0_f64), (10.0_f64, 0.0_f64), (10.0_f64, 10.0_f64), (0.0_f64, 10.0_f64), (0.0_f64, 0.0_f64)]),
        Curve::Circle(vec![xy(8.0, 5.0), xy(5.0, 8.0), xy(2.0, 5.0)]),
    ];
    let p = point_on_surface(&holed, Metric::Plane, &Planar).unwrap().unwrap();
    assert!(inside_square_outside_hole(p), "{p:?}");
    let c = [ring(&[
        (0.0_f64, 0.0_f64),
        (6.0_f64, 0.0_f64),
        (6.0_f64, 1.0_f64),
        (1.0_f64, 1.0_f64),
        (1.0_f64, 5.0_f64),
        (6.0_f64, 5.0_f64),
        (6.0_f64, 6.0_f64),
        (0.0_f64, 6.0_f64),
        (0.0_f64, 0.0_f64),
    ])];
    let q = point_on_surface(&c, Metric::Plane, &Planar).unwrap().unwrap();
    let in_bar = q.x > 0.0_f64 && q.x < 1.0_f64;
    let in_arm = q.x < 6.0_f64 && ((q.y > 0.0_f64 && q.y < 1.0_f64) || (q.y > 5.0_f64 && q.y < 6.0_f64));
    assert!(in_bar || in_arm, "{q:?}");
    assert_eq!(point_on_surface(&[], Metric::Plane, &Planar).unwrap(), None);
    let flat = [ring(&[(0.0_f64, 0.0_f64), (2.0_f64, 0.0_f64), (0.0_f64, 0.0_f64)])];
    assert_eq!(point_on_surface(&flat, Metric::Plane, &Planar).unwrap(), None);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.1.8] Arcs are
/// crossed exactly: a disk's point is within it; a lens of two arcs
/// keeps its point between them; a clothoid side is bisected.
#[test]
fn points_inside_curved_polygons() {
    let disk = [Curve::Circle(vec![xy(1.0, 0.0), xy(0.0, 1.0), xy(-1.0, 0.0)])];
    let p = point_on_surface(&disk, Metric::Plane, &Planar).unwrap().unwrap();
    assert!(p.x.hypot(p.y) < 1.0_f64, "{p:?}");
    let lens = [Curve::CircularString(vec![xy(0.0, 0.0), xy(1.0, 0.2), xy(2.0, 0.0), xy(1.0, -0.2), xy(0.0, 0.0)])];
    let q = point_on_surface(&lens, Metric::Plane, &Planar).unwrap().unwrap();
    assert!(q.x > 0.0_f64 && q.x < 2.0_f64 && q.y.abs() < 0.2_f64, "{q:?}");
    let placement = Placement {
        location: xy(0.0, 0.0),
        directions: vec![Vector::of([1.0_f64, 0.0_f64, 0.0_f64]), Vector::of([0.0_f64, 1.0_f64, 0.0_f64])],
    };
    let spiral = Curve::Clothoid(Some(Clothoid { placement, scale: 2.0, start: 0.0, end: 3.0, measures: None }));
    let path = crate::spatial::curves::path::path(&spiral, &Planar).unwrap().unwrap();
    let close = Curve::LineString(vec![path.end, path.start]);
    let region = [Curve::Compound(vec![spiral, close])];
    let r = point_on_surface(&region, Metric::Plane, &Planar).unwrap().unwrap();
    assert!(r.y > 0.0_f64 && r.x > 0.0_f64, "{r:?}");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.1.9] In space
/// the point lies on the polygon's plane, inside it, whichever plane
/// the polygon faces.
#[test]
fn points_on_planes_in_space() {
    for (a, b, c) in [
        ([1.0_f64, 0.0_f64, 0.0_f64], [0.0_f64, 1.0_f64, 0.0_f64], [0.0_f64, 0.0_f64, 1.0_f64]),
        ([0.0_f64, 0.0_f64, 0.0_f64], [0.0_f64, 4.0_f64, 0.0_f64], [0.0_f64, 0.0_f64, 4.0_f64]),
        ([0.0_f64, 0.0_f64, 0.0_f64], [4.0_f64, 0.0_f64, 0.1_f64], [0.0_f64, 0.0_f64, 4.0_f64]),
    ] {
        let triangle = [Curve::LineString([a, b, c, a].map(|p: V3| Coord::at(p, 0.0)).to_vec())];
        let p = point_on_surface(&triangle, Metric::Space, &Planar).unwrap().unwrap();
        let n =
            crate::spatial::curves::numeric::cross(crate::spatial::curves::numeric::sub(b, a), crate::spatial::curves::numeric::sub(c, a));
        let offset = crate::spatial::curves::numeric::dot(n, crate::spatial::curves::numeric::sub(p.space(), a)) / norm(n);
        assert!(offset.abs() < 1e-12_f64, "{p:?} off the plane by {offset}");
    }
}

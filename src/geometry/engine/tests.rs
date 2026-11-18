//! The bridge between model values and the planar engine.

use super::super::model::{Body, Collection, Coords, Curve, Dims, Geom, Kind, Surface, cast};
use super::*;

fn square(x: f64) -> Coords {
    vec![x, 0.0_f64, x + 2.0_f64, 0.0_f64, x + 2.0_f64, 2.0_f64, x, 2.0_f64, x, 0.0_f64]
}

fn polygon(srid: i32, ring: Coords) -> Geom {
    Geom::new(srid, Dims::Xy, Body::Surface(Surface::Polygon(vec![ring]))).unwrap()
}

/// Verifies: [ISO/IEC 13249-3:2016 §5.1.43, §5.1.65] spatial equality
/// ignores representation, compares x and y only, and keeps SRIDs apart.
#[test]
fn equality_is_spatial() {
    let first = polygon(0, square(0.0_f64));
    let rotated = polygon(0, vec![2.0_f64, 2.0_f64, 0.0_f64, 2.0_f64, 0.0_f64, 0.0_f64, 2.0_f64, 0.0_f64, 2.0_f64, 2.0_f64]);
    assert!(equal(&first, &rotated).unwrap());
    let with_vertex =
        polygon(0, vec![0.0_f64, 0.0_f64, 1.0_f64, 0.0_f64, 2.0_f64, 0.0_f64, 2.0_f64, 2.0_f64, 0.0_f64, 2.0_f64, 0.0_f64, 0.0_f64]);
    assert!(equal(&first, &with_vertex).unwrap());
    assert!(!equal(&first, &polygon(0, square(1.0_f64))).unwrap());
    assert!(!equal(&first, &polygon(4326, square(0.0_f64))).unwrap());
    let z = Geom::new(0, Dims::Xyz, Body::Point(vec![1.0_f64, 2.0_f64, 3.0_f64])).unwrap();
    let flat = Geom::new(0, Dims::Xy, Body::Point(vec![1.0_f64, 2.0_f64])).unwrap();
    assert!(equal(&z, &flat).unwrap());
    let empty_point = Geom::new(0, Dims::Xy, Body::Point(Vec::new())).unwrap();
    let empty_line = Geom::new(0, Dims::Xy, Body::Curve(Curve::LineString(Vec::new()))).unwrap();
    assert!(equal(&empty_point, &empty_line).unwrap());
    assert!(!equal(&empty_point, &flat).unwrap());
}

/// Verifies: [DESIGN] a result comes back as the most specific XY value,
/// which the casts of §4.2.1.5 Table 4 take to a wider type.
#[test]
fn results_come_back_as_values() {
    let (first, second) = (polygon(7, square(0.0_f64)), polygon(7, square(1.0_f64)));
    let union = planar::union(&engine_of(&first).unwrap(), &engine_of(&second).unwrap()).unwrap();
    let value = geom_of(&union, 7).unwrap();
    assert_eq!(value.kind(), Kind::Polygon);
    assert_eq!(value.srid, 7_i32);
    let multi = cast(&value, Kind::MultiPolygon).unwrap();
    assert!(matches!(multi.body, Body::Collection(Collection::MultiPolygon(ref list)) if list.len() == 1));
    let apart = planar::sym_difference(&engine_of(&first).unwrap(), &engine_of(&second).unwrap()).unwrap();
    assert_eq!(geom_of(&apart, 7).unwrap().kind(), Kind::MultiPolygon);
    let nothing = planar::difference(&engine_of(&first).unwrap(), &engine_of(&first).unwrap()).unwrap();
    let empty = geom_of(&nothing, 7).unwrap();
    assert!(empty.is_empty());
    assert_eq!(empty.kind(), Kind::GeomCollection);
}

/// Verifies: [DESIGN] collections of every linear kind reach the engine
/// with their parts.
#[test]
fn collections_reach_the_engine() {
    let multi = Geom::new(
        0,
        Dims::Xy,
        Body::Collection(Collection::Geometry(vec![
            Body::Point(vec![5.0_f64, 5.0_f64]),
            Body::Curve(Curve::LineString(vec![0.0_f64, 0.0_f64, 1.0_f64, 1.0_f64])),
            Body::Collection(Collection::MultiPolygon(vec![vec![square(0.0_f64)]])),
        ])),
    )
    .unwrap();
    let Geometry::Collection(items) = engine_of(&multi).unwrap() else { panic!("not a collection") };
    assert_eq!(items.len(), 3);
    assert!(matches!(items[2], Geometry::MultiPolygon(ref list) if list.len() == 1));
}

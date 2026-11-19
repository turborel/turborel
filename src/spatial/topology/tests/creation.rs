//! Tests of ST_CreateTopoGeo on each kind of collection, and of an edge
//! along another's curve the other way.

use super::*;
use crate::geometry::model::Collection;

/// The topology ST_CreateTopoGeo makes of `body`, or its error.
fn made(body: Body) -> Result<Topology> {
    let mut t = fresh();
    create(&mut t, &Geom::new(0, Dims::Xy, body)?).map(|()| t)
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.18 Description
/// 2i] a multipoint's points become isolated nodes, a multilinestring's
/// lines and a multipolygon's rings edges, a collection within a
/// collection its parts.
#[test]
fn created_of_each_collection() {
    let points = made(Body::Collection(Collection::MultiPoint(vec![vec![0.0_f64, 0.0_f64], vec![3.0_f64, 4.0_f64]]))).unwrap();
    assert_eq!((points.nodes.len(), points.edges.len()), (2, 0));
    let lines = made(Body::Collection(Collection::MultiLineString(vec![vec![0.0_f64, 0.0_f64, 4.0_f64, 0.0_f64]]))).unwrap();
    assert_eq!((lines.nodes.len(), lines.edges.len()), (2, 1));
    let ring = vec![0.0_f64, 0.0_f64, 4.0_f64, 0.0_f64, 4.0_f64, 4.0_f64, 0.0_f64, 0.0_f64];
    let polygons = made(Body::Collection(Collection::MultiPolygon(vec![vec![ring.clone()]]))).unwrap();
    assert_eq!((polygons.edges.len(), polygons.faces.len()), (1, 2));
    consistent(&polygons);
    let nested =
        Body::Collection(Collection::Geometry(vec![Body::Collection(Collection::Geometry(vec![Body::Point(vec![9.0_f64, 9.0_f64])]))]));
    assert_eq!(made(nested).unwrap().nodes.len(), 1);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.18 Description
/// 2h] a solid, alone or in a collection, makes no topology (2FF02).
#[test]
fn created_of_no_solid() {
    let ring = vec![0.0_f64, 0.0_f64, 0.0_f64, 4.0_f64, 0.0_f64, 0.0_f64, 0.0_f64, 4.0_f64, 0.0_f64, 0.0_f64, 0.0_f64, 0.0_f64];
    let solid = Body::Solid(vec![Surface::PolyhdrlSurface(vec![vec![ring]])]);
    let mut t = fresh();
    assert_eq!(state(create(&mut t, &Geom { srid: 0, dims: Dims::Xyz, body: solid.clone() })), "2FF02");
    let held = Geom { srid: 0, dims: Dims::Xyz, body: Body::Collection(Collection::Geometry(vec![solid])) };
    assert_eq!(state(create(&mut fresh(), &held)), "2FF02");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.12 Description
/// 2f] an edge joining two nodes along the curve of another edge between
/// them, taken the other way, is a coincident edge (2FF27).
#[test]
fn a_reversed_coincident_edge() {
    let mut t = square();
    t.add_edge(1, 3, &line(&[(0.0_f64, 0.0_f64), (10.0_f64, 10.0_f64)]), Faces::Modify).unwrap();
    assert_eq!(state(t.add_edge(3, 1, &line(&[(10.0_f64, 10.0_f64), (0.0_f64, 0.0_f64)]), Faces::Modify)), "2FF27");
}

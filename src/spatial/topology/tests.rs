//! Tests of topology-geometry's routines on topologies held in memory.

use super::*;
use crate::geometry::model::Surface;

mod build;
mod checks;
mod creation;
mod edits;
mod faces;

/// A point of SRID 0.
fn pt(x: f64, y: f64) -> Geom {
    Geom::new(0, Dims::Xy, Body::Point(vec![x, y])).unwrap()
}

/// A linestring of SRID 0.
fn line(points: &[(f64, f64)]) -> Geom {
    Geom::new(0, Dims::Xy, Body::Curve(Curve::LineString(points.iter().flat_map(|(x, y)| [*x, *y]).collect()))).unwrap()
}

/// A topology of only its universal face, as ST_InitTopoGeo makes it.
fn fresh() -> Topology {
    let mut t = Topology::default();
    t.faces.insert(UNIVERSAL_FACE, Face { mbr: None });
    t
}

/// The SQLSTATE of `r`'s error.
fn state<T: std::fmt::Debug>(r: Result<T>) -> String {
    r.unwrap_err().sqlstate().as_str().to_owned()
}

/// That `t` is consistent: no problem found, and its next edges are what
/// the stars give.
fn consistent(t: &Topology) {
    if !(t.nodes.is_empty() && t.edges.is_empty()) {
        assert_eq!(validate(t).unwrap(), Vec::new());
    }
    let mut again = t.clone();
    let nodes: Vec<i32> = t.nodes.keys().copied().collect();
    again.relink(&nodes).unwrap();
    assert_eq!(again.edges, t.edges);
    for (face, ring) in t.rings().unwrap() {
        for s in ring {
            assert_eq!(t.left_of(s).unwrap(), face);
        }
    }
}

/// A square of side 10 at the origin, its four nodes 1 to 4 counterclockwise
/// from the origin and its edges 1 to 4 along its sides, added with
/// ST_AddEdgeModFace; the face inside is 1.
fn square() -> Topology {
    let mut t = fresh();
    for (x, y) in [(0.0_f64, 0.0_f64), (10.0_f64, 0.0_f64), (10.0_f64, 10.0_f64), (0.0_f64, 10.0_f64)] {
        t.add_iso_node(None, &pt(x, y)).unwrap();
    }
    t.add_edge(1, 2, &line(&[(0.0_f64, 0.0_f64), (10.0_f64, 0.0_f64)]), Faces::Modify).unwrap();
    t.add_edge(2, 3, &line(&[(10.0_f64, 0.0_f64), (10.0_f64, 10.0_f64)]), Faces::Modify).unwrap();
    t.add_edge(3, 4, &line(&[(10.0_f64, 10.0_f64), (0.0_f64, 10.0_f64)]), Faces::Modify).unwrap();
    t.add_edge(4, 1, &line(&[(0.0_f64, 10.0_f64), (0.0_f64, 0.0_f64)]), Faces::Modify).unwrap();
    t
}

/// The SRID-0 polygon of `ring`.
fn polygon_of(ring: &[(f64, f64)]) -> Geom {
    Geom::new(0, Dims::Xy, Body::Surface(Surface::Polygon(vec![ring.iter().flat_map(|(x, y)| [*x, *y]).collect()]))).unwrap()
}

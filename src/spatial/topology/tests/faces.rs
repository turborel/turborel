//! Tests of the faces of a topology and of ST_ValidateTopoGeo.

use super::*;

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.5, §11.3.16]
/// a face with a dangling edge inside lists the edge both ways, and its
/// geometry leaves the edge out.
#[test]
fn a_dangling_edge_bounds_no_area() {
    let mut t = square();
    let inside = t.add_iso_node(None, &pt(3.0, 3.0)).unwrap();
    t.add_edge(1, inside, &line(&[(0.0_f64, 0.0_f64), (3.0_f64, 3.0_f64)]), Faces::Modify).unwrap();
    let edges = face_edges(&t, 1).unwrap();
    assert!(edges.contains(&5_i32) && edges.contains(&-5_i32), "{edges:?}");
    assert_eq!(
        face_geometry(&t, 1).unwrap(),
        polygon_of(&[(0.0_f64, 0.0_f64), (10.0_f64, 0.0_f64), (10.0_f64, 10.0_f64), (0.0_f64, 10.0_f64), (0.0_f64, 0.0_f64)])
    );
    consistent(&t);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.19] the
/// inconsistencies of a topology edited by hand are found: coincident
/// nodes, an edge through a node, a crossing, a mismatched end, a face of
/// no edges, a missing universal face and mixed systems.
#[test]
fn validate_finds_inconsistencies() {
    let mut t = square();
    let mut bad = t.clone();
    bad.nodes.insert(9_i32, Node { geometry: pt(0.0, 0.0), containing_face: Some(0_i32) });
    bad.nodes.insert(10_i32, Node { geometry: pt(5.0_f64, 10.0_f64), containing_face: Some(0_i32) });
    let errors: Vec<_> = validate(&bad).unwrap().into_iter().map(|p| (p.error, p.primitive1, p.primitive2)).collect();
    assert!(errors.contains(&(validate::COINCIDENT_NODES, Some(1_i32), Some(9_i32))), "{errors:?}");
    assert!(errors.contains(&(validate::EDGE_CROSSED_NODE, Some(10_i32), Some(3_i32))), "{errors:?}");
    let mut moved = t.clone();
    let mut edge = moved.edges[&1_i32].clone();
    edge.geometry = line(&[(0.0_f64, 0.0_f64), (10.0_f64, 10.0_f64)]);
    moved.edges.insert(1_i32, edge);
    let mismatched: Vec<_> = validate(&moved).unwrap().into_iter().map(|p| p.error).collect();
    assert!(mismatched.contains(&validate::GEOMETRY_MISMATCH), "{mismatched:?}");
    let mut crossing = t.clone();
    crossing.edges.get_mut(&1_i32).unwrap().geometry = line(&[(0.0_f64, 0.0_f64), (10.0_f64, 10.0_f64), (10.0_f64, 0.0_f64)]);
    let crossed: Vec<_> = validate(&crossing).unwrap().into_iter().map(|p| p.error).collect();
    assert!(crossed.contains(&validate::EDGE_CROSSES_EDGE), "{crossed:?}");
    t.faces.insert(7_i32, Face { mbr: None });
    t.faces.remove(&0_i32);
    t.nodes.get_mut(&1_i32).unwrap().geometry = Geom::new(4326, Dims::Xy, Body::Point(vec![0.0_f64, 0.0_f64])).unwrap();
    let broken: Vec<_> = validate(&t).unwrap().into_iter().map(|p| p.error).collect();
    for problem in [validate::FACE_WITHOUT_EDGES, validate::NO_UNIVERSAL_FACE, validate::MIXED_SRIDS] {
        assert!(broken.contains(&problem), "{problem} not in {broken:?}");
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.12] a grid
/// built edge by edge with ST_AddEdgeNewFaces stays consistent, each cell a
/// face of its own.
#[test]
fn a_grid_of_faces() {
    let mut t = fresh();
    let mut id = BTreeMap::new();
    for i in 0_i32..3_i32 {
        for j in 0_i32..3_i32 {
            let (x, y) = (f64::from(i) * 5.0_f64, f64::from(j) * 5.0_f64);
            id.insert((i, j), t.add_iso_node(None, &pt(x, y)).unwrap());
        }
    }
    for i in 0_i32..3_i32 {
        for j in 0_i32..3_i32 {
            let (x, y) = (f64::from(i) * 5.0_f64, f64::from(j) * 5.0_f64);
            if i < 2_i32 {
                t.add_edge(id[&(i, j)], id[&(i + 1, j)], &line(&[(x, y), (x + 5.0_f64, y)]), Faces::New).unwrap();
            }
            if j < 2_i32 {
                t.add_edge(id[&(i, j)], id[&(i, j + 1)], &line(&[(x, y), (x, y + 5.0_f64)]), Faces::New).unwrap();
            }
            consistent(&t);
        }
    }
    assert_eq!(t.faces.len(), 5);
    for f in t.faces.keys().filter(|f| **f != 0_i32) {
        let Body::Surface(Surface::Polygon(rings)) = face_geometry(&t, *f).unwrap().body else { panic!() };
        assert_eq!(rings.len(), 1);
        assert_eq!(face_edges(&t, *f).unwrap().len(), 4);
    }
}

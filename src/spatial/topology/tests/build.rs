//! Tests of the routines that build and edit a topology.

use super::*;

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.1, §11.3.4]
/// an isolated node goes to the face it is within, an isolated edge
/// between two isolated nodes points to itself both ways and takes their
/// face, and its nodes are then no longer isolated.
#[test]
fn isolated_nodes_and_edges() {
    let mut t = fresh();
    assert_eq!(t.add_iso_node(None, &pt(0.0, 0.0)).unwrap(), 1_i32);
    assert_eq!(t.add_iso_node(Some(0_i32), &pt(5.0, 0.0)).unwrap(), 2_i32);
    assert_eq!(t.nodes[&1_i32].containing_face, Some(0_i32));
    let e = t.add_iso_edge(1, 2, &line(&[(0.0_f64, 0.0_f64), (2.0_f64, 1.0_f64), (5.0_f64, 0.0_f64)])).unwrap();
    let edge = &t.edges[&e];
    assert_eq!((edge.next_left, edge.next_right, edge.left_face, edge.right_face), (-e, e, 0_i32, 0_i32));
    assert_eq!(t.nodes[&1_i32].containing_face, None);
    consistent(&t);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.13] closing a
/// ring with ST_AddEdgeModFace makes a face inside it, with the edges
/// counterclockwise round it, the outside left to the universal face.
#[test]
fn a_ring_makes_a_face() {
    let t = square();
    assert_eq!(t.faces.keys().copied().collect::<Vec<_>>(), vec![0_i32, 1_i32]);
    for e in 1_i32..=4_i32 {
        assert_eq!((t.edges[&e].left_face, t.edges[&e].right_face), (1_i32, 0_i32), "edge {e}");
    }
    assert_eq!(face_edges(&t, 1).unwrap(), vec![1_i32, 2_i32, 3_i32, 4_i32]);
    assert_eq!(face_edges(&t, 0).unwrap(), vec![-4_i32, -3_i32, -2_i32, -1_i32]);
    assert_eq!(
        face_geometry(&t, 1).unwrap(),
        polygon_of(&[(0.0_f64, 0.0_f64), (10.0_f64, 0.0_f64), (10.0_f64, 10.0_f64), (0.0_f64, 10.0_f64), (0.0_f64, 0.0_f64)])
    );
    assert_eq!(
        t.faces[&1_i32].mbr,
        Some(polygon_of(&[(0.0_f64, 0.0_f64), (10.0_f64, 0.0_f64), (10.0_f64, 10.0_f64), (0.0_f64, 10.0_f64), (0.0_f64, 0.0_f64)]))
    );
    consistent(&t);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.12, §11.3.13]
/// an edge across a face splits it: ST_AddEdgeNewFaces replaces it with two
/// new faces, ST_AddEdgeModFace keeps it on the right and makes one on the
/// left; an isolated node inside goes to the part that holds it.
#[test]
fn an_edge_splits_a_face() {
    let mut t = square();
    let inside = t.add_iso_node(None, &pt(2.0, 7.0)).unwrap();
    assert_eq!(t.nodes[&inside].containing_face, Some(1_i32));
    let mut modified = t.clone();
    let e = modified.add_edge(1, 3, &line(&[(0.0_f64, 0.0_f64), (10.0_f64, 10.0_f64)]), Faces::Modify).unwrap();
    assert_eq!((modified.edges[&e].left_face, modified.edges[&e].right_face), (2_i32, 1_i32));
    assert_eq!(modified.nodes[&inside].containing_face, Some(2_i32));
    consistent(&modified);
    let new_edge = t.add_edge(1, 3, &line(&[(0.0_f64, 0.0_f64), (10.0_f64, 10.0_f64)]), Faces::New).unwrap();
    assert_eq!((t.edges[&new_edge].left_face, t.edges[&new_edge].right_face), (2_i32, 3_i32));
    assert!(!t.faces.contains_key(&1_i32));
    assert_eq!(t.nodes[&inside].containing_face, Some(2_i32));
    consistent(&t);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.13] a ring of
/// edges inside a face that touches nothing makes a face of its own,
/// which becomes a hole of the face round it.
#[test]
fn a_ring_inside_a_face_is_a_hole() {
    let mut t = square();
    for (x, y) in [(2.0_f64, 2.0_f64), (4.0_f64, 2.0_f64), (3.0_f64, 4.0_f64)] {
        t.add_iso_node(None, &pt(x, y)).unwrap();
    }
    t.add_edge(5, 6, &line(&[(2.0_f64, 2.0_f64), (4.0_f64, 2.0_f64)]), Faces::Modify).unwrap();
    t.add_edge(6, 7, &line(&[(4.0_f64, 2.0_f64), (3.0_f64, 4.0_f64)]), Faces::Modify).unwrap();
    t.add_edge(7, 5, &line(&[(3.0_f64, 4.0_f64), (2.0_f64, 2.0_f64)]), Faces::Modify).unwrap();
    let Body::Surface(Surface::Polygon(rings)) = face_geometry(&t, 1).unwrap().body else { panic!() };
    assert_eq!(rings.len(), 2);
    assert_eq!(face_edges(&t, 2).unwrap(), vec![5_i32, 6_i32, 7_i32]);
    consistent(&t);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.8 to §11.3.11]
/// splitting an edge at a new node, either way, and healing it again,
/// either way, keeps the topology consistent and the edges' directions.
#[test]
fn split_and_heal() {
    let mut t = square();
    let n = t.split_edge(1, &pt(4.0, 0.0), false).unwrap();
    assert_eq!(n, 5_i32);
    assert_eq!((t.edges[&1_i32].start, t.edges[&1_i32].end, t.edges[&5_i32].start, t.edges[&5_i32].end), (1_i32, 5_i32, 5_i32, 2_i32));
    assert_eq!(t.edges[&1_i32].next_left, 5_i32);
    assert_eq!(t.edges[&5_i32].next_right, -1_i32);
    consistent(&t);
    t.heal(1, 5, false).unwrap();
    assert_eq!(t.edges[&1_i32].geometry, line(&[(0.0_f64, 0.0_f64), (4.0_f64, 0.0_f64), (10.0_f64, 0.0_f64)]));
    assert!(!t.nodes.contains_key(&5_i32));
    consistent(&t);
    let middle = t.split_edge(4, &pt(0.0, 5.0), true).unwrap();
    assert!(!t.edges.contains_key(&4_i32));
    assert_eq!((t.edges[&5_i32].start, t.edges[&5_i32].end, t.edges[&6_i32].start, t.edges[&6_i32].end), (4_i32, middle, middle, 1_i32));
    consistent(&t);
    let made = t.heal(6, 5, true).unwrap().unwrap();
    assert_eq!(made, 7_i32);
    assert_eq!((t.edges[&made].start, t.edges[&made].end), (4_i32, 1_i32));
    assert_eq!(t.edges[&made].geometry, line(&[(0.0_f64, 10.0_f64), (0.0_f64, 5.0_f64), (0.0_f64, 0.0_f64)]));
    consistent(&t);
    let right_middle = t.split_edge(2, &pt(10.0, 5.0), true).unwrap();
    let healed = t.heal(9, 8, true).unwrap().unwrap();
    assert!(!t.nodes.contains_key(&right_middle));
    assert_eq!((t.edges[&healed].start, t.edges[&healed].end), (2_i32, 3_i32));
    assert_eq!(t.edges[&healed].geometry, line(&[(10.0_f64, 0.0_f64), (10.0_f64, 5.0_f64), (10.0_f64, 10.0_f64)]));
    consistent(&t);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.14, §11.3.15]
/// removing an edge between two faces heals them: into a new face, or into
/// the left one; between a face and the universal face, into the
/// universal face; and its nodes are kept.
#[test]
fn removing_edges_heals_faces() {
    let mut t = square();
    t.add_edge(1, 3, &line(&[(0.0_f64, 0.0_f64), (10.0_f64, 10.0_f64)]), Faces::Modify).unwrap();
    let mut modified = t.clone();
    assert_eq!(modified.remove_edge(5, false).unwrap(), None);
    assert_eq!(modified.faces.keys().copied().collect::<Vec<_>>(), vec![0_i32, 2_i32]);
    consistent(&modified);
    assert_eq!(t.remove_edge(5, true).unwrap(), Some(3_i32));
    assert_eq!(t.faces.keys().copied().collect::<Vec<_>>(), vec![0_i32, 3_i32]);
    consistent(&t);
    assert_eq!(t.remove_edge(1, true).unwrap(), None);
    assert_eq!(t.faces.keys().copied().collect::<Vec<_>>(), vec![0_i32]);
    assert_eq!(t.nodes.len(), 4);
    consistent(&t);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.2, §11.3.3,
/// §11.3.6, §11.3.7] moving and removing isolated nodes, changing an
/// edge's curve and removing an isolated edge, which leaves its nodes
/// isolated.
#[test]
fn move_change_and_remove() {
    let mut t = fresh();
    t.add_iso_node(None, &pt(0.0, 0.0)).unwrap();
    t.add_iso_node(None, &pt(5.0, 0.0)).unwrap();
    t.add_iso_node(None, &pt(9.0, 9.0)).unwrap();
    t.move_iso_node(3, &pt(8.0, 8.0)).unwrap();
    assert_eq!(t.nodes[&3_i32].geometry, pt(8.0, 8.0));
    t.remove_iso_node(3).unwrap();
    let e = t.add_iso_edge(1, 2, &line(&[(0.0_f64, 0.0_f64), (5.0_f64, 0.0_f64)])).unwrap();
    t.change_edge_geom(e, &line(&[(0.0_f64, 0.0_f64), (2.0_f64, 3.0_f64), (5.0_f64, 0.0_f64)])).unwrap();
    assert_eq!(t.edges[&e].geometry, line(&[(0.0_f64, 0.0_f64), (2.0_f64, 3.0_f64), (5.0_f64, 0.0_f64)]));
    t.remove_iso_edge(e).unwrap();
    assert_eq!(t.nodes[&1_i32].containing_face, Some(0_i32));
    consistent(&t);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.18] a
/// collection is noded into a topology: crossing lines meet at a new node,
/// a polygon's ring bounds a face, a point inside it is an isolated node of
/// that face, and a hole of the polygon is a face of its own.
#[test]
fn create_from_a_collection() {
    let mut t = fresh();
    let cross = Geom::new(
        0,
        Dims::Xy,
        Body::Collection(crate::geometry::model::Collection::Geometry(vec![
            line(&[(0.0_f64, 0.0_f64), (10.0_f64, 10.0_f64)]).body,
            line(&[(0.0_f64, 10.0_f64), (10.0_f64, 0.0_f64)]).body,
        ])),
    )
    .unwrap();
    create(&mut t, &cross).unwrap();
    assert_eq!((t.nodes.len(), t.edges.len(), t.faces.len()), (5, 4, 1));
    consistent(&t);
    let mut holed = fresh();
    let square = vec![0.0_f64, 0.0_f64, 10.0_f64, 0.0_f64, 10.0_f64, 10.0_f64, 0.0_f64, 10.0_f64, 0.0_f64, 0.0_f64];
    let hole = vec![4.0_f64, 4.0_f64, 4.0_f64, 6.0_f64, 6.0_f64, 6.0_f64, 6.0_f64, 4.0_f64, 4.0_f64, 4.0_f64];
    let parts = vec![Body::Surface(Surface::Polygon(vec![square, hole])), Body::Point(vec![2.0_f64, 2.0_f64])];
    let g = Geom::new(0, Dims::Xy, Body::Collection(crate::geometry::model::Collection::Geometry(parts))).unwrap();
    create(&mut holed, &g).unwrap();
    assert_eq!((holed.nodes.len(), holed.edges.len(), holed.faces.len()), (3, 2, 3));
    let isolated: Vec<Option<i32>> = holed.nodes.values().filter_map(|n| n.containing_face).map(Some).collect();
    assert_eq!(isolated.len(), 1);
    consistent(&holed);
    assert_eq!(state(create(&mut holed, &g)), "2FF42");
}

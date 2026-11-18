//! Tests of the exceptions topology-geometry's routines raise.

use super::*;

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.1 to §11.3.3]
/// the checks of isolated nodes: a node on another, on an edge, in the
/// wrong face, a node that is not isolated and one that is not there.
#[test]
fn node_checks() {
    let mut t = square();
    assert_eq!(state(t.add_iso_node(None, &pt(0.0, 0.0))), "2FF28");
    assert_eq!(state(t.add_iso_node(None, &pt(5.0, 0.0))), "2FF30");
    assert_eq!(state(t.add_iso_node(Some(0_i32), &pt(5.0, 5.0))), "2FF51");
    assert_eq!(state(t.move_iso_node(1, &pt(5.0, 5.0))), "2FF50");
    assert_eq!(state(t.remove_iso_node(1)), "2FF50");
    assert_eq!(state(t.remove_iso_node(42)), "2FF46");
    let n = t.add_iso_node(None, &pt(5.0, 5.0)).unwrap();
    assert_eq!(state(t.move_iso_node(n, &pt(5.0, 5.0))), "2FF28");
    assert_eq!(state(t.move_iso_node(n, &pt(0.0, 5.0))), "2FF30");
    assert_eq!(state(t.add_iso_node(None, &Geom::new(4326, Dims::Xy, Body::Point(vec![1.0_f64, 1.0_f64])).unwrap())), "2FF10");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.4] the checks
/// of an isolated edge: its ends, its nodes, what it meets and its face.
#[test]
fn edge_checks() {
    let mut t = square();
    let a = t.add_iso_node(None, &pt(2.0, 2.0)).unwrap();
    let b = t.add_iso_node(None, &pt(8.0, 2.0)).unwrap();
    assert_eq!(state(t.add_iso_edge(a, 99, &line(&[(2.0_f64, 2.0_f64), (8.0_f64, 2.0_f64)]))), "2FF46");
    assert_eq!(state(t.add_iso_edge(1, b, &line(&[(0.0_f64, 0.0_f64), (8.0_f64, 2.0_f64)]))), "2FF50");
    assert_eq!(state(t.add_iso_edge(a, b, &line(&[(3.0_f64, 2.0_f64), (8.0_f64, 2.0_f64)]))), "2FF57");
    assert_eq!(state(t.add_iso_edge(a, b, &line(&[(2.0_f64, 2.0_f64), (7.0_f64, 2.0_f64)]))), "2FF33");
    assert_eq!(state(t.add_iso_edge(a, b, &line(&[(2.0_f64, 2.0_f64), (5.0_f64, 12.0_f64), (8.0_f64, 2.0_f64)]))), "2FF37");
    assert_eq!(state(t.add_iso_edge(a, b, &line(&[(2.0_f64, 2.0_f64), (5.0_f64, 10.0_f64), (8.0_f64, 2.0_f64)]))), "2FF36");
    assert_eq!(
        state(t.add_iso_edge(a, b, &line(&[(2.0_f64, 2.0_f64), (6.0_f64, 4.0_f64), (4.0_f64, 4.0_f64), (8.0_f64, 2.0_f64)]))),
        "2FF29"
    );
    t.add_iso_edge(a, b, &line(&[(2.0_f64, 2.0_f64), (8.0_f64, 2.0_f64)])).unwrap();
    consistent(&t);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.8 to §11.3.15]
/// the checks of splitting, healing, adding and removing edges.
#[test]
fn split_heal_and_face_checks() {
    let mut t = square();
    assert_eq!(state(t.split_edge(9, &pt(5.0, 0.0), true)), "2FF43");
    assert_eq!(state(t.split_edge(1, &pt(5.0, 1.0), true)), "2FF54");
    assert_eq!(state(t.split_edge(1, &pt(0.0, 0.0), false)), "2FF54");
    assert_eq!(state(t.heal(1, 3, true)), "2FF40");
    assert_eq!(state(t.heal(1, 1, false)), "2FF40");
    t.add_edge(1, 3, &line(&[(0.0_f64, 0.0_f64), (10.0_f64, 10.0_f64)]), Faces::Modify).unwrap();
    assert_eq!(state(t.heal(1, 4, false)), "2FF52");
    assert_eq!(state(t.add_edge(2, 4, &line(&[(10.0_f64, 0.0_f64), (0.0_f64, 10.0_f64)]), Faces::New)), "2FF35");
    assert_eq!(state(t.add_edge(1, 3, &line(&[(0.0_f64, 0.0_f64), (10.0_f64, 10.0_f64)]), Faces::New)), "2FF27");
    assert_eq!(state(t.remove_edge(77, true)), "2FF43");
    assert_eq!(state(face_edges(&t, 77)), "2FF44");
    assert_eq!(state(face_geometry(&t, 0)), "2FF60");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.18, §11.3.19]
/// an empty topology cannot be validated, and one with a face other than
/// the universal face is not empty.
#[test]
fn empty_topology_checks() {
    let t = fresh();
    assert_eq!(state(validate(&t)), "2FF32");
    let mut nonempty = fresh();
    nonempty.faces.insert(3_i32, Face { mbr: None });
    assert_eq!(state(create(&mut nonempty, &pt(1.0, 1.0))), "2FF42");
    let mut faceless = Topology::default();
    faceless.faces.insert(3_i32, Face { mbr: None });
    assert_eq!(state(create(&mut faceless, &pt(1.0, 1.0))), "2FF61");
}

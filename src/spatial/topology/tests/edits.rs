//! Tests of removing isolated edges, healing edges met at either end,
//! faces with holes, and validating a topology whose rows contradict it.

use super::super::validate::{COINCIDENT_NODES, EDGE_CROSSED_NODE, EDGE_CROSSES_EDGE, FACE_WITHOUT_EDGES, GEOMETRY_MISMATCH};
use super::*;

/// Three isolated nodes along the x axis at 0, 5 and 10, numbered 1 to 3.
fn three() -> Topology {
    let mut t = fresh();
    for x in [0.0_f64, 5.0_f64, 10.0_f64] {
        t.add_iso_node(None, &pt(x, 0.0_f64)).unwrap();
    }
    t
}

/// An edge to add: its start and end nodes and the x of its two ends.
type Link = (i32, i32, f64, f64);

/// The segment from `(a, 0)` to `(b, 0)`.
fn along(a: f64, b: f64) -> Geom {
    line(&[(a, 0.0_f64), (b, 0.0_f64)])
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.5 Description
/// 2e] an isolated edge is removed and its nodes are isolated again; an
/// edge between two faces, or one another edge meets at either node, is
/// not isolated (2FF50), and an edge that is not there is 2FF43.
#[test]
fn removing_isolated_edges() {
    let mut t = three();
    let e = t.add_iso_edge(1, 2, &along(0.0_f64, 5.0_f64)).unwrap();
    t.remove_iso_edge(e).unwrap();
    assert!(t.edges.is_empty());
    assert_eq!(t.nodes[&1_i32].containing_face, Some(UNIVERSAL_FACE));
    let first = t.add_iso_edge(1, 2, &along(0.0_f64, 5.0_f64)).unwrap();
    t.add_edge(2, 3, &along(5.0_f64, 10.0_f64), Faces::Modify).unwrap();
    assert_eq!(state(t.remove_iso_edge(first)), "2FF50");
    let mut other = three();
    let last = other.add_iso_edge(2, 3, &along(5.0_f64, 10.0_f64)).unwrap();
    other.add_edge(1, 2, &along(0.0_f64, 5.0_f64), Faces::Modify).unwrap();
    assert_eq!(state(other.remove_iso_edge(last)), "2FF50");
    let mut s = square();
    assert_eq!(state(s.remove_iso_edge(1)), "2FF50");
    assert_eq!(state(s.remove_iso_edge(99)), "2FF43");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.10, §11.3.11
/// Description 2f] edges meeting end to end, end to end reversed, start
/// to start and start to end heal into one curve through their common
/// node.
#[test]
fn healing_at_either_end() {
    let cases: [[Link; 2]; 3] =
        [[(1, 2, 0.0, 5.0), (3, 2, 10.0, 5.0)], [(2, 1, 5.0, 0.0), (2, 3, 5.0, 10.0)], [(2, 3, 5.0, 10.0), (1, 2, 0.0, 5.0)]];
    for [(s1, e1, a1, b1), (s2, e2, a2, b2)] in cases {
        for new_edge in [false, true] {
            let mut t = three();
            let one = t.add_iso_edge(s1, e1, &along(a1, b1)).unwrap();
            let two = t.add_edge(s2, e2, &along(a2, b2), Faces::Modify).unwrap();
            t.heal(one, two, new_edge).unwrap();
            assert_eq!(t.edges.len(), 1);
            let healed = t.edges.values().next().unwrap();
            let mut xs: Vec<f64> = line_of(&healed.geometry).unwrap().iter().map(|p| p[0]).collect();
            if xs.first() > xs.last() {
                xs.reverse();
            }
            assert_eq!(xs, [0.0_f64, 5.0_f64, 10.0_f64]);
            assert!(!t.nodes.contains_key(&2_i32), "the common node goes");
            consistent(&t);
        }
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.11 Description
/// 2f v] curves of different coordinate dimensions do not heal (2FF25).
#[test]
fn healing_mixed_dimensions() {
    let mut t = three();
    let one = t.add_iso_edge(1, 2, &along(0.0_f64, 5.0_f64)).unwrap();
    let raised =
        Geom::new(0, Dims::Xyz, Body::Curve(Curve::LineString(vec![5.0_f64, 0.0_f64, 1.0_f64, 10.0_f64, 0.0_f64, 1.0_f64]))).unwrap();
    let two = t.add_edge(2, 3, &raised, Faces::Modify).unwrap();
    assert_eq!(state(t.heal(one, two, false)), "2FF25");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.1, §11.3.4
/// Description 2e v, §11.3.9] a ring within a face makes a hole of it, a
/// node in the hole is in the inner face and one between in the outer;
/// an isolated edge may close on its node, nodes of different faces take
/// no isolated edge (2FF39), and a point of other dimensions splits an
/// edge with the rest interpolated.
#[test]
fn holes_loops_and_splits() {
    let mut t = square();
    let corners = [(3.0_f64, 3.0_f64), (7.0_f64, 3.0_f64), (7.0_f64, 7.0_f64), (3.0_f64, 7.0_f64)];
    let inner: Vec<i32> = corners.iter().map(|(x, y)| t.add_iso_node(None, &pt(*x, *y)).unwrap()).collect();
    for (k, (from, to)) in inner.iter().zip(inner.iter().cycle().skip(1)).enumerate() {
        let (a, b) = (corners[k], corners[(k + 1) % 4]);
        t.add_edge(*from, *to, &line(&[a, b]), Faces::Modify).unwrap();
    }
    assert_eq!(t.faces.len(), 3);
    let middle = t.add_iso_node(None, &pt(5.0_f64, 5.0_f64)).unwrap();
    let between = t.add_iso_node(None, &pt(1.0_f64, 1.0_f64)).unwrap();
    assert_ne!(t.nodes[&middle].containing_face, t.nodes[&between].containing_face);
    assert_eq!(t.nodes[&between].containing_face, Some(1_i32));
    assert_eq!(state(t.add_iso_edge(middle, between, &line(&[(5.0_f64, 5.0_f64), (1.0_f64, 1.0_f64)]))), "2FF39");
    let ring = line(&[(1.0_f64, 1.0_f64), (2.0_f64, 1.0_f64), (2.0_f64, 2.0_f64), (1.0_f64, 1.0_f64)]);
    t.add_iso_edge(between, between, &ring).unwrap();
    assert_eq!(t.faces.len(), 4);
    let raised = Geom::new(0, Dims::Xyz, Body::Point(vec![5.0_f64, 0.0_f64, 7.0_f64])).unwrap();
    t.split_edge(1, &raised, false).unwrap();
    consistent(&t);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.6 Description
/// 2e] removing an edge whose left face is the universal face keeps the
/// universal face; removing a dangling edge keeps its one face.
#[test]
fn removing_edges_of_the_universal_face() {
    let mut t = fresh();
    for (x, y) in [(0.0_f64, 0.0_f64), (10.0_f64, 0.0_f64), (10.0_f64, 10.0_f64), (0.0_f64, 10.0_f64)] {
        t.add_iso_node(None, &pt(x, y)).unwrap();
    }
    t.add_edge(1, 4, &line(&[(0.0_f64, 0.0_f64), (0.0_f64, 10.0_f64)]), Faces::Modify).unwrap();
    t.add_edge(4, 3, &line(&[(0.0_f64, 10.0_f64), (10.0_f64, 10.0_f64)]), Faces::Modify).unwrap();
    t.add_edge(3, 2, &line(&[(10.0_f64, 10.0_f64), (10.0_f64, 0.0_f64)]), Faces::Modify).unwrap();
    t.add_edge(2, 1, &line(&[(10.0_f64, 0.0_f64), (0.0_f64, 0.0_f64)]), Faces::Modify).unwrap();
    assert_eq!(t.faces.len(), 2);
    assert_eq!(t.edges[&1_i32].left_face, UNIVERSAL_FACE);
    assert_eq!(t.remove_edge(1, true).unwrap(), None);
    assert_eq!(t.faces.len(), 1);
    assert_eq!(t.remove_edge(2, false).unwrap(), None);
    consistent(&t);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.19 Description
/// 2d ii 1, 2, 4, 6, 8] rows contradicting the topology are reported:
/// coincident nodes, a node on an edge, crossing edges, an edge whose
/// nodes are not its ends, and a face no edge bounds.
#[test]
fn validating_contradicting_rows() {
    let mut t = square();
    t.nodes.insert(9_i32, Node { geometry: pt(0.0_f64, 0.0_f64), containing_face: None });
    t.nodes.insert(10_i32, Node { geometry: pt(5.0_f64, 0.0_f64), containing_face: None });
    let crossing = Edge {
        start: 30,
        end: 31,
        next_left: -9,
        next_right: 9,
        left_face: UNIVERSAL_FACE,
        right_face: UNIVERSAL_FACE,
        geometry: line(&[(5.0_f64, -5.0_f64), (5.0_f64, 5.0_f64)]),
    };
    t.edges.insert(9_i32, crossing);
    t.faces.insert(7_i32, Face { mbr: None });
    let found: Vec<&str> = validate(&t).unwrap().iter().map(|p| p.error).collect();
    for error in [COINCIDENT_NODES, EDGE_CROSSED_NODE, EDGE_CROSSES_EDGE, GEOMETRY_MISMATCH, FACE_WITHOUT_EDGES] {
        assert!(found.contains(&error), "{error} in {found:?}");
    }
}

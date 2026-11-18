//! Unit tests of topology-networks [ISO/IEC 13249-3:2016 (DIS, SC32
//! N2593) §12.3], on networks held in memory.

use super::heal::Heal;
use super::split::Split;
use super::validate::{END_MISMATCH, MISSING_LINK_GEOMETRY, MISSING_NODE_GEOMETRY, MIXED_SRIDS, START_MISMATCH};
use super::{Change, Link, Network, Problem, logical_problems, spatial_problems};
use crate::error::SqlState;
use crate::geometry::model::{Body, Collection, Curve, Dims, Geom, Surface};

/// The point (x, y) of SRID 0.
fn pt(x: f64, y: f64) -> Geom {
    Geom::new(0, Dims::Xy, Body::Point(vec![x, y])).unwrap()
}

/// The linestring through `points`, of SRID 0.
fn line(points: &[f64]) -> Geom {
    Geom::new(0, Dims::Xy, Body::Curve(Curve::LineString(points.to_vec()))).unwrap()
}

/// The points of linestring `g`.
fn points(g: &Geom) -> Vec<f64> {
    let Body::Curve(Curve::LineString(c)) = &g.body else { panic!("{g:?} is no linestring") };
    c.clone()
}

/// Nodes 1 to 3 at (0 0), (10 0) and (10 10); link 1 from node 1 to 2
/// and link 2 from node 2 to 3.
fn corner() -> Network {
    let mut net = Network::default();
    for (x, y) in [(0.0_f64, 0.0_f64), (10.0_f64, 0.0_f64), (10.0_f64, 10.0_f64)] {
        net.add_iso_node(Some(pt(x, y))).unwrap();
    }
    assert_eq!(net.add_link((1_i32, 2_i32), Some(line(&[0.0_f64, 0.0_f64, 10.0_f64, 0.0_f64]))).unwrap(), 1_i32);
    assert_eq!(net.add_link((2_i32, 3_i32), Some(line(&[10.0_f64, 0.0_f64, 10.0_f64, 10.0_f64]))).unwrap(), 2_i32);
    net
}

/// The SQLSTATE of `r`'s error.
fn state<T: std::fmt::Debug>(r: crate::error::Result<T>) -> SqlState {
    r.unwrap_err().sqlstate()
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12.3.4 Description
/// 2f] a link's curve must start at its start node's point and end at its
/// end node's, checked only of a node that has one; a node that is not
/// there is 2FF46.
#[test]
fn a_link_meets_its_nodes() {
    let mut net = corner();
    assert_eq!(
        state(net.add_link((1_i32, 3_i32), Some(line(&[0.0_f64, 1.0_f64, 10.0_f64, 10.0_f64])))),
        SqlState::SPATIAL_START_NODE_NOT_GEOMETRY_START
    );
    assert_eq!(
        state(net.add_link((1_i32, 3_i32), Some(line(&[0.0_f64, 0.0_f64, 10.0_f64, 11.0_f64])))),
        SqlState::SPATIAL_END_NODE_NOT_GEOMETRY_END
    );
    assert_eq!(state(net.add_link((1_i32, 9_i32), None)), SqlState::SPATIAL_NON_EXISTENT_NODE);
    assert_eq!(state(net.add_link((9_i32, 1_i32), None)), SqlState::SPATIAL_NON_EXISTENT_NODE);
    let free = net.add_iso_node(None).unwrap();
    assert_eq!(net.add_link((free, 1_i32), Some(line(&[5.0_f64, 5.0_f64, 0.0_f64, 0.0_f64]))).unwrap(), 3_i32);
    assert_eq!(net.add_link((1_i32, free), None).unwrap(), 4_i32);
    assert_eq!(net.links.get(&4_i32), Some(&Link { start: 1, end: free, geometry: None }));
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12.2.2 Description
/// 5, §12.3.1 Description 3] the geometries of a network are of one
/// system; the one geometry replaced does not decide it.
#[test]
fn geometries_are_of_one_system() {
    let mut net = corner();
    let other = pt(5.0, 5.0).with_srid(4326);
    assert_eq!(state(net.add_iso_node(Some(other.clone()))), SqlState::SPATIAL_MIXED_SPATIAL_REFERENCE_SYSTEMS);
    let mut lone = Network::default();
    let id = lone.add_iso_node(Some(pt(1.0, 1.0))).unwrap();
    lone.move_iso_node(id, Some(other.clone())).unwrap();
    assert_eq!(lone.nodes.get(&id), Some(&Some(other)));
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12.3.2, §12.3.3,
/// §12.3.5, §12.3.6] a node a link ends at is not moved or removed; a
/// link's curve is changed only to one between its nodes, which must have
/// points; a link is removed, its nodes kept.
#[test]
fn moves_changes_and_removals() {
    let mut net = corner();
    assert_eq!(state(net.move_iso_node(2, None)), SqlState::SPATIAL_NOT_ISOLATED_NODE);
    assert_eq!(state(net.remove_iso_node(3)), SqlState::SPATIAL_NOT_ISOLATED_NODE);
    net.change_link_geom(1, Some(line(&[0.0_f64, 0.0_f64, 5.0_f64, -1.0_f64, 10.0_f64, 0.0_f64]))).unwrap();
    assert_eq!(
        state(net.change_link_geom(1, Some(line(&[1.0_f64, 0.0_f64, 10.0_f64, 0.0_f64])))),
        SqlState::SPATIAL_START_NODE_NOT_GEOMETRY_START
    );
    assert_eq!(
        state(net.change_link_geom(1, Some(line(&[0.0_f64, 0.0_f64, 9.0_f64, 0.0_f64])))),
        SqlState::SPATIAL_END_NODE_NOT_GEOMETRY_END
    );
    assert_eq!(state(net.change_link_geom(9, Some(line(&[0.0_f64, 0.0_f64, 9.0_f64, 0.0_f64])))), SqlState::SPATIAL_NULL_NODE_GEOMETRY);
    net.nodes.insert(2_i32, None);
    assert_eq!(state(net.change_link_geom(1, Some(line(&[0.0_f64, 0.0_f64, 10.0_f64, 0.0_f64])))), SqlState::SPATIAL_NULL_NODE_GEOMETRY);
    net.change_link_geom(1, None).unwrap();
    assert_eq!(net.links[&1_i32].geometry, None);
    net.remove_link(2);
    net.remove_iso_node(3).unwrap();
    assert_eq!(net.nodes.keys().copied().collect::<Vec<_>>(), [1_i32, 2_i32]);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12.3.10
/// Description 2f] a point at an end of the link or off it is 2FF55; a
/// link that is not there is 2FF45.
#[test]
fn spatial_splits_refused() {
    let mut net = corner();
    net.change_link_geom(1, Some(line(&[0.0_f64, 0.0_f64, 5.0_f64, 0.0_f64, 10.0_f64, 0.0_f64]))).unwrap();
    for (x, y) in [(0.0_f64, 0.0_f64), (10.0_f64, 0.0_f64), (4.0_f64, 1.0_f64), (11.0_f64, 0.0_f64)] {
        assert_eq!(state(net.split_spatial(1, &pt(x, y), Split::New)), SqlState::SPATIAL_POINT_NOT_ON_LINK, "{x} {y}");
    }
    assert_eq!(state(net.split_spatial(9, &pt(4.0, 0.0), Split::New)), SqlState::SPATIAL_NON_EXISTENT_LINK);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12.3.10, §12.3.11]
/// a spatial split cuts the curve at the point, which becomes a vertex of
/// both parts, at an inner vertex too; a link of no geometry is 2FF38;
/// new links take new identifiers, never the split link's.
#[test]
fn spatial_splits() {
    let mut net = corner();
    net.change_link_geom(1, Some(line(&[0.0_f64, 0.0_f64, 5.0_f64, 0.0_f64, 10.0_f64, 0.0_f64]))).unwrap();
    assert_eq!(net.split_spatial(1, &pt(5.0, 0.0), Split::New).unwrap(), 4_i32);
    assert_eq!(net.links.get(&1_i32), None);
    let (l3, l4) = (&net.links[&3_i32], &net.links[&4_i32]);
    assert_eq!((l3.start, l3.end, l4.start, l4.end), (1_i32, 4_i32, 4_i32, 2_i32));
    assert_eq!(points(l3.geometry.as_ref().unwrap()), [0.0_f64, 0.0_f64, 5.0_f64, 0.0_f64]);
    assert_eq!(points(l4.geometry.as_ref().unwrap()), [5.0_f64, 0.0_f64, 10.0_f64, 0.0_f64]);
    assert_eq!(net.split_spatial(2, &pt(10.0, 2.5), Split::Modify).unwrap(), 5_i32);
    let (l2, l5) = (&net.links[&2_i32], &net.links[&5_i32]);
    assert_eq!((l2.start, l2.end, l5.start, l5.end), (2_i32, 5_i32, 5_i32, 3_i32));
    assert_eq!(points(l2.geometry.as_ref().unwrap()), [10.0_f64, 0.0_f64, 10.0_f64, 2.5_f64]);
    assert_eq!(points(l5.geometry.as_ref().unwrap()), [10.0_f64, 2.5_f64, 10.0_f64, 10.0_f64]);
    net.change_link_geom(5, None).unwrap();
    assert_eq!(state(net.split_spatial(5, &pt(10.0, 5.0), Split::New)), SqlState::SPATIAL_LINK_HAS_NULL_GEOMETRY);
    assert_eq!(spatial_problems(&net).unwrap(), [Problem { error: MISSING_LINK_GEOMETRY, primitive1: Some(5_i32), primitive2: None }]);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12.3.10 Description
/// 2f iv 6] a split of a curve with z keeps z, interpolated at a point of
/// x and y alone.
#[test]
fn a_split_interpolates_z() {
    let mut net = Network::default();
    let a = net.add_iso_node(None).unwrap();
    let b = net.add_iso_node(None).unwrap();
    let g = Geom::new(0, Dims::Xyz, Body::Curve(Curve::LineString(vec![0.0_f64, 0.0_f64, 0.0_f64, 4.0_f64, 0.0_f64, 8.0_f64]))).unwrap();
    net.add_link((a, b), Some(g)).unwrap();
    net.split_spatial(1, &pt(1.0, 0.0), Split::Modify).unwrap();
    assert_eq!(points(net.links[&1_i32].geometry.as_ref().unwrap()), [0.0_f64, 0.0_f64, 0.0_f64, 1.0_f64, 0.0_f64, 2.0_f64]);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12.3.8, §12.3.9]
/// a logical split; a link with geometry is not logical (2FF49).
#[test]
fn logical_splits() {
    let mut net = corner();
    assert_eq!(state(net.split_logical(1, Split::New)), SqlState::SPATIAL_NOT_A_LOGICAL_LINK);
    assert_eq!(state(net.split_logical(7, Split::Modify)), SqlState::SPATIAL_NON_EXISTENT_LINK);
    net.change_link_geom(1, None).unwrap();
    assert_eq!(net.split_logical(1, Split::Modify).unwrap(), 4_i32);
    assert_eq!(net.links.get(&1_i32), Some(&Link { start: 1, end: 4, geometry: None }));
    assert_eq!(net.links.get(&3_i32), Some(&Link { start: 4, end: 2, geometry: None }));
    assert_eq!(net.nodes.get(&4_i32), Some(&None));
}

/// Two links of a network of three nodes, as the cases of §12.3.12
/// Description 2f iii 5 need: link 1 between nodes 1 and 2, from 2 if
/// `first_from_common`; link 2 between nodes 2 and 3, from 2 if
/// `second_from_common`.
fn pair((first_from_common, second_from_common): (bool, bool)) -> Network {
    let mut net = Network::default();
    for (x, y) in [(0.0_f64, 0.0_f64), (5.0_f64, 0.0_f64), (5.0_f64, 5.0_f64)] {
        net.add_iso_node(Some(pt(x, y))).unwrap();
    }
    let first = if first_from_common {
        ((2_i32, 1_i32), [5.0_f64, 0.0_f64, 0.0_f64, 0.0_f64])
    } else {
        ((1_i32, 2_i32), [0.0_f64, 0.0_f64, 5.0_f64, 0.0_f64])
    };
    let second = if second_from_common {
        ((2_i32, 3_i32), [5.0_f64, 0.0_f64, 5.0_f64, 5.0_f64])
    } else {
        ((3_i32, 2_i32), [5.0_f64, 5.0_f64, 5.0_f64, 0.0_f64])
    };
    net.add_link(first.0, Some(line(&first.1))).unwrap();
    net.add_link(second.0, Some(line(&second.1))).unwrap();
    net
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12.3.12, §12.3.13
/// Description 2f v] each of the four ways two links meet heals into one
/// link in the first's direction, its curve joined and the second's
/// reversed where needed; the common node goes.
#[test]
fn heals_of_each_case() {
    let cases = [
        (false, true, (1_i32, 3_i32), [0.0_f64, 0.0_f64, 5.0_f64, 0.0_f64, 5.0_f64, 5.0_f64]),
        (false, false, (1_i32, 3_i32), [0.0_f64, 0.0_f64, 5.0_f64, 0.0_f64, 5.0_f64, 5.0_f64]),
        (true, true, (3_i32, 1_i32), [5.0_f64, 5.0_f64, 5.0_f64, 0.0_f64, 0.0_f64, 0.0_f64]),
        (true, false, (3_i32, 1_i32), [5.0_f64, 5.0_f64, 5.0_f64, 0.0_f64, 0.0_f64, 0.0_f64]),
    ];
    for (first_from_common, second_from_common, ends, coords) in cases {
        for how in [Heal::New, Heal::Modify] {
            let case = format!("{first_from_common} {second_from_common} {how:?}");
            let mut net = pair((first_from_common, second_from_common));
            let made = net.heal((1_i32, 2_i32), how).unwrap();
            assert_eq!(made, (how == Heal::New).then_some(3_i32), "{case}");
            let link = &net.links[&made.unwrap_or(1_i32)];
            assert_eq!((link.start, link.end), ends, "{case}");
            assert_eq!(points(link.geometry.as_ref().unwrap()), coords, "{case}");
            assert_eq!(net.links.len(), 1, "{case}");
            assert_eq!(net.nodes.keys().copied().collect::<Vec<_>>(), [1_i32, 3_i32], "{case}");
        }
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12.3.12 Description
/// 2f] links that share no node (2FF41), a node another link ends at
/// (2FF53), and a link with none heal to a link of none.
#[test]
fn heals_refused_and_logical() {
    let mut net = corner();
    net.add_iso_node(None).unwrap();
    net.add_link((3_i32, 4_i32), None).unwrap();
    assert_eq!(state(net.heal((1_i32, 3_i32), Heal::New)), SqlState::SPATIAL_NON_CONNECTED_LINKS);
    assert_eq!(state(net.heal((2_i32, 2_i32), Heal::New)), SqlState::SPATIAL_NON_CONNECTED_LINKS);
    assert_eq!(state(net.heal((1_i32, 9_i32), Heal::Modify)), SqlState::SPATIAL_NON_EXISTENT_LINK);
    net.add_link((2_i32, 1_i32), None).unwrap();
    assert_eq!(state(net.heal((1_i32, 2_i32), Heal::New)), SqlState::SPATIAL_OTHER_LINKS_CONNECTED);
    net.remove_link(4);
    assert_eq!(net.heal((2_i32, 3_i32), Heal::New).unwrap(), Some(4_i32));
    assert_eq!(net.links.get(&4_i32), Some(&Link { start: 2, end: 4, geometry: None }));
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12.3.16] a
/// collection's curves are links between nodes at their ends, shared
/// where ends meet; points are nodes; a surface is 2FF02.
#[test]
fn a_network_of_a_collection() {
    let lines = vec![vec![0.0_f64, 0.0_f64, 1.0_f64, 1.0_f64], vec![1.0_f64, 1.0_f64, 2.0_f64, 0.0_f64]];
    let body = Body::Collection(Collection::Geometry(vec![
        Body::Collection(Collection::MultiLineString(lines)),
        Body::Point(vec![2.0_f64, 0.0_f64]),
        Body::Point(vec![]),
        Body::Point(vec![7.0_f64, 7.0_f64]),
    ]));
    let mut net = Network::default();
    net.add_geometry(&Geom::new(0, Dims::Xy, body).unwrap()).unwrap();
    assert_eq!(net.nodes.len(), 4);
    assert_eq!(net.nodes.get(&4_i32), Some(&Some(pt(7.0, 7.0))));
    let ends: Vec<(i32, i32)> = net.links.values().map(|l| (l.start, l.end)).collect();
    assert_eq!(ends, [(1_i32, 2_i32), (2_i32, 3_i32)]);
    let ring = vec![0.0_f64, 0.0_f64, 1.0_f64, 0.0_f64, 1.0_f64, 1.0_f64, 0.0_f64, 0.0_f64];
    let area = Body::Collection(Collection::Geometry(vec![Body::Surface(Surface::Polygon(vec![ring]))]));
    let refused = Network::default().add_geometry(&Geom::new(0, Dims::Xy, area).unwrap());
    assert_eq!(state(refused), SqlState::SPATIAL_INVALID_ARGUMENT);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12.3.17, §12.3.18]
/// the checks of a network: empty is 2FF31; each kind of row, in the
/// Description's order.
#[test]
fn checks_of_a_network() {
    let empty = Network::default();
    assert_eq!(state(logical_problems(&empty)), SqlState::SPATIAL_EMPTY_NETWORK);
    assert_eq!(state(spatial_problems(&empty)), SqlState::SPATIAL_EMPTY_NETWORK);
    let mut net = corner();
    assert_eq!(spatial_problems(&net).unwrap(), []);
    assert_eq!(logical_problems(&net).unwrap().len(), 5);
    net.nodes.insert(1_i32, Some(pt(0.0, 1.0)));
    net.nodes.insert(3_i32, Some(pt(10.0, 10.0).with_srid(4326)));
    net.nodes.insert(4_i32, None);
    net.links.insert(3_i32, Link { start: 4, end: 4, geometry: None });
    let p = |error, primitive1, primitive2| Problem { error, primitive1, primitive2 };
    assert_eq!(
        spatial_problems(&net).unwrap(),
        [
            p(MISSING_NODE_GEOMETRY, Some(4_i32), None),
            p(MISSING_LINK_GEOMETRY, Some(3_i32), None),
            p(START_MISMATCH, Some(1_i32), Some(1_i32)),
            p(END_MISMATCH, Some(2_i32), Some(3_i32)),
            p(MIXED_SRIDS, None, None),
        ]
    );
    let logical = Network { nodes: [(1_i32, None)].into(), links: [(1_i32, Link { start: 1, end: 1, geometry: None })].into() };
    assert_eq!(logical_problems(&logical).unwrap(), []);
}

/// Verifies: [DESIGN] the rows a routine changed: inserted, updated and
/// deleted, of both tables, deletions first.
#[test]
fn changes_of_a_routine() {
    let before = corner();
    let mut after = before.clone();
    after.split_spatial(1, &pt(4.0, 0.0), Split::New).unwrap();
    let changes = before.changes(&after);
    assert_eq!(changes.nodes.len(), 1);
    assert_eq!(changes.links.len(), 3);
    assert!(matches!(changes.links.first(), Some(Change::Delete(1_i32))));
}

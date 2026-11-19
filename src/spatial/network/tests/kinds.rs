//! A network made of each kind of collection, and links of mismatched or
//! damaged curves.

use super::*;

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12.3.16 Description
/// 2h iii] a multipoint's points are nodes and a multicurve's curves
/// links; a multisurface or multipolygon is 2FF02.
#[test]
fn networks_of_each_collection() {
    let points = Body::Collection(Collection::MultiPoint(vec![vec![0.0_f64, 0.0_f64], vec![3.0_f64, 4.0_f64]]));
    let mut dots = Network::default();
    dots.add_geometry(&Geom::new(0, Dims::Xy, points).unwrap()).unwrap();
    assert_eq!((dots.nodes.len(), dots.links.len()), (2, 0));
    let curves = Body::Collection(Collection::MultiCurve(vec![Curve::LineString(vec![0.0_f64, 0.0_f64, 1.0_f64, 1.0_f64])]));
    let mut path = Network::default();
    path.add_geometry(&Geom::new(0, Dims::Xy, curves).unwrap()).unwrap();
    assert_eq!((path.nodes.len(), path.links.len()), (2, 1));
    let ring = vec![0.0_f64, 0.0_f64, 1.0_f64, 0.0_f64, 1.0_f64, 1.0_f64, 0.0_f64, 0.0_f64];
    for area in [
        Body::Collection(Collection::MultiSurface(vec![Surface::Polygon(vec![ring.clone()])])),
        Body::Collection(Collection::MultiPolygon(vec![vec![ring.clone()]])),
    ] {
        assert_eq!(state(Network::default().add_geometry(&Geom::new(0, Dims::Xy, area).unwrap())), SqlState::SPATIAL_INVALID_ARGUMENT);
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12.3.4 Description
/// 2f, §12.3.15 Description 2f v] an empty curve does not start at its
/// node (2FF57); links of curves of different coordinate dimensions do
/// not heal (2FF25).
#[test]
fn mismatched_curves() {
    let mut net = corner();
    let empty = Geom::new(0, Dims::Xy, Body::Curve(Curve::LineString(Vec::new()))).unwrap();
    assert_eq!(state(net.add_link((1_i32, 3_i32), Some(empty))), SqlState::SPATIAL_START_NODE_NOT_GEOMETRY_START);
    let mut mixed = Network::default();
    for (x, y) in [(0.0_f64, 0.0_f64), (10.0_f64, 0.0_f64), (10.0_f64, 10.0_f64)] {
        mixed.add_iso_node(Some(pt(x, y))).unwrap();
    }
    mixed.add_link((1_i32, 2_i32), Some(line(&[0.0_f64, 0.0_f64, 10.0_f64, 0.0_f64]))).unwrap();
    let raised =
        Geom::new(0, Dims::Xyz, Body::Curve(Curve::LineString(vec![10.0_f64, 0.0_f64, 1.0_f64, 10.0_f64, 10.0_f64, 1.0_f64]))).unwrap();
    mixed.links.insert(2_i32, Link { start: 2, end: 3, geometry: Some(raised) });
    assert_eq!(state(mixed.heal((1_i32, 2_i32), Heal::New)), SqlState::SPATIAL_MIXED_COORDINATE_DIMENSIONS);
}

/// Verifies: [DESIGN] a link whose stored geometry is no curve is a
/// damaged network (SQLCODE -916) when it is split.
#[test]
fn a_damaged_link() {
    let mut net = corner();
    net.links.insert(9_i32, Link { start: 1, end: 2, geometry: Some(pt(5.0, 0.0)) });
    assert_eq!(net.split_spatial(9_i32, &pt(5.0, 0.0), Split::New).unwrap_err().sqlcode(), -916_i32);
}

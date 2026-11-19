//! Tests of the paths along a curve and of the places they give
//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.4.4, §15.4.5, §15.9.1,
//! §15.10.1]: ST_LRPosition in each type of LRM and with vertical
//! offsets, the exceptions of locating a position, and the paths of a
//! geometry an offset is measured from.

use super::measure::{Path, Vertex, paths_of, point_geom, point_vertex};
use super::*;
use crate::spatial::formats::wkt::read_geom;

/// An LRM `id` of type `lrm_type` in metres, its offsets in metres
/// positive upwards or, of `down`, downwards.
fn vertical_lrm(id: i32, lrm_type: &str, down: bool) -> Lrm {
    let parts = LrmParts {
        id: Some(id),
        name: Some(format!("lrm {id}")),
        lrm_type: Some(lrm_type.to_owned()),
        units: Some(METRE.to_owned()),
        constraints: None,
    };
    let vertical = Some(if down { DOWN } else { UP }.to_owned());
    let offsets = OffsetParts { units: Some(METRE.to_owned()), lateral: Some(RIGHT.to_owned()), vertical };
    Lrm::new_with_offsets(parts, offsets, &units()).unwrap()
}

/// The LRMs of the samples: 1 absolute, 2 interpolative, 3 relative, 4
/// local interpolative, 5 of an unknown type, 6 absolute of offsets up,
/// 7 the same down.
fn lrms() -> Vec<Lrm> {
    vec![
        lrm(1, ABSOLUTE),
        lrm(2, INTERPOLATIVE),
        lrm(3, RELATIVE),
        lrm(4, LOCAL_INTERPOLATIVE),
        lrm(5, "bogus"),
        vertical_lrm(6, ABSOLUTE, false),
        vertical_lrm(7, ABSOLUTE, true),
    ]
}

/// The point (x, y, z) of SRID 0.
fn point_z(x: f64, y: f64, z: f64) -> Geom {
    Geom::new(0, Dims::Xyz, Body::Point(vec![x, y, z])).unwrap()
}

/// The distance along of a position expression.
fn along(p: &PositionExp) -> f64 {
    p.distance_exp().distance_along().measure()
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.4.5 Description
/// 3] ST_LRPosition of (30 4) along the road, in each LRM type: 30 m
/// absolute and relative, 30 of the default measure 100 interpolative,
/// the m there local interpolative; an LRM of an unknown type raises
/// 2FF81.
#[test]
fn lr_position_in_each_lrm_type() {
    let all = units();
    let lrms = lrms();
    let cat = Catalog { units: &all, lrms: &lrms, elements: &[], metres_per_unit: 1.0_f64 };
    let at = point(30.0_f64, 4.0_f64);
    for id in [1_i32, 2_i32, 3_i32] {
        let p = curve(1, id, road().curve).lr_position(Some(&at), &cat).unwrap();
        assert!((along(&p) - 30.0_f64).abs() < 1e-9_f64, "LRM {id}");
    }
    let measured = curve(1, 4, line(&[0.0_f64, 0.0_f64, 10.0_f64, 100.0_f64, 0.0_f64, 20.0_f64], Dims::Xym));
    assert!((along(&measured.lr_position(Some(&at), &cat).unwrap()) - 13.0_f64).abs() < 1e-9_f64);
    assert_eq!(state(curve(1, 5, road().curve).lr_position(Some(&at), &cat)), SqlState::SPATIAL_INVALID_LRM);
    let p = PositionExp::of_ids(1, 5, DistanceExp::along(m(1.0_f64)));
    assert_eq!(state(road().point(Some(&p), &cat)), SqlState::SPATIAL_INVALID_LRM);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.4.5 Description
/// 4] a point above a curve of z is given a vertical offset, positive
/// upwards or downwards as the LRM says; one beside it a lateral offset
/// too.
#[test]
fn lr_position_gives_vertical_offsets() {
    let all = units();
    let lrms = lrms();
    let cat = Catalog { units: &all, lrms: &lrms, elements: &[], metres_per_unit: 1.0_f64 };
    let flat = line(&[0.0_f64, 0.0_f64, 0.0_f64, 100.0_f64, 0.0_f64, 0.0_f64], Dims::Xyz);
    let vertical = |p: &PositionExp| p.distance_exp().ver_offset_exp().and_then(VerOffsetExp::offset_ver_dist).map(LrMeasure::measure);
    let up = curve(1, 6, flat.clone()).lr_position(Some(&point_z(30.0_f64, 0.0_f64, 2.0_f64)), &cat).unwrap();
    assert_eq!(vertical(&up), Some(2.0_f64));
    assert!(up.distance_exp().lat_offset_exp().is_none());
    let down = curve(1, 7, flat).lr_position(Some(&point_z(30.0_f64, 4.0_f64, 2.0_f64)), &cat).unwrap();
    assert_eq!(vertical(&down), Some(-2.0_f64));
    assert!(down.distance_exp().lat_offset_exp().is_some());
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.4.4, §15.9.1
/// Description 15, §15.10.1] locating raises 2FF02 of a position along a
/// feature, not a curve, and of a referent of no position and no
/// location; 2FF14 of a unit the catalog does not hold; 2FF13 of a
/// coordinate unit of no length.
#[test]
fn locating_exceptions() {
    let all = units();
    let lrms = lrms();
    let cat = Catalog { units: &all, lrms: &lrms, elements: &[], metres_per_unit: 1.0_f64 };
    let base = Catalog::of_units(&all);
    let lost = Referent::new(Some("lost".to_owned()), Some("post".to_owned()), None, None, &base).unwrap();
    let feature = LrFeature::new_with_referents(parts(9, 3), Some("F1".to_owned()), Some(vec![lost])).unwrap();
    let on_feature = PositionExp::of_element_and_id(LinearElement::Feature(feature.clone()), 1, DistanceExp::along(m(1.0_f64)));
    assert_eq!(state(locate::locate(&on_feature, &cat, 0)), SqlState::SPATIAL_INVALID_ARGUMENT);
    let elements = [LinearElement::Feature(feature)];
    let with_feature = Catalog { elements: &elements, ..cat };
    let names = ReferentNames { from_name: Some("lost".to_owned()), ..ReferentNames::default() };
    let from_lost = PositionExp::of_ids(1, 3, DistanceExp::new(Some(m(1.0_f64)), names, None, None).unwrap());
    assert_eq!(state(road().point(Some(&from_lost), &with_feature)), SqlState::SPATIAL_INVALID_ARGUMENT);
    let p = PositionExp::of_ids(1, 1, DistanceExp::along(m(25.0_f64)));
    let no_units = Catalog { units: &[], ..cat };
    assert_eq!(state(road().point(Some(&p), &no_units)), SqlState::SPATIAL_UNSUPPORTED_UNIT_SPECIFIED);
    let no_length = Catalog { metres_per_unit: 0.0_f64, ..cat };
    assert_eq!(state(road().point(Some(&p), &no_length)), SqlState::SPATIAL_ATTEMPTED_DIVISION_BY_ZERO);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.10.2 Description
/// 8b] a referent whose position is of another spatial reference system
/// than its location is refused (2FF83).
#[test]
fn referent_of_mixed_reference_systems() {
    let all = units();
    let lrms = lrms();
    let elements = [LinearElement::Curve(road())];
    let cat = Catalog { units: &all, lrms: &lrms, elements: &elements, metres_per_unit: 1.0_f64 };
    let at25 = PositionExp::of_ids(1, 1, DistanceExp::along(m(25.0_f64)));
    let elsewhere = point(25.0_f64, 0.0_f64).with_srid(4326);
    let r = Referent::new(Some("r".to_owned()), Some("post".to_owned()), Some(elsewhere), Some(at25), &cat);
    assert_eq!(state(r), SqlState::SPATIAL_INCOMPATIBLE_REFERENT_POSITION);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.9.1 Description
/// 18, 25] a path of one vertex gives that vertex at any distance and as
/// the foot of any point; along a segment of one m, that m is at its
/// start; an empty curve has no path (2FF02).
#[test]
fn paths_of_one_vertex_and_flat_measures() {
    let one = Path::of_coords(&[3.0_f64, 4.0_f64], Dims::Xy).unwrap();
    let station = one.at(5.0_f64);
    assert!(near(&[station.s, station.at.x, station.at.y], &[0.0_f64, 3.0_f64, 4.0_f64]));
    let foot = one.foot(0.0_f64, 0.0_f64);
    assert!((foot.distance - 5.0_f64).abs() < 1e-9_f64);
    let flat = Path::of_coords(&[0.0_f64, 0.0_f64, 5.0_f64, 10.0_f64, 0.0_f64, 5.0_f64], Dims::Xym).unwrap();
    assert_eq!(flat.s_at_m(5.0_f64), Some(0.0_f64));
    assert_eq!(flat.s_at_m(6.0_f64), None);
    let empty = read_geom("LINESTRING EMPTY", 0).unwrap();
    assert_eq!(state(Path::of_curve(&empty)), SqlState::SPATIAL_INVALID_ARGUMENT);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.11.1, §15.12.1]
/// the paths an offset's feature geometry gives: one of each point,
/// linestring and ring of a collection, of each kind of collection.
#[test]
fn paths_of_collections() {
    let collection = "GEOMETRYCOLLECTION (POINT (1 1), MULTIPOINT ((2 2), (3 3)), MULTILINESTRING ((0 0, 1 0)), \
                      MULTIPOLYGON (((0 0, 1 0, 1 1, 0 0))), POLYGON ((0 0, 2 0, 2 2, 0 0)))";
    assert_eq!(paths_of(&read_geom(collection, 0).unwrap()).unwrap().len(), 6);
    let curves = "MULTICURVE ((0 0, 1 0), (0 1, 1 1))";
    assert_eq!(paths_of(&read_geom(curves, 0).unwrap()).unwrap().len(), 2);
    let surfaces = "MULTISURFACE (((0 0, 1 0, 1 1, 0 0)), ((5 5, 6 5, 6 6, 5 5)))";
    assert_eq!(paths_of(&read_geom(surfaces, 0).unwrap()).unwrap().len(), 2);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.4.5 Description
/// 1a, 2b] the place of a point value: a geometry no point raises 2FF02,
/// an empty point 2FF17; a place of z and m makes a point of both.
#[test]
fn point_places() {
    assert_eq!(state(point_vertex(&road().curve)), SqlState::SPATIAL_INVALID_ARGUMENT);
    assert_eq!(state(point_vertex(&read_geom("POINT EMPTY", 0).unwrap())), SqlState::SPATIAL_EMPTY_POINT_VALUE);
    let v = Vertex { x: 1.0_f64, y: 2.0_f64, z: Some(3.0_f64), m: Some(4.0_f64) };
    let g = point_geom(v, 0).unwrap();
    assert_eq!(g.dims, Dims::Xyzm);
    assert!(near(&ordinates(&g), &[1.0_f64, 2.0_f64, 3.0_f64, 4.0_f64]));
}

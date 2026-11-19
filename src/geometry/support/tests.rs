//! Unit tests of the types beside the geometry types: their stored
//! forms, keys, representations, routine tables and sizes.

use super::annotation;
use super::types::TYPES;
use super::values::{self, Knot, NurbsPoint, Placement, Vector, angle_value, direction_value, system_value};
use super::vector_formats::{read_binary, read_text, write_binary, write_text};
use super::{Form, SUPPORTS, Support, TRANSFORM_GROUPS, data_type, ops, sizings};
use crate::bytes::Reader;
use crate::spatial::srs::angle::Angle;
use crate::spatial::srs::direction::Direction;
use crate::spatial::srs::system::SpatialRefSys;
use crate::value::{SqlType, Value};

/// The type and bytes of a value of these types.
fn bytes(v: &Value) -> (Support, Vec<u8>) {
    super::of_value(v).map(|(s, b)| (s, b.to_vec())).unwrap()
}

/// A value of each type, not empty.
fn samples() -> Vec<Value> {
    let point =
        crate::geometry::model::Geom::from_value(&crate::geometry::Geometry::from_wkt("POINT (1 2)", 0).unwrap().to_value()).unwrap();
    let vector = Vector { x: Some(1.0_f64), y: Some(2.0_f64), z: Some(3.0_f64), srid: 4326_i32 };
    vec![
        system_value(&SpatialRefSys::from_srid(4326_i32).unwrap()),
        angle_value(Angle::new(1.5_f64)),
        direction_value(Direction::new(2.5_f64).unwrap()),
        vector.value(),
        Placement { location: Some(point.encode()), directions: Some(vec![vector, Vector::EMPTY]) }.value(),
        NurbsPoint { point: Some(point.encode()), weight: Some(0.5_f64) }.value(),
        Knot { value: Some(1.0_f64), multiplicity: Some(2_i32) }.value(),
        element(&point).value_of(),
        annotation::array_value(&[element(&point), annotation::Element::default()]),
        annotation::Text { envelope: Some(point.encode()), elements: Some(vec![element(&point)]) }.value_of(),
    ]
}

/// A text element at `point`, of no leader line.
fn element(point: &crate::geometry::model::Geom) -> annotation::Element {
    annotation::Element { value: Some("Lake".to_owned()), location: Some(point.encode()), leader: None, attributes: Some(String::new()) }
}

/// Verifies: [DESIGN] RULES.md D53, D123: a value of each
/// type survives its stored form and its record field, and stored bytes
/// that are no value of the type are refused.
#[test]
fn values_round_trip_their_stored_forms() {
    for (s, v) in SUPPORTS.into_iter().zip(samples()) {
        let kind = &TYPES[s.at()];
        let mut out = Vec::new();
        kind.put_value(&v, &mut out);
        assert_eq!(bytes(&kind.value(&mut Reader::new(&out, "test")).unwrap()), bytes(&v), "{}", s.name());
        let field = kind.encode_record(&v, &data_type(s)).unwrap();
        assert_eq!(kind.decode_record(&field, &data_type(s)).map(|d| bytes(&d)), Some(bytes(&v)), "{}", s.name());
        assert!(values::check(s, &[0xFF_u8, 1, 2]).is_err(), "{}", s.name());
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §16.1, §16.2] an
/// angle's and a direction's keys order as their radians, and read back
/// as the value; no other type has a key.
#[test]
fn angle_keys_order_by_radians() {
    let angle = &TYPES[Support::Angle.at()];
    let t = data_type(Support::Angle);
    let key = |r: f64| {
        let mut out = Vec::new();
        angle.encode_key(&angle_value(Angle::new(r)), &t, &mut out).unwrap();
        out
    };
    assert!(key(-1.0_f64) < key(0.0_f64) && key(0.0_f64) < key(0.5_f64) && key(0.5_f64) < key(7.0_f64));
    assert_eq!(key(-0.0_f64), key(0.0_f64));
    let k = key(0.5_f64);
    assert_eq!(bytes(&angle.decode_key(&mut k.as_slice(), &t).unwrap()), bytes(&angle_value(Angle::new(0.5_f64))));
    let vector = &TYPES[Support::Vector.at()];
    let v = Vector { x: Some(1.0_f64), y: Some(2.0_f64), z: None, srid: 0_i32 }.value();
    assert!(vector.encode_key(&v, &data_type(Support::Vector), &mut Vec::new()).is_err());
    assert_eq!(angle.compare(&angle_value(Angle::new(1.0_f64)), &angle_value(Angle::new(2.0_f64))), Some(std::cmp::Ordering::Less));
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §17.2.22, §17.2.23]
/// a vector's well-known text and binary round trip, a big-endian binary
/// is read, and a text whose dimension is not its tag's is 2FF25.
#[test]
fn vector_text_and_binary_round_trip() {
    let v = Vector { x: Some(1.0_f64), y: Some(-2.5_f64), z: Some(3.0_f64), srid: 0_i32 };
    assert_eq!(write_text(&v).unwrap(), "VECTOR Z (1 -2.5 3)");
    assert_eq!(read_text("VECTOR Z (1 -2.5 3)", 0_i32).unwrap(), v);
    assert_eq!(read_binary(&write_binary(&v).unwrap(), 0_i32).unwrap(), v);
    let big = [0_u8, 0, 0, 0, 101, 0x3F, 0xF0, 0, 0, 0, 0, 0, 0, 0x40, 0, 0, 0, 0, 0, 0, 0];
    assert_eq!(read_binary(&big, 7_i32).unwrap(), Vector { x: Some(1.0_f64), y: Some(2.0_f64), z: None, srid: 7_i32 });
    assert_eq!(write_text(&Vector::EMPTY).unwrap(), "VECTOR EMPTY");
    let e = read_text("VECTOR Z (1 2)", 0_i32).unwrap_err();
    assert_eq!(e.sqlstate().as_str(), "2FF25");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §14.1.7, §17.2.20]
/// ST_OrderingEquals of two systems or two vectors compares them, of a
/// system and a vector it is not theirs to answer.
#[test]
fn ordering_equals_of_systems_and_vectors() {
    let wgs = system_value(&SpatialRefSys::from_srid(4326_i32).unwrap());
    let nad = system_value(&SpatialRefSys::from_srid(4269_i32).unwrap());
    assert!(values::ordering_equals(&wgs, &wgs).unwrap().unwrap());
    assert!(!values::ordering_equals(&wgs, &nad).unwrap().unwrap());
    let v = Vector { x: Some(1.0_f64), y: Some(2.0_f64), z: None, srid: 0_i32 }.value();
    let w = Vector { x: Some(1.0_f64), y: Some(2.0_f64), z: None, srid: 4326_i32 }.value();
    assert!(values::ordering_equals(&v, &v).unwrap().unwrap());
    assert!(!values::ordering_equals(&v, &w).unwrap().unwrap());
    assert!(values::ordering_equals(&v, &wgs).is_none());
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §14 to §18; Annex
/// A.1] each type has a constructor function of its name; each method's
/// SELF is of a type; each transform group's routines are the type's;
/// ST_MaxAngleString and ST_MaxDirectionString are the engine's.
#[test]
fn routine_tables_are_whole() {
    for s in SUPPORTS {
        assert!(ops::all().any(|op| op.form == Form::Constructor && op.name == s.name()), "{}", s.name());
    }
    for op in ops::all().filter(|op| op.form == Form::Method) {
        assert!(matches!(op.params.first(), Some(ops::Ty::Of(_))), "{}", op.name);
    }
    for g in TRANSFORM_GROUPS {
        let of = |name: &str| ops::all().any(|op| op.name == name && op.params.iter().chain([&op.result]).any(|t| *t == ops::Ty::Of(g.of)));
        assert!(of(g.to_sql) && of(g.from_sql), "{} of {}", g.name, g.of.name());
    }
    let size = |name: &str| sizings().into_iter().find(|(n, ..)| *n == name).and_then(|(_, v, _)| v);
    assert_eq!(size("ST_MaxAngleString"), i64::try_from(crate::spatial::srs::angle::MAX_ANGLE_STRING).ok());
    assert_eq!(size("ST_MaxDirectionString"), i64::try_from(crate::spatial::srs::direction::MAX_DIRECTION_STRING).ok());
    assert_eq!(sizings().len(), 11);
}

mod types;
mod vector_formats;

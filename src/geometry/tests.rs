//! Geometry representation requirements, including hostile interchange input.

use super::*;
use crate::bytes::Reader;
use crate::value::SqlType;

mod canonical;
pub(in crate::geometry) mod common;
mod damage;
mod distance;
mod envelopes;
mod functions;
mod prepared;
mod pruning;
mod reference;
mod relate;

/// Verifies: [EXTENSION] OGC 06-103r4 §§7.2.2, 8: all seven XY kinds
/// round trip through WKT, both byte orders, stored values and empties.
#[test]
fn shapes_and_empty_components_round_trip() {
    let cases = [
        "POINT (1 2)",
        "POINT EMPTY",
        "LINESTRING (0 0, 2 3)",
        "LINESTRING EMPTY",
        "POLYGON ((0 0, 4 0, 4 4, 0 0), (1 1, 2 1, 1 2, 1 1))",
        "POLYGON EMPTY",
        "MULTIPOINT ((1 2), EMPTY, (3 4))",
        "MULTIPOINT EMPTY",
        "MULTILINESTRING ((0 0, 1 1), EMPTY)",
        "MULTILINESTRING EMPTY",
        "MULTIPOLYGON (((0 0, 4 0, 4 4, 0 0)), EMPTY)",
        "MULTIPOLYGON EMPTY",
        "GEOMETRYCOLLECTION (POINT EMPTY, LINESTRING (1 2, 3 4), GEOMETRYCOLLECTION EMPTY)",
        "GEOMETRYCOLLECTION EMPTY",
    ];
    for text in cases {
        let g = Geometry::from_wkt(text, -17).unwrap();
        assert_eq!(g.to_wkt(), text);
        let binary = g.to_wkb();
        assert_eq!(Geometry::from_wkb(&binary, -17).unwrap().to_wkt(), text);
        let value = g.to_value();
        let read = Geometry::from_value(&value).unwrap();
        assert_eq!((read.to_wkt(), read.srid(), read.bounds()), (text.to_owned(), -17_i32, g.bounds()));
    }
}

/// Verifies: [EXTENSION] OGC 06-103r4 §7.2.1: case, whitespace, signed
/// decimal/exponent forms and signed zero; finite f64 extremes retain all
/// their coordinate bits through text and binary interchange.
#[test]
fn coordinates_round_trip_without_rounding_or_negative_zero() {
    let zero = Geometry::from_wkt(" \tPoInT( -0.0\n+.25E+1 ) \r", 0).unwrap();
    assert_eq!(zero.to_wkt(), "POINT (0 2.5)");
    assert_eq!(zero.bounds().unwrap().lower(), [0.0_f64, 2.5_f64]);
    for x in [f64::MAX, -f64::MAX, f64::MIN_POSITIVE, f64::from_bits(1), -f64::from_bits(1), 1.0_f64 / 3.0_f64] {
        let text = format!("POINT ({x:e} 0)");
        let g = Geometry::from_wkt(&text, 0).unwrap();
        let read = Geometry::from_wkt(&g.to_wkt(), 0).unwrap();
        assert_eq!(read.bounds().unwrap().lower()[0].to_bits(), x.to_bits());
        assert_eq!(Geometry::from_wkb(&g.to_wkb(), 0).unwrap().bounds(), g.bounds());
    }
}

/// Verifies: [DESIGN] D82: envelopes include every collection component
/// and ring, and an empty point set has no invented coordinate envelope.
#[test]
fn envelopes_cover_components_and_distinguish_empty_from_origin() {
    let g = Geometry::from_wkt("GEOMETRYCOLLECTION (POINT (-10 8), LINESTRING (3 -7, 6 2), POLYGON EMPTY)", 4326).unwrap();
    assert_eq!(g.bounds().unwrap(), Bounds::new([-10.0_f64, -7.0_f64], [6.0_f64, 8.0_f64]).unwrap());
    for text in ["POINT EMPTY", "MULTIPOINT (EMPTY)", "GEOMETRYCOLLECTION (POINT EMPTY, LINESTRING EMPTY)"] {
        let empty = Geometry::from_wkt(text, 0).unwrap();
        assert!(empty.is_empty());
        assert!(empty.bounds().is_none());
        assert!(!empty.to_value().is_null());
    }
    assert!(!Geometry::from_wkt("POINT (0 0)", 0).unwrap().is_empty());
}

/// Verifies: [EXTENSION] OGC 06-103r4 §8: each nested WKB component has
/// its own byte order; the endian byte is not inherited from its parent.
#[test]
fn binary_byte_order_is_local_to_each_component() {
    let mut big = vec![0];
    big.extend_from_slice(&1_u32.to_be_bytes());
    big.extend_from_slice(&2.5_f64.to_be_bytes());
    big.extend_from_slice(&(-3.0_f64).to_be_bytes());
    assert_eq!(Geometry::from_wkb(&big, 0).unwrap().to_wkt(), "POINT (2.5 -3)");
    let little = Geometry::from_wkt("POINT (7 8)", 0).unwrap().to_wkb();
    let mut collection = vec![0];
    collection.extend_from_slice(&7_u32.to_be_bytes());
    collection.extend_from_slice(&2_u32.to_be_bytes());
    collection.extend_from_slice(&little);
    collection.extend_from_slice(&big);
    assert_eq!(Geometry::from_wkb(&collection, 0).unwrap().to_wkt(), "GEOMETRYCOLLECTION (POINT (7 8), POINT (2.5 -3))");
}

/// Verifies: [DESIGN] D82: GEOMETRY's ordering and hashing identify its
/// canonical representation including SRID, without claiming ST_Equals.
#[test]
fn representations_keep_srid_and_coordinate_order() {
    let a = Geometry::from_wkt("POINT (-0 2)", 7).unwrap().to_value();
    let b = Geometry::from_wkt("point (0.0 2e0)", 7).unwrap().to_value();
    let c = Geometry::from_wkt("POINT (0 2)", 8).unwrap().to_value();
    assert_eq!(GEOMETRY.compare(&a, &b), Some(std::cmp::Ordering::Equal));
    assert_eq!(GEOMETRY.bucket(&a), GEOMETRY.bucket(&b));
    assert_ne!(GEOMETRY.compare(&a, &c), Some(std::cmp::Ordering::Equal));
    let forward = Geometry::from_wkt("LINESTRING (0 0, 1 1)", 0).unwrap().to_value();
    let reverse = Geometry::from_wkt("LINESTRING (1 1, 0 0)", 0).unwrap().to_value();
    assert_ne!(GEOMETRY.compare(&forward, &reverse), Some(std::cmp::Ordering::Equal));
    assert_eq!(GEOMETRY.compare(&a, &Value::Null), None);
}

/// Verifies: [DESIGN] D53, D82: record and key decoders reject unknown
/// versions, noncanonical bytes, and damaged payloads; framed decoding
/// retains the caller's corruption/connection error category.
#[test]
fn stored_payloads_are_versioned_and_checked() {
    let g = Geometry::from_wkt("POINT (1 2)", 0).unwrap();
    let v = g.to_value();
    let raw = GEOMETRY.encode_record(&v, &data_type()).unwrap();
    let mut bad = raw.clone();
    bad[0] = 2;
    assert!(GEOMETRY.decode_record(&bad, &data_type()).is_none());
    for end in 0..raw.len() {
        assert!(GEOMETRY.decode_record(&raw[..end], &data_type()).is_none());
    }
    let mut framed = Vec::new();
    GEOMETRY.put_value(&v, &mut framed);
    framed[4] = 2;
    assert_eq!(GEOMETRY.value(&mut Reader::new(&framed, "test")).unwrap_err().code, Code::Corrupt);
    assert_eq!(GEOMETRY.value(&mut Reader::received(&framed, "test")).unwrap_err().code, Code::Connection);
    assert_eq!(GEOMETRY.decode_key(&mut &[0][..], &data_type()).unwrap_err().code, Code::Corrupt);
}

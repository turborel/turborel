//! Rectangular polygon construction and coordinate-extrema observations.

use super::*;
use crate::functions::ScalarFunction;
use crate::geometry::GeometryFunction::{MakeEnvelope, MaxX, MaxY, MinX, MinY};
use crate::value::{DOUBLE_PRECISION, DataType, Decimal, Spelled};

const NUMBER: DataType = DataType::Approximate { precision: DOUBLE_PRECISION, spelled: Spelled::DoublePrecision };

/// Verifies: [EXTENSION] D82: extrema include every coordinate across
/// all seven XY kinds, ignore empty components and return NULL for an
/// empty set. These observers do not require a point geometry.
#[test]
fn extrema_cover_all_geometry_kinds_and_empty_components() {
    let cases: [(&str, [f64; 4]); 7] = [
        ("POINT (1 2)", [1.0_f64, 1.0, 2.0, 2.0]),
        ("LINESTRING (-3 4, 2 -5)", [-3.0, 2.0, -5.0, 4.0]),
        ("POLYGON ((0 0, 10 0, 10 10, 0 0))", [0.0, 10.0, 0.0, 10.0]),
        ("MULTIPOINT (EMPTY, (1 2), (-3 -4))", [-3.0, 1.0, -4.0, 2.0]),
        ("MULTILINESTRING (EMPTY, (2 3, 4 5))", [2.0, 4.0, 3.0, 5.0]),
        ("MULTIPOLYGON (EMPTY, ((1 2, 3 2, 3 4, 1 2)))", [1.0, 3.0, 2.0, 4.0]),
        ("GEOMETRYCOLLECTION (POINT EMPTY, GEOMETRYCOLLECTION (POINT (-8 9)), LINESTRING (1 2, 3 4))", [-8.0, 3.0, 2.0, 9.0]),
    ];
    for (text, expected) in cases {
        let geometry = Geometry::from_wkt(text, 4326).unwrap();
        let value = geometry.to_value();
        let empty = Geometry::from_wkt(&format!("{} EMPTY", geometry.geometry_type()), 4326).unwrap().to_value();
        for (function, ordinate) in [MinX, MaxX, MinY, MaxY].into_iter().zip(expected) {
            assert_eq!(function.call(std::slice::from_ref(&value)).unwrap().as_f64(), Some(ordinate), "{text}");
            assert!(function.call(std::slice::from_ref(&empty)).unwrap().is_null());
            assert!(function.call(&[Value::Null]).unwrap().is_null());
            assert_eq!(function.return_type(&[data_type()]).unwrap(), NUMBER);
        }
    }
    let empty = Geometry::from_wkt("GEOMETRYCOLLECTION (POINT EMPTY, MULTILINESTRING (EMPTY))", 0).unwrap().to_value();
    for function in [MinX, MaxX, MinY, MaxY] {
        assert!(function.call(std::slice::from_ref(&empty)).unwrap().is_null());
    }
}

/// Verifies: [EXTENSION] D82: a rectangle is a closed clockwise polygon
/// with the supplied bounds and SRID. Extreme and subnormal coordinates
/// need no overflowing width/area calculation; signed zero is normalized.
#[test]
fn rectangles_preserve_finite_extreme_and_subnormal_bounds() {
    let rectangle = Geometry::rectangle([1.0_f64, 2.0_f64], [3.0_f64, 4.0_f64], 4326).unwrap();
    assert_eq!(rectangle.to_wkt(), "POLYGON ((1 2, 1 4, 3 4, 3 2, 1 2))");
    assert_eq!(rectangle.srid(), 4326_i32);
    let points: [[f64; 2]; 4] = [[1.0, 2.0], [1.0, 3.0], [2.0, 3.0], [3.0, 4.0]];
    for [x, y] in points {
        assert!(rectangle.intersects(&Geometry::point(x, y, 4326).unwrap()).unwrap());
    }
    assert!(!rectangle.intersects(&Geometry::point(3.0_f64.next_up(), 3.0, 4326).unwrap()).unwrap());
    let huge = Geometry::rectangle([-f64::MAX, -f64::MAX], [f64::MAX, f64::MAX], i32::MIN).unwrap();
    assert!(huge.intersects(&Geometry::point(0.0, 0.0, i32::MIN).unwrap()).unwrap());
    let tiny = f64::from_bits(1);
    let small = Geometry::rectangle([-0.0_f64, -0.0_f64], [tiny, tiny], 0).unwrap();
    let bounds = small.bounds().unwrap();
    assert_eq!(bounds.lower().map(f64::to_bits), [0, 0]);
    assert_eq!(bounds.upper().map(f64::to_bits), [1, 1]);
    assert!(small.intersects(&Geometry::point(tiny, tiny, 0).unwrap()).unwrap());
    assert_eq!(Geometry::from_wkb(&small.to_wkb(), 0).unwrap().to_wkt(), small.to_wkt());
}

/// Verifies: [EXTENSION] D82: non-finite, reversed and zero-area bounds
/// are refused. MC/DC: each coordinate's finiteness and each axis's
/// ordering independently rejects an otherwise valid rectangle.
#[test]
fn rectangle_bounds_reject_nonfinite_reversed_and_zero_area() {
    let cases: [[f64; 4]; 4] = [[0.0, 0.0, 0.0, 1.0], [0.0, 0.0, 1.0, 0.0], [1.0, 0.0, 0.0, 1.0], [0.0, 1.0, 1.0, 0.0]];
    for coordinates in cases {
        let [min_x, min_y, max_x, max_y] = coordinates;
        assert_eq!(Geometry::rectangle([min_x, min_y], [max_x, max_y], 0).unwrap_err().code, Code::Data);
    }
    for position in 0..4 {
        for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut coordinates: [f64; 4] = [0.0, 0.0, 1.0, 1.0];
            coordinates[position] = invalid;
            let [min_x, min_y, max_x, max_y] = coordinates;
            assert_eq!(Geometry::rectangle([min_x, min_y], [max_x, max_y], 0).unwrap_err().code, Code::Data);
        }
    }
}

/// Verifies: [EXTENSION] D82: the numeric constructor accepts four
/// coordinates and an optional exact i32 SRID, propagates any NULL and
/// rejects a nonnumeric coordinate or fractional/out-of-range reference.
#[test]
fn envelope_function_types_every_coordinate_and_reference() {
    let coordinates = [Value::Approx(1.0), Value::Approx(2.0), Value::Approx(3.0), Value::Approx(4.0)];
    let value = MakeEnvelope.call(&coordinates).unwrap();
    assert_eq!(Geometry::from_value(&value).unwrap().srid(), 0_i32);
    for srid in [i32::MIN, 4326_i32, i32::MAX] {
        let mut args = coordinates.to_vec();
        args.push(Value::Exact(Decimal::new(i128::from(srid), 0)));
        assert_eq!(Geometry::from_value(&MakeEnvelope.call(&args).unwrap()).unwrap().srid(), srid);
        for position in 0..args.len() {
            let mut null = args.clone();
            null[position] = Value::Null;
            assert!(MakeEnvelope.call(&null).unwrap().is_null());
        }
    }
    for position in 0..coordinates.len() {
        let mut args = coordinates.to_vec();
        args[position] = Value::Char("bad".into());
        assert_eq!(MakeEnvelope.call(&args).unwrap_err().code, Code::Data);
        let mut types = vec![NUMBER; 4];
        types[position] = DataType::character(3);
        assert_eq!(MakeEnvelope.return_type(&types).unwrap_err().code, Code::SyntaxRule);
        assert!(MakeEnvelope.parameter_type(position, &[]).is_some());
    }
    assert!(MakeEnvelope.return_type(&vec![NUMBER; 4]).is_ok());
    assert!(MakeEnvelope.parameter_type(4, &[]).is_some());
    for srid in [Value::Exact(Decimal::new(1, 1)), Value::Approx(1.0), Value::Exact(Decimal::new(i128::MAX, 0))] {
        let mut args = coordinates.to_vec();
        args.push(srid);
        assert_eq!(MakeEnvelope.call(&args).unwrap_err().code, Code::Data);
    }
}

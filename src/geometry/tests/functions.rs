//! Direct function contracts and representation boundaries.

use super::*;
use crate::functions::ScalarFunction;
use crate::value::{Binary, DataType, Decimal};

/// Verifies: [EXTENSION] D85: SQL type names retain their SQL/MM spelling
/// for every basic kind, including empty geometries; Rust reports WKT kinds.
#[test]
fn type_names_distinguish_sql_names_from_wkt_kinds() {
    let names = [
        ("POINT", "ST_Point"),
        ("LINESTRING", "ST_LineString"),
        ("POLYGON", "ST_Polygon"),
        ("MULTIPOINT", "ST_MultiPoint"),
        ("MULTILINESTRING", "ST_MultiLineString"),
        ("MULTIPOLYGON", "ST_MultiPolygon"),
        ("GEOMETRYCOLLECTION", "ST_GeomCollection"),
    ];
    for (kind, expected) in names {
        let geometry = Geometry::from_wkt(&format!("{kind} EMPTY"), 0).unwrap();
        assert_eq!(geometry.geometry_type(), kind);
        assert_eq!(GeometryFunction::TypeName.call(&[geometry.to_value()]).unwrap().to_string(), expected);
    }
}

/// Verifies: [EXTENSION] D85: function arity, input types, exact signed
/// SRIDs, NULL propagation and unsupported index opt-in are explicit.
#[test]
fn function_contracts_reject_arity_and_propagate_null() {
    for function in FUNCTIONS {
        assert!(!function.index_predicate());
        assert!(function.deterministic());
        assert!(function.return_type(&[]).is_err());
        assert!(function.call(&[]).is_err());
        assert!(function.call(&vec![Value::Null; 6]).is_err());
        assert!(function.parameter_type(0, &[]).is_some());
        assert!(function.parameter_type(5, &[]).is_none());
        let args = if matches!(function, GeometryFunction::MakeEnvelope) {
            vec![Value::Null; 4]
        } else if matches!(
            function,
            GeometryFunction::Point
                | GeometryFunction::SetSrid
                | GeometryFunction::Intersects
                | GeometryFunction::Disjoint
                | GeometryFunction::Distance
        ) {
            vec![Value::Null, Value::Null]
        } else {
            vec![Value::Null]
        };
        assert!(function.call(&args).unwrap().is_null());
    }
}

/// Verifies: [EXTENSION] D85: SRIDs are signed 32-bit integers without
/// rounding; text, binary and geometry arguments are not interchangeable.
#[test]
fn function_contracts_refuse_malformed_types_and_srids() {
    let text = Value::Char("POINT EMPTY".into());
    for srid in [i128::from(i32::MIN), i128::from(i32::MAX)] {
        let value = GeometryFunction::FromText.call(&[text.clone(), Value::Exact(Decimal::new(srid, 0))]).unwrap();
        assert_eq!(GeometryFunction::Srid.call(&[value]).unwrap().to_string(), srid.to_string());
    }
    for srid in [Value::Exact(Decimal::new(1, 1)), Value::Approx(1.0), Value::Exact(Decimal::new(i128::MAX, 0))] {
        assert_eq!(GeometryFunction::FromText.call(&[text.clone(), srid]).unwrap_err().code, Code::Data);
    }
    assert!(GeometryFunction::FromBinary.call(std::slice::from_ref(&text)).is_err());
    assert!(GeometryFunction::FromText.call(&[Value::Binary(Binary::new(Vec::new()))]).is_err());
    assert!(GeometryFunction::AsText.call(&[text.clone(), Value::Null]).is_err());
    assert!(GeometryFunction::FromText.return_type(&[DataType::varying(10), data_type()]).is_err());
    assert_eq!(Geometry::from_value(&Value::Null).unwrap_err().code, Code::Data);
    assert!(!GEOMETRY.comparable(&DataType::character(1)));
    assert!(GEOMETRY.assign(text.clone(), &data_type(), "test").is_err());
    assert!(GEOMETRY.encode_record(&text, &data_type()).is_err());
    assert!(GEOMETRY.encode_key(&text, &data_type(), &mut Vec::new()).is_err());
    assert!(GEOMETRY.cast(Value::Null, &data_type()).unwrap().is_null());
}

/// Verifies: [EXTENSION] D85: point construction needs finite coordinates;
/// ordinates preserve their order, EMPTY has no ordinate, and relabeling
/// keeps the coordinates exactly while changing the reference identifier.
#[test]
fn point_construction_and_relabeling_preserve_coordinates() {
    let point = Geometry::point(4.9, 52.37, 4326).unwrap();
    let same = point.clone().with_srid(28992);
    assert_eq!(same.coordinates().unwrap(), point.coordinates().unwrap());
    assert_eq!(same.srid(), 28_992_i32);
    let empty = Geometry::from_wkt("POINT EMPTY", 4326).unwrap().to_value();
    let line = Geometry::from_wkt("LINESTRING (1 2, 3 4)", 0).unwrap().to_value();
    for function in [GeometryFunction::X, GeometryFunction::Y] {
        assert!(function.call(std::slice::from_ref(&empty)).unwrap().is_null());
        assert_eq!(function.call(std::slice::from_ref(&line)).unwrap_err().code, Code::Data);
    }
    for coordinates in [[f64::NAN, 0.0_f64], [0.0_f64, f64::INFINITY]] {
        assert_eq!(Geometry::point(coordinates[0], coordinates[1], 0).unwrap_err().code, Code::Data);
    }
    assert!(GeometryFunction::Point.call(&[Value::Char("x".into()), Value::Approx(0.0)]).is_err());
    assert!(GeometryFunction::Point.call(&[Value::Approx(0.0), Value::Char("y".into())]).is_err());
}

/// Verifies: [DESIGN] D85: SQL observers refuse results beyond existing
/// string limits while the Rust codecs can still round trip those shapes.
#[test]
fn sql_observer_limits_do_not_truncate_geometries() {
    let text = format!("LINESTRING ({})", vec!["12345 67890"; 4000].join(", "));
    let geometry = Geometry::from_wkt(&text, 0).unwrap();
    assert_eq!(geometry.to_wkt(), text);
    let value = geometry.to_value();
    for function in [GeometryFunction::AsText, GeometryFunction::AsBinary] {
        assert_eq!(function.call(std::slice::from_ref(&value)).unwrap_err().code, Code::Unsupported);
    }
    assert_eq!(Geometry::from_wkb(&geometry.to_wkb(), 0).unwrap().to_wkt(), text);
}

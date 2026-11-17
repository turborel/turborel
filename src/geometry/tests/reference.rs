//! SRID constraints apply at every typed value and storage boundary.

use super::*;
use crate::value::DataType;

/// Verifies: [EXTENSION] D53, D85: fixed-SRID descriptors reject invalid
/// parameters and round trip their versioned form; truncated or unknown
/// encodings cannot silently turn a constrained column into an unconstrained one.
#[test]
fn reference_descriptors_are_validated_and_versioned() {
    for srid in [0_u32, 4326, 2_147_483_647] {
        let t = data_type_with_srid(srid).unwrap();
        assert_eq!(t.to_string(), format!("GEOMETRY({srid})"));
        let mut bytes = Vec::new();
        GEOMETRY.put_params(&t, &mut bytes);
        assert_eq!(GEOMETRY.params(&mut Reader::new(&bytes, "test")).unwrap(), t);
        for end in 0..bytes.len() {
            assert_eq!(GEOMETRY.params(&mut Reader::new(&bytes[..end], "test")).unwrap_err().code, Code::Corrupt);
        }
        bytes[0] = 2;
        assert_eq!(GEOMETRY.params(&mut Reader::new(&bytes, "test")).unwrap_err().code, Code::Corrupt);
    }
    for params in [vec![2_147_483_648_u32], vec![u32::MAX], vec![4326_u32, 1]] {
        assert_eq!(GEOMETRY.check_params(&params).unwrap_err().code, Code::SyntaxRule);
    }
    for bytes in [vec![1_u8, 2], vec![1_u8, 1, 255, 255, 255, 255]] {
        assert_eq!(GEOMETRY.params(&mut Reader::new(&bytes, "test")).unwrap_err().code, Code::Corrupt);
    }
    assert!(data_type_with_srid(u32::MAX).is_err());
    assert_eq!(fixed_srid(&data_type()), None);
    assert_eq!(fixed_srid(&DataType::Boolean), None);
}

/// Verifies: [EXTENSION] D85: assignment, CAST, records, keys and anchors
/// enforce the declared SRID even for empty geometries. Text CAST supplies
/// the destination reference; casting a geometry never relabels it.
#[test]
fn typed_geometry_boundaries_enforce_reference() {
    let constrained = data_type_with_srid(4326).unwrap();
    let matching = Geometry::from_wkt("POINT EMPTY", 4326).unwrap().to_value();
    let wrong = Geometry::from_wkt("POINT EMPTY", 0).unwrap().to_value();
    assert!(GEOMETRY.assign(matching, &constrained, "test").is_ok());
    assert_eq!(GEOMETRY.assign(wrong.clone(), &constrained, "test").unwrap_err().code, Code::Data);
    assert_eq!(GEOMETRY.cast(wrong.clone(), &constrained).unwrap_err().code, Code::Data);
    assert!(GEOMETRY.cast(Value::Null, &constrained).unwrap().is_null());
    let cast = GEOMETRY.cast(Value::Char("POINT (1 2)".into()), &constrained).unwrap();
    assert_eq!(Geometry::from_value(&cast).unwrap().srid(), 4_326_i32);
    assert!(GEOMETRY.encode_record(&wrong, &constrained).is_err());
    let raw = GEOMETRY.encode_record(&wrong, &data_type()).unwrap();
    assert!(GEOMETRY.decode_record(&raw, &constrained).is_none());
    assert!(GEOMETRY.encode_key(&wrong, &constrained, &mut Vec::new()).is_err());
    let mut key = Vec::new();
    GEOMETRY.encode_key(&wrong, &data_type(), &mut key).unwrap();
    assert_eq!(GEOMETRY.decode_key(&mut key.as_slice(), &constrained).unwrap_err().code, Code::Corrupt);
    assert!(GEOMETRY.anchor(&wrong, &constrained).is_none());
}

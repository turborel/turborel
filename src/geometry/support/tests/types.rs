//! The types through the SqlType contract: their casts from character,
//! numeric and binary strings, their literals and their lack of keys.

use super::super::types::TYPES;
use super::super::values::{Knot, Vector};
use super::super::vector_formats::write_binary;
use super::super::{SUPPORTS, Support, data_type};
use super::{bytes, samples};
use crate::bytes::Reader;
use crate::error::SqlState;
use crate::value::{Binary, SqlType, Value};

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §14.1.8, §16.1,
/// §16.2, §17.2.22] a character string casts to a type of a transform
/// group as its well-known text, a number to an angle or a direction as
/// its radians, a binary string to a vector as its well-known binary; a
/// type of no transform group takes none of them (22005).
#[test]
fn values_cast_from_strings_and_numbers() {
    let cast = |s: Support, v: Value| TYPES[s.at()].cast(v, &data_type(s));
    let angle = cast(Support::Angle, Value::Approx(0.5_f64)).unwrap();
    assert_eq!(TYPES[Support::Angle.at()].position(&angle), Some(0.5_f64));
    let direction = cast(Support::Direction, Value::Approx(0.25_f64)).unwrap();
    assert_eq!(TYPES[Support::Direction.at()].position(&direction), Some(0.25_f64));
    assert!(cast(Support::Direction, Value::Approx(f64::NAN)).is_err());
    let v = Vector { x: Some(1.0_f64), y: Some(2.0_f64), z: None, srid: 0_i32 };
    let read = cast(Support::Vector, Value::Binary(Binary::new(write_binary(&v).unwrap()))).unwrap();
    assert_eq!(Vector::of(&read).unwrap(), v);
    let text = cast(Support::Vector, Value::Char("VECTOR (1 2)  ".into())).unwrap();
    assert_eq!(Vector::of(&text).unwrap(), v);
    assert!(cast(Support::Knot, Value::Null).unwrap().is_null());
    for s in [
        Support::Knot,
        Support::NurbsPoint,
        Support::AffinePlacement,
        Support::TextElement,
        Support::TextElementArray,
        Support::AnnotationText,
    ] {
        assert_eq!(cast(s, Value::Char("x".into())).unwrap_err().sqlstate(), SqlState::ERROR_IN_ASSIGNMENT, "{}", s.name());
    }
    assert_eq!(cast(Support::Vector, Value::Bool(true)).unwrap_err().sqlstate(), SqlState::ERROR_IN_ASSIGNMENT);
    assert_eq!(cast(Support::Angle, Value::Binary(Binary::new(vec![1_u8]))).unwrap_err().sqlstate(), SqlState::ERROR_IN_ASSIGNMENT);
    assert!(TYPES[Support::Knot.at()].assign(Value::Bool(true), &data_type(Support::Knot), "r").is_err());
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §14.1.8, §16.1.20,
/// §16.2.22, §17.2.21] a value is written as the constructor of its well-
/// known text, or of a type of no transform group as the constructor that
/// makes it of its attributes, or the constructor function and the
/// mutators of those it has.
#[test]
fn values_are_written_as_their_constructors() {
    let prefixes = [
        "ST_SPATIALREFSYS('",
        "ST_ANGLEFROMTEXT('",
        "ST_DIRECTIONFRMTXT('",
        "ST_VECTORFROMTEXT('VECTOR Z (1 2 3)', 4326)",
        "ST_AFFINEPLACEMENT(ST_GEOMFROMTEXT('POINT (1 2)', 0), ARRAY[ST_VECTORFROMTEXT(",
        "ST_NURBSPOINT(ST_GEOMFROMTEXT('POINT (1 2)', 0), 5e-1)",
        "ST_KNOT(1e0, 2)",
        "(NEW ANNOTATION_TEXT_ELEMENT()).VALUE('Lake')",
        "ANNOTATION_TEXT_ELEMENT",
        "ANNOTATION_TEXT",
    ];
    for ((s, v), want) in SUPPORTS.into_iter().zip(samples()).zip(prefixes) {
        let kind = &TYPES[s.at()];
        assert_eq!(kind.which(), s);
        let literal = kind.literal(&v);
        assert!(literal.contains(want), "{}: {literal}", s.name());
        assert_eq!(kind.anchor(&v, &data_type(s)).map(|(a, exact)| (bytes(&a), exact)), Some((bytes(&v), true)));
        assert!(kind.bucket(&v).is_some());
        assert!(kind.anchor(&Value::Bool(true), &data_type(s)).is_none());
        assert!(kind.bucket(&Value::Bool(true)).is_none());
        let mut params = Vec::new();
        kind.put_params(&data_type(s), &mut params);
        assert_eq!(kind.params(&mut Reader::new(&params, "test")).unwrap(), data_type(s));
    }
    let empty = Knot { value: None, multiplicity: None }.value();
    assert_eq!(TYPES[Support::Knot.at()].literal(&empty), "NEW ST_KNOT()");
    let half = Knot { value: Some(1.0_f64), multiplicity: None }.value();
    assert_eq!(TYPES[Support::Knot.at()].literal(&half), "(NEW ST_KNOT()).ST_VALUE(1e0)");
    assert_eq!(TYPES[Support::Angle.at()].literal(&Value::Bool(true)), "ST_ANGLEFROMTEXT()");
    assert_eq!(TYPES[Support::Vector.at()].literal(&Value::Bool(true)), "ST_VECTORFROMTEXT(, 0)");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §17.2, §17.3]
/// vectors compare by their stored form under EQUALS ONLY, and a type
/// whose values are not ordered fully has no key to read back.
#[test]
fn unordered_types_compare_bytes_and_have_no_keys() {
    let kind = &TYPES[Support::Vector.at()];
    let a = Vector { x: Some(1.0_f64), y: Some(2.0_f64), z: None, srid: 0_i32 }.value();
    assert_eq!(kind.compare(&a, &a), Some(std::cmp::Ordering::Equal));
    assert_eq!(kind.compare(&a, &Value::Bool(true)), None);
    let t = data_type(Support::Vector);
    assert!(kind.decode_key(&mut [0_u8; 8].as_slice(), &t).is_err());
    let angle = &TYPES[Support::Angle.at()];
    assert!(angle.decode_key(&mut [0_u8; 3].as_slice(), &data_type(Support::Angle)).is_err());
}

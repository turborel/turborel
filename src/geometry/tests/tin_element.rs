//! The ST_TINElement type as a kind of type: its text, its stored and
//! record forms, its lack of an ordering and its checks.

use crate::bytes::{Put, Reader};
use crate::error::{Code, SqlState};
use crate::geometry::model::{Body, Curve, Dims, Geom};
use crate::geometry::tin_element::{self, Parts, TIN_ELEMENT_TYPE};
use crate::value::{Bucket, SqlType, Value};

fn breakline() -> Value {
    let g = Geom::new(0, Dims::Xyz, Body::Curve(Curve::LineString(vec![0.0_f64, 0.0_f64, 1.0_f64, 1.0_f64, 1.0_f64, 1.0_f64]))).unwrap();
    tin_element::element("breakline", 7, "b", &g).unwrap()
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §17.1.1] an
/// ST_TINElement value is written as the text of a TIN's element, its
/// type named ST_TINELEMENT.
#[test]
fn tin_element_text_and_no_ordering_17_1_1() {
    let t = tin_element::data_type();
    let v = breakline();
    assert_eq!(TIN_ELEMENT_TYPE.written(&t), "ST_TINELEMENT");
    assert_eq!(TIN_ELEMENT_TYPE.text(&v), "BREAKLINE ID 7 TAG \"b\" LINESTRING Z (0 0 1, 1 1 1)");
    assert_eq!(TIN_ELEMENT_TYPE.literal(&v), TIN_ELEMENT_TYPE.text(&v));
    assert_eq!(TIN_ELEMENT_TYPE.text(&tin_element::empty()), " ID 0 TAG \"\" ");
    assert_eq!(TIN_ELEMENT_TYPE.text(&Value::Bool(true)), "");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §17.1.1] an
/// ST_TINElement value has no ordering: no comparison, position or key;
/// its bucket and anchor are its payload.
#[test]
fn tin_element_has_no_key_17_1_1() {
    let t = tin_element::data_type();
    let v = breakline();
    assert!(!TIN_ELEMENT_TYPE.comparable(&t));
    assert_eq!(TIN_ELEMENT_TYPE.compare(&v, &v), None);
    assert_eq!(TIN_ELEMENT_TYPE.position(&v), None);
    assert_eq!(TIN_ELEMENT_TYPE.record_width(&t), None);
    let mut key = Vec::new();
    assert_eq!(TIN_ELEMENT_TYPE.encode_key(&v, &t, &mut key).unwrap_err().code, Code::SyntaxRule);
    assert_eq!(TIN_ELEMENT_TYPE.decode_key(&mut &key[..], &t).unwrap_err().code, Code::SyntaxRule);
    assert_eq!(TIN_ELEMENT_TYPE.bucket(&v), Some(Bucket::Extension(TIN_ELEMENT_TYPE.encode_record(&v, &t).unwrap().into())));
    assert_eq!(TIN_ELEMENT_TYPE.bucket(&Value::Bool(true)), None);
    assert_eq!(format!("{:?}", TIN_ELEMENT_TYPE.anchor(&v, &t)), format!("{:?}", Some((v.clone(), true))));
    assert!(TIN_ELEMENT_TYPE.anchor(&Value::Bool(true), &t).is_none());
    assert_eq!(TIN_ELEMENT_TYPE.descriptor(&t), crate::value::types::Descriptor::of("ST_TINELEMENT"));
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.13 SR8, §9.2] an ST_TINElement value
/// or NULL is assigned and cast as it is; any other value is refused
/// with a data exception.
#[test]
fn tin_element_assignment_and_cast_9_2() {
    let t = tin_element::data_type();
    let v = breakline();
    assert_eq!(format!("{:?}", TIN_ELEMENT_TYPE.assign(v.clone(), &t, "r").unwrap()), format!("{v:?}"));
    assert!(TIN_ELEMENT_TYPE.cast(Value::Null, &t).unwrap().is_null());
    assert_eq!(TIN_ELEMENT_TYPE.cast(Value::Bool(true), &t).unwrap_err().code, Code::Data);
    assert_eq!(TIN_ELEMENT_TYPE.encode_record(&Value::Bool(true), &t).unwrap_err().code, Code::Api);
}

/// Verifies: [DESIGN] RULES.md D53 an ST_TINElement value round trips
/// through its stored and record forms, its parameters are none, and a
/// damaged payload is reported corrupt or not taken.
#[test]
fn tin_element_stored_forms_round_trip() {
    let t = tin_element::data_type();
    for v in [breakline(), tin_element::empty()] {
        let mut out = Vec::new();
        TIN_ELEMENT_TYPE.put_value(&v, &mut out);
        assert_eq!(format!("{:?}", TIN_ELEMENT_TYPE.value(&mut Reader::new(&out, "test")).unwrap()), format!("{v:?}"));
        let record = TIN_ELEMENT_TYPE.encode_record(&v, &t).unwrap();
        assert_eq!(format!("{:?}", TIN_ELEMENT_TYPE.decode_record(&record, &t)), format!("{:?}", Some(v)));
    }
    let mut params = Vec::new();
    TIN_ELEMENT_TYPE.put_params(&t, &mut params);
    assert_eq!(params, Vec::<u8>::new());
    assert_eq!(TIN_ELEMENT_TYPE.params(&mut Reader::new(&params, "test")).unwrap(), t);
    let mut bad = Vec::new();
    bad.put_bytes(&[2_u8, 0]);
    assert_eq!(TIN_ELEMENT_TYPE.value(&mut Reader::new(&bad, "test")).unwrap_err().code, Code::Corrupt);
    for damaged in [vec![1_u8], vec![1_u8, 0, 9], vec![1_u8, 2, 0], vec![1_u8, 1, 5, b'a'], vec![1_u8, 1, 1, 0xFF], vec![1_u8, 8, 0]] {
        assert!(TIN_ELEMENT_TYPE.decode_record(&damaged, &t).is_none());
    }
    assert_eq!(Parts::of_value(&Value::Bool(true)).unwrap_err().code, Code::SyntaxRule);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §17.1.2] an element
/// of an unknown type, of a tag longer than 64 characters or of a
/// geometry its type does not take is refused with its SQLSTATE.
#[test]
fn tin_element_parts_are_checked_17_1_2() {
    let g = Geom::new(0, Dims::Xy, Body::Curve(Curve::LineString(vec![0.0_f64, 0.0_f64, 1.0_f64, 1.0_f64]))).unwrap();
    assert_eq!(tin_element::element("cliff", 1, "", &g).unwrap_err().state, Some(SqlState::SPATIAL_INVALID_ARGUMENT));
    assert_eq!(tin_element::element("breakline", 1, "", &g).unwrap_err().state, Some(SqlState::SPATIAL_INVALID_GEOMETRY));
    assert!(tin_element::element("stop line", 1, "", &g).is_ok());
    let long = "t".repeat(tin_element::TAG_LENGTH + 1);
    assert_eq!(tin_element::element("stop line", 1, &long, &g).unwrap_err().state, Some(SqlState::SPATIAL_INVALID_ARGUMENT));
    let tagged = Parts { tag: Some("t".repeat(tin_element::TAG_LENGTH)), ..Parts::default() };
    let parts = Parts::of_value(&tagged.to_value().unwrap()).unwrap();
    assert!(!parts.is_empty());
    assert_eq!(parts.tag.unwrap().len(), tin_element::TAG_LENGTH);
}

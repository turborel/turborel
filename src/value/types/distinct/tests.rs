use super::*;
use crate::value::Decimal;

/// The distinct type S.MONEY of NUMERIC(9, 2).
fn money() -> DataType {
    DataType::distinct("S", "MONEY", DataType::Exact { precision: 9, scale: 2, spelled: crate::value::Spelled::Numeric })
}

/// Verifies: [DESIGN] RULES.md D69 [ISO/IEC 9075-2:2023 §4.9.2, §11.51
/// GR2] a distinct type's kind given a type of another kind, as no type of
/// it is, refuses to assign, cast, encode a record or a key, or read a key
/// of it, and has no width, record or anchor; its designator and
/// descriptor are those of a user-defined type unnamed.
#[test]
fn a_type_not_distinct_is_refused() {
    let other = DataType::Boolean;
    let v = Value::Bool(true);
    assert_eq!(DISTINCT.written(&other), USER_DEFINED_NAME);
    assert_eq!(DISTINCT.descriptor(&other).user_defined, None);
    assert_eq!(DISTINCT.assign(v.clone(), &other, "store").unwrap_err().sqlstate().as_str(), "22005");
    assert_eq!(DISTINCT.cast(v.clone(), &other).unwrap_err().sqlstate().as_str(), "22005");
    assert_eq!(DISTINCT.encode_record(&v, &other).unwrap_err().code, Code::Api);
    assert_eq!(DISTINCT.encode_key(&v, &other, &mut Vec::new()).unwrap_err().code, Code::Api);
    assert!(DISTINCT.decode_key(&mut &[1_u8][..], &other).is_err());
    assert_eq!(DISTINCT.record_width(&other), None);
    assert!(DISTINCT.decode_record(&[1], &other).is_none());
    assert!(DISTINCT.anchor(&v, &other).is_none());
    let mut none = Vec::new();
    DISTINCT.put_params(&other, &mut none);
    assert!(none.is_empty(), "no parameters of a type not distinct");
}

/// Verifies: [DESIGN] RULES.md D69 [ISO/IEC 9075-2:2023 §11.51 GR2b, GR2c]
/// a distinct type's values are its source's: their literal, text,
/// position, bucket, order, width and anchor; the null value's literal is
/// NULL, its text empty and its position none; its descriptor names it.
#[test]
fn values_are_the_sources() {
    let t = money();
    let (a, b) = (Value::Exact(Decimal::new(150, 2)), Value::Exact(Decimal::new(2, 0)));
    assert_eq!(DISTINCT.written(&t), "S.MONEY");
    assert_eq!(DISTINCT.descriptor(&t).user_defined, Some(("S".to_owned(), "MONEY".to_owned())));
    assert_eq!(DISTINCT.literal(&a), a.to_literal());
    assert_eq!(DISTINCT.text(&a), a.kind().unwrap().text(&a));
    assert_eq!(DISTINCT.position(&a), Some(1.5_f64));
    assert_eq!(
        (DISTINCT.literal(&Value::Null), DISTINCT.text(&Value::Null), DISTINCT.position(&Value::Null)),
        ("NULL".to_owned(), String::new(), None)
    );
    assert_eq!(DISTINCT.compare(&a, &b), Some(Ordering::Less));
    assert_eq!(DISTINCT.bucket(&a), crate::value::bucket(&a));
    assert_eq!(DISTINCT.record_width(&t), t.represented().kind().record_width(t.represented()));
    let (anchor, exact) = DISTINCT.anchor(&a, &t).unwrap();
    assert!(exact && anchor.to_literal() == a.to_literal());
    let mut bytes = Vec::new();
    DISTINCT.put_value(&a, &mut bytes);
    assert_eq!(DISTINCT.value(&mut Reader::new(&bytes, "value")).unwrap().to_literal(), a.to_literal());
}

/// Verifies: [DESIGN] RULES.md D69 [ISO/IEC 9075-2:2023 §11.51 SR5a, SR7a]
/// a distinct type's parameters read back; parameters naming a source
/// that is a distinct type or a row type are malformed, as no distinct
/// type has one.
#[test]
fn parameters_of_a_source_not_allowed_are_malformed() {
    let t = money();
    let mut params = Vec::new();
    DISTINCT.put_params(&t, &mut params);
    assert_eq!(DISTINCT.params(&mut Reader::new(&params, "parameters")).unwrap(), t);
    let field = super::super::Field { name: "X".into(), data_type: DataType::Boolean, unnamed: false };
    for source in [money(), DataType::row(vec![field])] {
        let mut bad = Vec::new();
        bad.put_str("S");
        bad.put_str("BAD");
        tags::put_type(&mut bad, &source);
        assert!(DISTINCT.params(&mut Reader::new(&bad, "parameters")).is_err(), "{source}");
    }
}

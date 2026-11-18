//! Unit tests of VECTOR's set functions.

use super::*;
use crate::functions::AggregateFunction;

fn run(of: Aggregate, values: &[Value]) -> Result<Value> {
    let mut acc = of.accumulator(data_type());
    for v in values {
        acc.update(v)?;
    }
    acc.finish()
}

/// Verifies: [EXTENSION] §6.5's set functions element by element:
/// VECTOR_SUM and VECTOR_AVG of the wider format (R6.9), VECTOR_MIN and
/// VECTOR_MAX keeping a shared format, sparse values counting their
/// unstored dimensions as zero and staying sparse (R6.10), null for no
/// values, a dimension mismatch SQLCODE -301.
#[test]
fn set_functions_are_element_wise() {
    let rows = [dense(Element::Int8, &[1.0_f64, -2.0_f64]), dense(Element::Int8, &[3.0_f64, 4.0_f64])];
    let sum = run(Aggregate::Sum, &rows).unwrap();
    assert_eq!((VECTOR.text(&sum), vector_of(&sum).unwrap().element), ("[4,2]".to_owned(), Element::Float32));
    assert_eq!(VECTOR.text(&run(Aggregate::Avg, &rows).unwrap()), "[2,1]");
    let low = run(Aggregate::Min, &rows).unwrap();
    assert_eq!((VECTOR.text(&low), vector_of(&low).unwrap().element), ("[1,-2]".to_owned(), Element::Int8));
    assert_eq!(VECTOR.text(&run(Aggregate::Max, &rows).unwrap()), "[3,4]");
    let s = [sparse(Element::Float32, 5, &[(1, 2.0_f64)]), sparse(Element::Float32, 5, &[(1, 4.0_f64), (3, -1.0_f64)])];
    assert_eq!(VECTOR.text(&run(Aggregate::Avg, &s).unwrap()), "[5,[1,3],[3,-0.5]]");
    assert_eq!(VECTOR.text(&run(Aggregate::Max, &s).unwrap()), "[5,[1],[4]]");
    assert_eq!(VECTOR.text(&run(Aggregate::Min, &s).unwrap()), "[5,[1,3],[2,-1]]");
    let bits = [dense(Element::Binary, &[1.0_f64; 8]), dense(Element::Binary, &[0.0_f64; 8])];
    assert_eq!(VECTOR.text(&run(Aggregate::Max, &bits).unwrap()), "[255]");
    assert!(run(Aggregate::Sum, &bits).is_err());
    assert!(run(Aggregate::Sum, &[]).unwrap().is_null());
    let err = run(Aggregate::Sum, &[rows[0].clone(), dense(Element::Int8, &[1.0_f64])]).unwrap_err();
    assert_eq!(err.sqlcode(), -301_i32);
    assert_eq!(Aggregate::Sum.result_type(Some(data_type())).unwrap(), data_type());
    assert_eq!(Aggregate::Sum.result_type(Some(DataType::Boolean)).unwrap_err().sqlcode(), -103_i32);
    assert_eq!(aggregates().len(), 5);
}

/// Verifies: [EXTENSION] §7.1 VECTOR_QUANTIZE_PARAMS: the scale and zero
/// point mapping the values' least to -128 and greatest to 127, so that
/// VECTOR_QUANTIZE and VECTOR_DEQUANTIZE with them round-trip to within
/// a step.
#[test]
fn quantize_params_span_the_range() {
    let rows = [dense(Element::Float32, &[-1.0_f64, 0.5_f64]), dense(Element::Float32, &[1.55_f64, 0.0_f64])];
    let Value::Array(params) = run(Aggregate::QuantizeParams, &rows).unwrap() else { panic!("not an array") };
    let (scale, zero) = (params[0].as_f64().unwrap(), params[1].as_f64().unwrap());
    assert!((scale - 2.55 / 255.0).abs() < 1e-9_f64);
    assert_eq!(zero, -28.0_f64);
    let q = |x: f64| ((x / scale).round() + zero).clamp(-128.0, 127.0);
    assert_eq!((q(-1.0_f64), q(1.55_f64)), (-128.0_f64, 127.0_f64));
}

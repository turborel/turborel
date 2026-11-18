//! Unit tests of VECTOR's scalar functions.

use super::*;
use crate::functions::ScalarFunction;
use crate::value::Decimal;
use crate::vector::functions::{FLOAT64_TYPE, INTEGER_TYPE, OPERATORS, TEXT_TYPE};
use std::sync::Arc;

fn function(name: &str) -> Arc<dyn ScalarFunction> {
    functions().into_iter().find(|f| f.name() == name).unwrap()
}

fn call(name: &str, args: &[Value]) -> Result<Value> {
    function(name).call(args)
}

fn text(s: &str) -> Value {
    Value::Char(s.into())
}

fn int(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

fn approx(v: &Value) -> f64 {
    v.as_f64().unwrap()
}

/// A character string result's text.
fn chars(v: Value) -> String {
    let Value::Char(text) = v else { panic!("not a character string") };
    text
}

/// Verifies: [EXTENSION] §6.1: VECTOR_DISTANCE in both argument orders
/// (R6.1), its metric in any case (R6.2), COSINE when not given (R6.3), a
/// FLOAT64 result and NULL from a NULL operand (R6.5); the shorthands of
/// §6.3, INNER_PRODUCT not negated; an unknown metric is SQLCODE -301.
#[test]
fn distances_take_both_orders() {
    let (a, b) = (dense(Element::Float32, &[1.0_f64, 0.0_f64]), dense(Element::Float32, &[0.0_f64, 2.0_f64]));
    let oracle = call("VECTOR_DISTANCE", &[a.clone(), b.clone(), text("euclidean")]).unwrap();
    let server = call("VECTOR_DISTANCE", &[text("EUCLIDEAN"), a.clone(), b.clone()]).unwrap();
    assert!((approx(&oracle) - 5_f64.sqrt()).abs() < 1e-12_f64 && (approx(&server) - 5_f64.sqrt()).abs() < 1e-12_f64);
    assert_eq!(approx(&call("VECTOR_DISTANCE", &[a.clone(), b.clone()]).unwrap()), 1.0_f64);
    assert!(call("VECTOR_DISTANCE", &[a.clone(), Value::Null]).unwrap().is_null());
    assert_eq!(call("VECTOR_DISTANCE", &[a.clone(), b.clone(), text("WARP")]).unwrap_err().sqlcode(), -301_i32);
    let (p, q) = (dense(Element::Float32, &[1.0_f64, 2.0_f64]), dense(Element::Float32, &[3.0_f64, -1.0_f64]));
    assert_eq!(approx(&call("L1_DISTANCE", &[p.clone(), q.clone()]).unwrap()), 5.0_f64);
    assert!((approx(&call("L2_DISTANCE", &[p.clone(), q.clone()]).unwrap()) - 13_f64.sqrt()).abs() < 1e-12_f64);
    assert_eq!(approx(&call("INNER_PRODUCT", &[p.clone(), q.clone()]).unwrap()), 1.0_f64);
    assert_eq!(approx(&call("VECTOR_DISTANCE", &[p.clone(), q.clone(), text("dot")]).unwrap()), -1.0_f64);
    assert_eq!(approx(&call("HAMMING_DISTANCE", &[p.clone(), q.clone()]).unwrap()), 2.0_f64);
    let zero = dense(Element::Float32, &[0.0_f64, 0.0_f64]);
    assert!(call("COSINE_DISTANCE", &[zero, p]).unwrap().is_null());
    let bits = dense(Element::Binary, &[1.0_f64, 1.0_f64, 0.0_f64, 0.0_f64, 0.0_f64, 0.0_f64, 0.0_f64, 0.0_f64]);
    let other = dense(Element::Binary, &[1.0_f64, 0.0_f64, 0.0_f64, 0.0_f64, 0.0_f64, 0.0_f64, 0.0_f64, 0.0_f64]);
    assert_eq!(approx(&call("JACCARD_DISTANCE", &[bits, other]).unwrap()), 0.5_f64);
    assert_eq!(OPERATORS.len(), 6);
}

/// Verifies: [EXTENSION] the functions' argument checks as bound: counts,
/// a metric first selecting the SQL Server order, character strings
/// accepted where a vector is (R5.1), and the declared result types.
#[test]
fn return_types_check_arguments() {
    let v = data_type();
    let distance = function("VECTOR_DISTANCE");
    assert_eq!(distance.return_type(&[v.clone(), v.clone()]).unwrap(), FLOAT64_TYPE);
    assert_eq!(distance.return_type(&[TEXT_TYPE, v.clone(), v.clone()]).unwrap(), FLOAT64_TYPE);
    assert_eq!(distance.return_type(&[v.clone(), v.clone(), TEXT_TYPE]).unwrap(), FLOAT64_TYPE);
    assert_eq!(distance.return_type(std::slice::from_ref(&v)).unwrap_err().sqlcode(), -103_i32);
    assert_eq!(distance.return_type(&[v.clone(), INTEGER_TYPE]).unwrap_err().sqlcode(), -103_i32);
    assert_eq!(distance.argument_type(2, &[Some(v.clone()), Some(v.clone()), None]), Some(TEXT_TYPE));
    assert_eq!(distance.argument_type(2, &[Some(TEXT_TYPE), None, None]), Some(data_type()));
    let fixed = typed_of(&[3]);
    assert_eq!(distance.argument_type(0, &[Some(fixed.clone()), None]), Some(fixed));
    assert_eq!(function("VECTOR_DIMENSION_COUNT").return_type(&[TEXT_TYPE]).unwrap(), INTEGER_TYPE);
    assert_eq!(function("VECTOR_EQUALS").return_type(&[v.clone(), v.clone()]).unwrap(), DataType::Boolean);
    assert_eq!(function("TO_VECTOR").argument_type(0, &[Some(TEXT_TYPE)]), None);
    assert!(function("FROM_VECTOR_CLOB").return_type(&[v]).unwrap().is_lob());
}

/// Verifies: [EXTENSION] every function is listed once, and a shorthand
/// names the metric an index must have to serve it (R13.1).
#[test]
fn functions_are_listed_with_their_metrics() {
    let mut names: Vec<String> = functions().iter().map(|f| f.name().to_owned()).collect();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), 31);
    assert_eq!(crate::vector::functions::Distance::L2.metric(), Some("EUCLIDEAN"));
    assert_eq!(crate::vector::functions::Distance::VectorDistance.metric(), None);
}

/// Verifies: [EXTENSION] §6.4's utilities: norms by name, normalizing (a
/// zero vector to NULL, INT8 to FLOAT32), dimension count, format,
/// layout and non-zeros.
#[test]
fn utilities_answer() {
    let v = dense(Element::Float32, &[3.0_f64, -4.0_f64, 0.0_f64]);
    assert_eq!(approx(&call("VECTOR_NORM", std::slice::from_ref(&v)).unwrap()), 5.0_f64);
    assert_eq!(approx(&call("VECTOR_NORM", &[v.clone(), text("norm1")]).unwrap()), 7.0_f64);
    assert_eq!(approx(&call("VECTOR_NORM", &[v.clone(), text("NORMINF")]).unwrap()), 4.0_f64);
    assert!(call("VECTOR_NORM", &[v.clone(), text("norm3")]).is_err());
    let unit = call("VECTOR_NORMALIZE", &[dense(Element::Int8, &[3.0_f64, 4.0_f64])]).unwrap();
    assert_eq!(VECTOR.text(&unit), "[0.6,0.8]");
    assert!(vector_of(&unit).unwrap().normalized);
    assert!(call("VECTOR_NORMALIZE", &[dense(Element::Float32, &[0.0_f64])]).unwrap().is_null());
    assert_eq!(call("VECTOR_DIMENSION_COUNT", std::slice::from_ref(&v)).unwrap().as_f64(), Some(3.0_f64));
    assert_eq!(chars(call("VECTOR_DIMENSION_FORMAT", std::slice::from_ref(&v)).unwrap()), "FLOAT32");
    assert_eq!(chars(call("VECTOR_LAYOUT", std::slice::from_ref(&v)).unwrap()), "DENSE");
    assert_eq!(call("VECTOR_NNZ", std::slice::from_ref(&v)).unwrap().as_f64(), Some(2.0_f64));
}

/// Verifies: [EXTENSION] §6.4's VECTOR_ELEMENT and VECTOR_SLICE count
/// from 1, an element or slice past the end SQLSTATE 2202E; VECTOR_CONCAT
/// joins vectors of one format.
#[test]
fn utilities_take_parts() {
    let v = dense(Element::Float32, &[3.0_f64, -4.0_f64, 0.0_f64]);
    assert_eq!(approx(&call("VECTOR_ELEMENT", &[v.clone(), int(2)]).unwrap()), -4.0_f64);
    let err = call("VECTOR_ELEMENT", &[v.clone(), int(4)]).unwrap_err();
    assert_eq!((err.sqlcode(), err.sqlstate().as_str()), (-301_i32, "2202E"));
    assert!(call("VECTOR_ELEMENT", &[v.clone(), int(0)]).is_err());
    assert_eq!(VECTOR.text(&call("VECTOR_SLICE", &[v.clone(), int(2), int(2)]).unwrap()), "[-4,0]");
    assert!(call("VECTOR_SLICE", &[v.clone(), int(3), int(2)]).is_err());
    let s = sparse(Element::Float32, 10, &[(4, 1.0_f64), (8, 2.0_f64)]);
    assert_eq!(VECTOR.text(&call("VECTOR_SLICE", &[s.clone(), int(5), int(4)]).unwrap()), "[4,[0],[1]]");
    assert_eq!(VECTOR.text(&call("VECTOR_CONCAT", &[v.clone(), v.clone()]).unwrap()), "[3,-4,0,3,-4,0]");
    assert_eq!(VECTOR.text(&call("VECTOR_CONCAT", &[s.clone(), v.clone()]).unwrap()), "[13,[4,8,10,11],[1,2,3,-4]]");
    assert!(call("VECTOR_CONCAT", &[v, dense(Element::Float64, &[1.0_f64])]).is_err());
}

/// Verifies: [EXTENSION] VECTOR_EQUALS is bit-identical, format included
/// (R3.12); VECTORPROPERTY; VECTOR_TO_ARRAY and ARRAY_TO_VECTOR
/// round-trip, a null element refused (R3.7).
#[test]
fn utilities_compare_and_convert() {
    let v = dense(Element::Float32, &[3.0_f64, -4.0_f64, 0.0_f64]);
    assert!(matches!(
        call("VECTOR_EQUALS", &[v.clone(), dense(Element::Float32, &[3.0_f64, -4.0_f64, 0.0_f64])]).unwrap(),
        Value::Bool(true)
    ));
    assert!(matches!(
        call("VECTOR_EQUALS", &[v.clone(), dense(Element::Float64, &[3.0_f64, -4.0_f64, 0.0_f64])]).unwrap(),
        Value::Bool(false)
    ));
    assert_eq!(chars(call("VECTORPROPERTY", &[v.clone(), text("Dimensions")]).unwrap()), "3");
    assert_eq!(chars(call("VECTORPROPERTY", &[v.clone(), text("basetype")]).unwrap()), "float32");
    let array = call("VECTOR_TO_ARRAY", std::slice::from_ref(&v)).unwrap();
    let back = call("ARRAY_TO_VECTOR", &[array, text("FLOAT64")]).unwrap();
    assert_eq!(VECTOR.text(&back), "[3,-4,0]");
    assert!(call("ARRAY_TO_VECTOR", &[Value::Array(Arc::from(vec![Value::Null]))]).is_err());
}

/// Verifies: [EXTENSION] §5.2: TO_VECTOR and VECTOR read text against a
/// dimension count, format and layout, null leaving each open.
#[test]
fn constructors_and_text_forms() {
    let v = call("TO_VECTOR", &[text("[1, 2, 3]"), int(3), text("int8")]).unwrap();
    assert_eq!(vector_of(&v).unwrap().element, Element::Int8);
    assert!(call("TO_VECTOR", &[text("[1, 2]"), int(3)]).is_err());
    let open = call("VECTOR", &[text("[1.5]"), Value::Null, Value::Null]).unwrap();
    assert_eq!(vector_of(&open).unwrap().element, Element::Float32);
    let s = call("TO_VECTOR", &[text("[1,0,2]"), int(3), text("*"), text("SPARSE")]).unwrap();
    assert_eq!(VECTOR.text(&s), "[3,[0,2],[1,2]]");
    assert!(call("TO_VECTOR", &[Value::Null]).unwrap().is_null());
    assert!(call("TO_VECTOR", &[text("[1]"), Value::Null, text("FLOAT8")]).is_err());
    let converted = call("TO_VECTOR", &[v, Value::Null, text("FLOAT64")]).unwrap();
    assert_eq!(vector_of(&converted).unwrap().element, Element::Float64);
}

/// Verifies: [EXTENSION] §5.2 FROM_VECTOR writes the text form, dense or
/// sparse, a character string past its limit SQLSTATE 22001;
/// FROM_VECTOR_CLOB writes a large object.
#[test]
fn text_forms_are_written() {
    let s = call("TO_VECTOR", &[text("[1,0,2]"), int(3), text("*"), text("SPARSE")]).unwrap();
    assert_eq!(chars(call("FROM_VECTOR", std::slice::from_ref(&s)).unwrap()), "[3,[0,2],[1,2]]");
    assert_eq!(chars(call("FROM_VECTOR", &[s, text("dense")]).unwrap()), "[1,0,2]");
    assert!(matches!(call("FROM_VECTOR_CLOB", &[dense(Element::Int8, &[1.0_f64])]).unwrap(), Value::Lob(_)));
    let long = Vector::dense(Element::Float64, vec![0.123_456_789_012_345_6_f64; 4000]).unwrap().value().unwrap();
    assert_eq!(call("FROM_VECTOR", &[long]).unwrap_err().sqlstate().as_str(), "22001");
}

/// Verifies: [EXTENSION] §6.5 arithmetic: element-wise sums, differences
/// and products of the wider format (R6.9), sparse with sparse staying
/// sparse (R6.10), scaling and division, division by zero SQLSTATE
/// 22012, overflow SQLSTATE 22003 (§6.6), BINARY refused.
#[test]
fn arithmetic_is_element_wise() {
    let (a, b) = (dense(Element::Int8, &[1.0_f64, 2.0_f64]), dense(Element::Float16, &[0.5_f64, -1.0_f64]));
    let sum = call("VECTOR_ADD", &[a.clone(), b.clone()]).unwrap();
    assert_eq!((VECTOR.text(&sum), vector_of(&sum).unwrap().element), ("[1.5,1]".to_owned(), Element::Float32));
    assert_eq!(VECTOR.text(&call("VECTOR_SUBTRACT", &[a.clone(), b.clone()]).unwrap()), "[0.5,3]");
    assert_eq!(VECTOR.text(&call("VECTOR_MULTIPLY", &[a.clone(), b.clone()]).unwrap()), "[0.5,-2]");
    assert_eq!(VECTOR.text(&call("VECTOR_SCALE", &[a.clone(), Value::Approx(1.5)]).unwrap()), "[1.5,3]");
    assert_eq!(VECTOR.text(&call("VECTOR_DIVIDE", &[a.clone(), Value::Approx(4.0)]).unwrap()), "[0.25,0.5]");
    assert_eq!(call("VECTOR_DIVIDE", &[a.clone(), Value::Approx(0.0)]).unwrap_err().sqlstate().as_str(), "22012");
    let big = dense(Element::Float16, &[60_000.0_f64]);
    assert_eq!(call("VECTOR_ADD", &[big.clone(), big]).unwrap_err().sqlstate().as_str(), "22003");
    let (s, t) = (sparse(Element::Float32, 9, &[(1, 1.0_f64)]), sparse(Element::Float32, 9, &[(1, -1.0_f64), (5, 2.0_f64)]));
    assert_eq!(VECTOR.text(&call("VECTOR_ADD", &[s.clone(), t]).unwrap()), "[9,[5],[2]]");
    assert!(call("VECTOR_ADD", &[a, s]).is_err());
    let bits = dense(Element::Binary, &[1.0_f64; 8]);
    assert!(call("VECTOR_ADD", &[bits.clone(), bits]).is_err());
}

/// Verifies: [EXTENSION] §7.1: VECTOR_QUANTIZE to INT8 with a given or
/// the vector's own symmetric scale, clamped; to BINARY by a threshold;
/// to FLOAT16; VECTOR_DEQUANTIZE back to FLOAT32.
#[test]
fn quantization_functions() {
    let v = dense(Element::Float32, &[-1.0_f64, 0.5_f64, 1.0_f64]);
    let own = call("VECTOR_QUANTIZE", &[v.clone(), text("INT8")]).unwrap();
    assert_eq!(VECTOR.text(&own), "[-127,64,127]");
    let given = call("VECTOR_QUANTIZE", &[v.clone(), text("int8"), Value::Approx(0.001), Value::Approx(1.0)]).unwrap();
    assert_eq!(VECTOR.text(&given), "[-128,127,127]");
    let bits = call(
        "VECTOR_QUANTIZE",
        &[dense(Element::Float32, &[1.0_f64, -1.0_f64, 0.2_f64, 0.0_f64, 3.0_f64, -2.0_f64, 0.0_f64, 9.0_f64]), text("BINARY")],
    )
    .unwrap();
    assert_eq!(VECTOR.text(&bits), "[169]");
    let thresholded = call("VECTOR_QUANTIZE", &[dense(Element::Float32, &[0.5_f64; 8]), text("BINARY"), Value::Approx(0.6)]).unwrap();
    assert_eq!(VECTOR.text(&thresholded), "[0]");
    let half = call("VECTOR_QUANTIZE", &[v.clone(), text("FLOAT16")]).unwrap();
    assert_eq!(vector_of(&half).unwrap().element, Element::Float16);
    let back = call("VECTOR_DEQUANTIZE", &[own, Value::Approx(1.0 / 127.0), Value::Approx(0.0)]).unwrap();
    assert_eq!(vector_of(&back).unwrap().element, Element::Float32);
    assert!((vector_of(&back).unwrap().at(1).unwrap() - 64.0 / 127.0).abs() < 1e-6_f64);
    assert!(call("VECTOR_QUANTIZE", &[v.clone(), text("FLOAT64")]).is_err());
    assert!(call("VECTOR_QUANTIZE", &[v, text("INT8"), Value::Approx(-1.0)]).is_err());
}

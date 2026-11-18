//! Unit tests of VECTOR's text forms.

use super::*;
use crate::vector::params::OPEN;
use crate::vector::text::{Parsed, number, parse, vector as read_text, write_as};

fn read(text: &str, params: &[u32]) -> Result<Value> {
    VECTOR.cast(Value::Char(text.into()), &typed_of(params))
}

/// Verifies: [EXTENSION] §5.1's text forms: a JSON array with exponents,
/// a sparse `[dims, [indexes], [values]]`, the JSON object with members
/// in any order, and BINARY as a byte per 8 dimensions.
#[test]
fn text_forms_are_read() {
    assert_eq!(parse(" [0.12, -0.5, 3.0E-2] ").unwrap(), Parsed::Dense(vec![0.12_f64, -0.5_f64, 0.03_f64]));
    assert_eq!(parse("[30522, [17, 2048], [0.4, 1.2]]").unwrap(), Parsed::Sparse(30522, vec![(17, 0.4_f64), (2048, 1.2_f64)]));
    let object = r#"{"values": [0.4], "dims": 10, "indices": [3]}"#;
    assert_eq!(parse(object).unwrap(), Parsed::Sparse(10, vec![(3, 0.4_f64)]));
    assert_eq!(parse("[]").unwrap(), Parsed::Dense(vec![]));
    let bits = read("[7, 200]", &[16, 5]).unwrap();
    assert_eq!(VECTOR.text(&bits), "[7,200]");
    assert_eq!(vector_of(&bits).unwrap().at(5), Some(1.0_f64));
    assert_eq!(vector_of(&bits).unwrap().at(0), Some(0.0_f64));
    let s = read("[5,[1,3],[2,4]]", &[5, 1 + LAYOUT_STEP]).unwrap();
    assert_eq!(vector_of(&s).unwrap().layout(), Layout::Sparse);
    let densified = read("[5,[1,3],[2,4]]", &[5]).unwrap();
    assert_eq!(VECTOR.text(&densified), "[0,2,0,4,0]");
    let open = read_text("[5,[1],[2]]", OPEN).unwrap();
    assert_eq!(open.layout(), Layout::Sparse);
}

/// Verifies: [EXTENSION] VEC-0005, VEC-0007, VEC-0003, VEC-0006 and
/// VEC-0016 on input. Malformed text is SQLCODE -301 SQLSTATE 22018;
/// NaN as a word is malformed; a number past the range is 22003; a
/// dimension count not the column's is 22000; INT8 out of range or not
/// an integer is 22003; sparse indexes out of order are 22018.
#[test]
fn bad_text_is_refused_with_its_sqlstate() {
    let state = |text: &str, params: &[u32]| {
        let err = read(text, params).unwrap_err();
        assert_eq!(err.sqlcode(), -301_i32, "{text}");
        err.sqlstate().as_str().to_owned()
    };
    for malformed in ["", "[1,", "[1 2]", "1,2", "[1,2]x", "[a]", "[NaN]", "{\"dims\": 3}", "{\"other\": 1}", "[3,[1],[1,2]]"] {
        assert_eq!(state(malformed, &[]), "22018", "{malformed}");
    }
    assert_eq!(state("[1e400]", &[]), "22003");
    assert_eq!(state("[1,2]", &[3]), "22000");
    assert_eq!(state("[128]", &[1, 4]), "22003");
    assert_eq!(state("[0.5]", &[1, 4]), "22003");
    assert_eq!(state("[256]", &[8, 5]), "22003");
    assert_eq!(state("[4,[2,1],[1,1]]", &[]), "22018");
    assert_eq!(state("[4,[4],[1]]", &[]), "22018");
    assert_eq!(state("[70000]", &[1, 2]), "22003");
}

/// Verifies: [EXTENSION] R5.6: numbers are written with the fewest digits
/// that read back as the same element of the vector's format, exponents
/// only for very small or large magnitudes.
#[test]
fn text_output_is_shortest_round_trip() {
    assert_eq!(number(0.1, Element::Float32), "0.1");
    assert_eq!(number(f64::from(single(0.1)), Element::Float32), "0.1");
    assert_eq!(number(0.1, Element::Float64), "0.1");
    assert_eq!(number(f64::from(half(half_bits(0.1))), Element::Float16), "0.1");
    assert_eq!(number(1e-7, Element::Float32), "1e-7");
    assert_eq!(number(3e20, Element::Float64), "3e20");
    assert_eq!(number(-2.0, Element::Int8), "-2");
    assert_eq!(number(65504.0, Element::Float16), "65500");
    for x in [0.1_f64, 1.0_f64 / 3.0_f64, -12_345.678_f64, 6.1e-5_f64] {
        let text = number(x, Element::Float16);
        let back: f64 = text.parse().unwrap();
        assert_eq!(half_bits(back), half_bits(x), "{text}");
    }
}

/// Verifies: [EXTENSION] R5.6 a vector's text: a dense JSON array, a
/// sparse `[dims,[indexes],[values]]`, either written in the other
/// layout's form, and read back.
#[test]
fn vectors_are_written_as_text() {
    let v = dense(Element::Float32, &[0.1_f64, 1.0_f64, -2.5_f64]);
    assert_eq!(VECTOR.text(&v), "[0.1,1,-2.5]");
    let s = sparse(Element::Int8, 6, &[(0, 3.0_f64), (5, -1.0_f64)]);
    assert_eq!(VECTOR.text(&s), "[6,[0,5],[3,-1]]");
    let view = vector_of(&s).unwrap();
    assert_eq!(write_as(view, Layout::Dense), "[3,0,0,0,0,-1]");
    assert_eq!(write_as(vector_of(&dense(Element::Float32, &[0.0_f64, 2.0_f64])).unwrap(), Layout::Sparse), "[2,[1],[2]]");
    assert_eq!(read(&VECTOR.text(&v), &[]).unwrap().kind_name(), NAME);
}

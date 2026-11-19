//! The well-known text, binary and GML of ST_Vector values, and the
//! representations that are none of them.

use super::super::values::Vector;
use super::super::vector_formats::{read_binary, read_gml, read_text, write_binary, write_text};
use crate::error::SqlState;

const BAD_TEXT: &str = "2FF22";
const BAD_WKB: &str = "2FF23";

fn text_state(text: &str) -> String {
    read_text(text, 0_i32).unwrap_err().sqlstate().as_str().to_owned()
}

fn binary_state(bytes: &[u8]) -> String {
    read_binary(bytes, 0_i32).unwrap_err().sqlstate().as_str().to_owned()
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §17.2.22] a text
/// that is no `<vector text representation>` is 2FF22: another key word,
/// no parentheses, a coordinate that is no number, or too many of them;
/// `VECTOR Z EMPTY` is the empty vector.
#[test]
fn vector_texts_that_are_none_17_2_22() {
    for text in ["POINT (1 2)", "VECTOR 1 2", "VECTOR (1 2", "VECTOR [1 2]", "VECTOR (1 x)", "VECTOR (1 2 3 4)", "VECTOR"] {
        assert_eq!(text_state(text), BAD_TEXT, "{text}");
    }
    assert_eq!(text_state("VECTOR (1 2 3)"), "2FF25");
    assert!(read_text("VECTOR Z EMPTY", 7_i32).unwrap().is_empty());
    assert_eq!(read_text("vector z (1 2 3)", 0_i32).unwrap().z, Some(3.0_f64));
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §17.2.1
/// Description 6, §17.2.12, §17.2.14] a vector of x without y, or y
/// without x, is not well formed (2FF02); a number too long to show
/// plainly is shown in exponent form, and reads back exactly.
#[test]
fn vectors_not_well_formed_are_not_written_17_2_1() {
    for v in [Vector { x: Some(1.0_f64), y: None, z: None, srid: 0_i32 }, Vector { x: None, y: Some(1.0_f64), z: None, srid: 0_i32 }] {
        assert_eq!(write_text(&v).unwrap_err().sqlstate(), SqlState::SPATIAL_INVALID_ARGUMENT);
        assert_eq!(write_binary(&v).unwrap_err().sqlstate(), SqlState::SPATIAL_INVALID_ARGUMENT);
    }
    let tiny = Vector { x: Some(1.234_567_890_123_456_7e-300_f64), y: Some(2.0_f64), z: None, srid: 0_i32 };
    let text = write_text(&tiny).unwrap();
    assert!(text.contains('e'), "{text}");
    assert_eq!(read_text(&text, 0_i32).unwrap(), tiny);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §17.2.23] bytes that
/// are no `<well-known binary representation>` of a vector are 2FF23: no
/// byte order, a type code of no vector, a length not the code's, or a
/// coordinate not finite; a header alone is the empty vector.
#[test]
fn vector_binaries_that_are_none_17_2_23() {
    let mut nan = vec![1_u8, 101, 0, 0, 0];
    nan.extend_from_slice(&f64::NAN.to_le_bytes());
    nan.extend_from_slice(&1.0_f64.to_le_bytes());
    for bytes in [vec![], vec![2_u8, 101, 0, 0, 0], vec![1_u8, 101, 0], vec![1_u8, 1, 0, 0, 0], vec![1_u8, 101, 0, 0, 0, 1], nan] {
        assert_eq!(binary_state(&bytes), BAD_WKB, "{bytes:?}");
    }
    assert!(read_binary(&[1_u8, 0x4D, 0x04, 0, 0], 0_i32).unwrap().is_empty());
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §17.2.15] the GML of
/// a vector is a vector element of two or three numbers (2FF24 else),
/// of the system its srsName names.
#[test]
fn vector_gml_is_read_17_2_15() {
    let v = read_gml("<gml:vector srsName=\"EPSG:4326\">1 2 3</gml:vector>", None, None).unwrap();
    assert_eq!(v, Vector { x: Some(1.0_f64), y: Some(2.0_f64), z: Some(3.0_f64), srid: 4326_i32 });
    assert_eq!(read_gml("<gml:vector>1 2</gml:vector>", Some(7_i32), None).unwrap().srid, 7_i32);
    for gml in ["<gml:point>1 2</gml:point>", "<gml:vector>1</gml:vector>", "<gml:vector>1 x</gml:vector>"] {
        assert_eq!(read_gml(gml, Some(0_i32), None).unwrap_err().sqlstate().as_str(), "2FF24", "{gml}");
    }
}

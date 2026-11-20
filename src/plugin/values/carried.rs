//! Unit tests: large objects carried to a program whole, as character
//! and binary strings, and a program's vectors made and read (RULES.md
//! D167, R-TEST-1).

use super::*;
use crate::plugin::errors::VIOLATION;
use crate::value::lob::LobKind;
use crate::value::lob::fixtures::{source, texts};

/// The value `v` crosses to a program as.
fn crossed(v: &Value) -> Value {
    let mut out = Vec::new();
    put_value(&mut out, v).unwrap();
    value(&mut Reader::new(&out, "test")).unwrap()
}

/// Verifies: [EXTENSION] RULES.md D167: a character large object crosses
/// to a program as the character string of all its chunks, and a binary
/// one as the binary string of all its octets.
#[test]
fn large_objects_cross_whole() {
    let Value::Char(text) = crossed(&Value::Lob(texts(&["ab", "cd", "é"]))) else { panic!("a character string") };
    assert_eq!(text, "abcdé");
    let Value::Binary(octets) = crossed(&Value::Lob(source(vec![vec![1, 2], vec![3]], LobKind::Binary))) else { panic!("a binary string") };
    assert_eq!(octets.bytes(), [1, 2, 3]);
}

/// Verifies: [EXTENSION] RULES.md D167, D221: a vector a program makes of
/// its coordinates gives them back, a value no vector gives none, and a
/// vector of no coordinates is refused, SQLSTATE 38T03.
#[test]
fn a_programs_vectors_are_made_and_read() {
    let given: Vec<f64> = vec![1.5, -2.0, 0.0];
    let v = dense_vector(given.clone()).unwrap();
    assert_eq!(coordinates(&v), Some(given.clone()));
    assert_eq!(coordinates(&crossed(&v)), Some(given), "as it crosses");
    assert_eq!(coordinates(&Value::Char("1".into())), None);
    assert_eq!(dense_vector(Vec::new()).unwrap_err().sqlstate(), VIOLATION);
}

//! Unit tests of column chunks (`columns.rs`).

use super::*;
use crate::value::{Decimal, Spelled};

fn int(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

const INTEGER: DataType = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer };
const TEXT: DataType = DataType::Varying { length: 100 };

/// The values of the chunk of all of `values`, read back.
fn round_trip(t: &DataType, values: &[Value]) -> Vec<String> {
    let fields = Fields::of(t, values.iter().cloned()).unwrap();
    let bytes = fields.chunk(t, 0, fields.len()).unwrap();
    let chunk = Chunk::parse(&bytes).unwrap();
    assert_eq!(chunk.len(), values.len());
    (0..chunk.len()).map(|n| chunk.value(n, t).unwrap().to_string()).collect()
}

/// Verifies: [DESIGN] RULES.md D31, column chunks: values of a type of a
/// fixed width in a record, and of one of variable width, read back as
/// written, nulls among them, each found alone; nine values, so the nulls
/// take a second byte.
#[test]
fn chunks_read_back() {
    let numbers: Vec<Value> = (0..9).map(|n| if n % 4 == 1 { Value::Null } else { int(n * 1000 - 3) }).collect();
    let expected: Vec<String> = numbers.iter().map(ToString::to_string).collect();
    assert_eq!(round_trip(&INTEGER, &numbers), expected);
    let texts: Vec<Value> = ["", "a", "bc", "", "def"].iter().map(|s| Value::Char((*s).to_owned())).chain([Value::Null]).collect();
    assert_eq!(round_trip(&TEXT, &texts), ["", "a", "bc", "", "def", "NULL"]);
    let fields = Fields::of(&TEXT, texts.into_iter()).unwrap();
    let bytes = fields.chunk(&TEXT, 0, fields.len()).unwrap();
    assert!(Chunk::parse(&bytes).unwrap().value(5, &TEXT).unwrap().is_null(), "a null is null, not empty");
    assert_eq!(bytes.first(), Some(&VARYING));
    let fixed = Fields::of(&INTEGER, [int(1)].into_iter()).unwrap().chunk(&INTEGER, 0, 1).unwrap();
    assert_eq!(fixed.first(), Some(&FIXED));
}

/// Verifies: [DESIGN] RULES.md D31, column chunks: as many values fit a
/// chunk as its capacity holds, from where asked, and at least one, so a
/// value longer than a chunk has one of its own; a chunk of a part of the
/// values holds that part.
#[test]
fn values_fit_their_chunks() {
    let fields = Fields::of(&INTEGER, (0..100).map(int)).unwrap();
    let width = INTEGER.kind().record_width(&INTEGER).unwrap();
    let capacity = HEADER + WORD + 2 + 10 * width;
    assert_eq!(fields.fitting(&INTEGER, 0, capacity), 10);
    assert_eq!(fields.fitting(&INTEGER, 95, capacity), 5, "the rest");
    let chunk = fields.chunk(&INTEGER, 20, 10).unwrap();
    assert!(chunk.len() <= capacity);
    assert_eq!(Chunk::parse(&chunk).unwrap().value(0, &INTEGER).unwrap().to_string(), "20");
    let long = Fields::of(&TEXT, [Value::Char("x".repeat(90)), Value::Char("y".into())].into_iter()).unwrap();
    assert_eq!(long.fitting(&TEXT, 0, 16), 1, "at least one");
    assert!(!long.is_empty());
}

/// Verifies: [DESIGN] RULES.md D31, column chunks: bytes that are not a
/// chunk are refused with SQLCODE -916: empty, of another kind, too short
/// for their count, nulls or values; and a value past the end is.
#[test]
fn malformed_chunks_are_refused() {
    let code = |bytes: &[u8]| Chunk::parse(bytes).err().map(|e| e.code);
    assert_eq!(code(b""), Some(Code::Corrupt));
    assert_eq!(code(&[9, 1, 0, 0, 0, 0]), Some(Code::Corrupt), "another kind");
    assert_eq!(code(&[FIXED, 1, 0]), Some(Code::Corrupt), "no count");
    assert_eq!(code(&[FIXED, 9, 0, 0, 0]), Some(Code::Corrupt), "no nulls");
    assert_eq!(code(&[FIXED, 1, 0, 0, 0, 0, 8, 0, 0, 0, 1]), Some(Code::Corrupt), "a value too short");
    assert_eq!(code(&[VARYING, 1, 0, 0, 0, 0, 0, 0]), Some(Code::Corrupt), "offsets too short");
    let bytes = Fields::of(&INTEGER, [int(1)].into_iter()).unwrap().chunk(&INTEGER, 0, 1).unwrap();
    let chunk = Chunk::parse(&bytes).unwrap();
    assert!(!chunk.is_empty());
    assert_eq!(chunk.value(1, &INTEGER).err().map(|e| e.code), Some(Code::Corrupt));
}

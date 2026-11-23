//! Unit tests of the octet LIKE function (`octet_like.rs`): its operands
//! of binary large objects, and its refusals.

use super::*;
use crate::value::Binary;
use crate::value::lob::Lob;

fn octets(b: &[u8]) -> Value {
    Value::Binary(Binary::of(b))
}

fn blob(b: &[u8]) -> Value {
    Value::Lob(Lob::of_octets(b))
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.5 GR4] [DESIGN] RULES.md D79, an
/// octet LIKE of binary large objects: a match value, pattern and escape
/// octet each a BLOB match as binary strings do, one if they match, zero
/// if not, null if one is null; the function shows its name and is of a
/// SMALLINT.
#[test]
fn large_objects_match_as_binary_strings() {
    let like = |args: &[Value]| OctetLike.call(args).unwrap().to_string();
    assert_eq!(like(&[blob(b"ab%c"), blob(b"ab!%%"), blob(b"!")]), "1");
    assert_eq!(like(&[blob(b"abxc"), blob(b"ab!%%"), blob(b"!")]), "0");
    assert_eq!(like(&[octets(b"abc"), blob(b"a%")]), "1");
    assert_eq!(like(&[octets(b"abc"), Value::Null]), "NULL");
    assert_eq!(OctetLike.name(), "OCTET LIKE");
    assert_eq!(OctetLike.return_type(&[]).unwrap(), DataType::Exact { precision: 1, scale: 0, spelled: Spelled::Smallint });
}

/// Verifies: [DESIGN] RULES.md D79, a pattern of a large object is held
/// whole up to a chunk: a longer one is refused with SQLCODE -900; an
/// operand not a binary string, or a call without a pattern, is a fault
/// of the binding, SQLCODE -910.
#[test]
fn a_pattern_past_a_chunk_is_refused() {
    let long = blob(&vec![b'%'; CHUNK_OCTETS.saturating_add(1)]);
    let e = OctetLike.call(&[octets(b"a"), long]).unwrap_err();
    assert!(e.code == Code::Unsupported && e.to_string().contains("a LIKE pattern"), "{e}");
    let text = Value::Char("a".into());
    assert_eq!(OctetLike.call(&[octets(b"a"), text.clone()]).unwrap_err().code, Code::Api);
    assert_eq!(OctetLike.call(&[text, octets(b"a")]).unwrap_err().code, Code::Api);
    assert_eq!(OctetLike.call(&[octets(b"a")]).unwrap_err().code, Code::Api);
}

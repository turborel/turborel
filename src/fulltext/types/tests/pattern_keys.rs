//! Unit tests of FT_Pattern as a column's type: its index keys, groups,
//! assignment and description.

use super::*;
use crate::value::types::Descriptor;

/// Verifies: [DESIGN] RULES.md D15, an FT_Pattern value as an index key:
/// its key reads back to an equal value, keys order as the values do, a
/// value is its own anchor, and a value of another type is refused, as a
/// key and as a record (SQLCODE -910); a key of no FT_Pattern payload is
/// corrupt (SQLCODE -902).
#[test]
fn ft_pattern_keys_read_back_and_order() {
    let t = FtPatternType::data_type();
    let (a, b) = (ft_pattern("\"a\"").unwrap(), ft_pattern("\"b\"").unwrap());
    let key = |v: &Value| {
        let mut out = Vec::new();
        FT_PATTERN_TYPE.encode_key(v, &t, &mut out).unwrap();
        out
    };
    assert!(key(&a) < key(&b));
    let whole = key(&a);
    let mut rest = whole.as_slice();
    assert_eq!(FT_PATTERN_TYPE.compare(&FT_PATTERN_TYPE.decode_key(&mut rest, &t).unwrap(), &a), Some(Ordering::Equal));
    assert_eq!(rest, b"");
    let (anchor, inclusive) = FT_PATTERN_TYPE.anchor(&a, &t).unwrap();
    assert!(inclusive && FT_PATTERN_TYPE.compare(&anchor, &a) == Some(Ordering::Equal));
    assert!(FT_PATTERN_TYPE.anchor(&text("a"), &t).is_none());
    assert_eq!(FT_PATTERN_TYPE.encode_key(&text("a"), &t, &mut Vec::new()).unwrap_err().code, Code::Api);
    assert_eq!(FT_PATTERN_TYPE.encode_record(&text("a"), &t).unwrap_err().code, Code::Api);
    let mut bad = Vec::new();
    put_key(&[9, b'x'], &mut bad);
    assert_eq!(FT_PATTERN_TYPE.decode_key(&mut bad.as_slice(), &t).unwrap_err().code, Code::Corrupt);
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.3.1] FT_Pattern as a column's
/// type: it is written FT_PATTERN and described by that name; equal
/// values are of one group and others of another; it has no position on a
/// number line; a pattern or the null value is assigned to it as it is,
/// a value of another type refused, 22 class.
#[test]
fn ft_pattern_columns_group_assign_and_describe() {
    let t = FtPatternType::data_type();
    assert_eq!(FT_PATTERN_TYPE.written(&t), "FT_PATTERN");
    assert_eq!(FT_PATTERN_TYPE.descriptor(&t), Descriptor::of("FT_PATTERN"));
    let (a, b) = (ft_pattern("x").unwrap(), ft_pattern("y").unwrap());
    assert_eq!(FT_PATTERN_TYPE.bucket(&a), FT_PATTERN_TYPE.bucket(&ft_pattern("x").unwrap()));
    assert_ne!(FT_PATTERN_TYPE.bucket(&a), FT_PATTERN_TYPE.bucket(&b));
    assert!(FT_PATTERN_TYPE.bucket(&text("x")).is_none());
    assert!(FT_PATTERN_TYPE.position(&a).is_none());
    assert_eq!(FT_PATTERN_TYPE.text(&FT_PATTERN_TYPE.assign(a, &t, "R").unwrap()), "x");
    assert!(FT_PATTERN_TYPE.assign(Value::Null, &t, "R").unwrap().is_null());
    let e = FT_PATTERN_TYPE.assign(english("x"), &t, "R").unwrap_err();
    assert!(e.code == Code::Data && e.to_string().contains("not an FT_PATTERN value"), "{e}");
}

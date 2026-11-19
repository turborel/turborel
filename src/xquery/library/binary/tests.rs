//! Tests of the operators on binary values and NOTATION.

use super::*;

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §12.1.1,
/// §12.1.2] binary values are equal of the same octets, whatever case or
/// spaces their lexical forms have; two of different types, or another
/// type, err:XPTY0004.
#[test]
fn binary_equality() {
    let hex = |t: &str| Atomic::new(AtomicType::HexBinary, t);
    let b64 = |t: &str| Atomic::new(AtomicType::Base64Binary, t);
    assert!(hex_binary_equal(&hex("0FB7"), &hex("0fb7")).unwrap());
    assert!(!hex_binary_equal(&hex("0FB7"), &hex("0FB8")).unwrap());
    assert!(!hex_binary_equal(&hex("0FB7"), &hex("0FB700")).unwrap());
    assert!(base64_binary_equal(&b64("AQID"), &b64("AQ ID")).unwrap());
    assert!(!base64_binary_equal(&b64("AQID"), &b64("AQIE")).unwrap());
    assert!(hex_binary_equal(&hex("01"), &b64("AQ==")).is_err());
    assert!(base64_binary_equal(&Atomic::new(AtomicType::String, "AQID"), &b64("AQID")).is_err());
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR3] [XQueryFO §13.1.1]
/// NOTATION values are equal code point by code point; another type
/// err:XPTY0004.
#[test]
fn notation_equality() {
    let n = |uri: Option<&str>, t: &str| Atomic { kind: AtomicType::Notation, ..Atomic::qname(uri, t) };
    assert!(notation_equal(&n(None, "gif"), &n(None, "gif")).unwrap());
    assert!(!notation_equal(&n(None, "gif"), &n(None, "GIF")).unwrap());
    assert!(!notation_equal(&n(Some("urn:a"), "gif"), &n(None, "gif")).unwrap());
    assert!(notation_equal(&n(None, "gif"), &Atomic::qname(None, "gif")).is_err());
}

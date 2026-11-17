//! Tests of the mapping of SQL values to XML.

use super::*;
use crate::value::interval::Qualifier;
use crate::value::{Binary, Decimal};

/// Verifies: [ISO/IEC 9075-14:2006 draft §9.8 GR9b] [Schema2 §3.2.15,
/// §3.2.16] a binary string in hex, upper case, or in base64, padded to
/// whole groups.
#[test]
fn binary_strings_in_hex_or_base64() {
    let v = |o: &[u8]| Value::Binary(Binary::new(o.to_vec()));
    assert_eq!(xml_lexical(&v(&[0x0F, 0xB7]), BinaryEncoding::Hex, true).unwrap(), "0FB7");
    let cases: [(&[u8], &str); 4] = [(b"", ""), (b"M", "TQ=="), (b"Ma", "TWE="), (b"Man", "TWFu")];
    for (octets, expected) in cases {
        assert_eq!(xml_lexical(&v(octets), BinaryEncoding::Base64, true).unwrap(), expected);
    }
    assert_eq!(BinaryEncoding::Base64.encode(&[0xFF, 0xFE, 0xFD, 0x00]), "//79AA==");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §9.8 GR9a, GR9c, GR9d, GR9h] a
/// string's characters, mapped if CHARMAPPING is True; a character no XML
/// 1.1 character, 0N002; a number as CAST writes it; a truth value in
/// lower case; an interval as a duration, its leading field of its
/// precision's digits.
#[test]
fn values_as_text() {
    let text = Value::Char("a<b>&\r".to_owned());
    assert_eq!(xml_lexical(&text, BinaryEncoding::Hex, true).unwrap(), "a&lt;b&gt;&amp;&#x0d;");
    assert_eq!(xml_lexical(&text, BinaryEncoding::Hex, false).unwrap(), "a<b>&\r");
    let bad = xml_lexical(&Value::Char("\u{FFFF}".to_owned()), BinaryEncoding::Hex, true).unwrap_err();
    assert_eq!(bad.sqlstate(), SqlState::INVALID_XML_CHARACTER);
    assert_eq!(xml_lexical(&Value::Exact(Decimal::new(-150, 2)), BinaryEncoding::Hex, true).unwrap(), "-1.50");
    assert_eq!(xml_lexical(&Value::Bool(false), BinaryEncoding::Hex, true).unwrap(), "false");
    let q = Qualifier::new(Field::Hour, Field::Second, 3, 2);
    let i = Interval { units: -(3_600_000_000 * 25 + 61_500_000), qualifier: q };
    assert_eq!(xml_lexical(&Value::Interval(i), BinaryEncoding::Hex, true).unwrap(), "-PT025H01M01.50S");
    assert!(xml_lexical(&Value::Null, BinaryEncoding::Hex, true).is_err());
}

//! Tests of the mapping of SQL values to XML.

use super::*;
use crate::value::interval::Qualifier;
use crate::value::types::Field as RowField;
use crate::value::{Binary, Decimal};

/// Verifies: [ISO/IEC 9075-14:2006 draft §9.8 GR9b] [Schema2 §3.2.15,
/// §3.2.16] a binary string in hex, upper case, or in base64, padded to
/// whole groups.
#[test]
fn binary_strings_in_hex_or_base64() {
    let v = |o: &[u8]| Value::Binary(Binary::of(o));
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
    let text = Value::Char("a<b>&\r".into());
    assert_eq!(xml_lexical(&text, BinaryEncoding::Hex, true).unwrap(), "a&lt;b&gt;&amp;&#x0d;");
    assert_eq!(xml_lexical(&text, BinaryEncoding::Hex, false).unwrap(), "a<b>&\r");
    let bad = xml_lexical(&Value::Char("\u{FFFF}".into()), BinaryEncoding::Hex, true).unwrap_err();
    assert_eq!(bad.sqlstate(), SqlState::INVALID_XML_CHARACTER);
    assert_eq!(xml_lexical(&Value::Exact(Decimal::new(-150, 2)), BinaryEncoding::Hex, true).unwrap(), "-1.50");
    assert_eq!(xml_lexical(&Value::Bool(false), BinaryEncoding::Hex, true).unwrap(), "false");
    let q = Qualifier::new(Field::Hour, Field::Second, 3, 2);
    let i = Interval { units: -(3_600_000_000 * 25 + 61_500_000), qualifier: q };
    assert_eq!(xml_lexical(&Value::Interval(i), BinaryEncoding::Hex, true).unwrap(), "-PT025H01M01.50S");
    assert!(xml_lexical(&Value::Null, BinaryEncoding::Hex, true).is_err());
}

/// The mapping XMLELEMENT's content takes: hex, characters mapped, UTC.
const CONTENT: Mapping = Mapping { encoding: BinaryEncoding::Hex, charmapping: true, zone: 0, nil: false };

/// Verifies: [ISO/IEC 9075-14:2006 draft §9.8 GR9i, GR9j, GR9a] an array
/// an element `element` of each element, a null one nilled, its prefix
/// declared; a row inside one an element of each field not null, named by
/// §9.1 fully escaped, a null field absent; characters mapped as asked.
#[test]
fn collections_and_rows_9_8() {
    let int = DataType::Exact { precision: 10, scale: 0, spelled: crate::value::Spelled::Integer };
    let text = DataType::Varying { length: 9, units: CharUnits::default() };
    let fields = vec![
        RowField { name: "a b".into(), data_type: int.clone(), unnamed: false },
        RowField { name: "c".into(), data_type: text, unnamed: false },
    ];
    let t = DataType::array(DataType::row(fields), 3);
    let row = |n: i128, s: Option<&str>| {
        Value::Row(vec![Value::Exact(Decimal::new(n, 0)), s.map_or(Value::Null, |s| Value::Char(s.into()))].into())
    };
    let v = Value::Array(vec![row(1, Some("<&")), row(2, None), Value::Null].into());
    let nil = format!("<element xmlns:{XSI_PREFIX}=\"{XSI_NAMESPACE}\" {XSI_PREFIX}:nil=\"true\"/>");
    let expected = format!("<element><a_x0020_b>1</a_x0020_b><c>&lt;&amp;</c></element><element><a_x0020_b>2</a_x0020_b></element>{nil}");
    assert_eq!(xml_mapped(&v, &t, CONTENT).unwrap(), expected);
    let raw = Mapping { charmapping: false, ..CONTENT };
    assert!(xml_mapped(&v, &t, raw).unwrap().contains("<c><&</c>"));
    let ints = DataType::array(int, 2);
    assert_eq!(xml_mapped(&Value::Array(Vec::new().into()), &ints, CONTENT).unwrap(), "");
}

/// Verifies: [DESIGN] R-CF-6: values nested deeper than `MAX_MAPPED_DEPTH`
/// are refused, SQLCODE -900, not walked; as deep are mapped.
#[test]
fn mapping_is_bounded() {
    let int = DataType::Exact { precision: 10, scale: 0, spelled: crate::value::Spelled::Integer };
    let nest = |levels: usize| {
        (0..levels).fold((Value::Exact(Decimal::new(7, 0)), int.clone()), |(v, t), _| (Value::Array(vec![v].into()), DataType::array(t, 1)))
    };
    let (deep, deep_type) = nest(MAX_MAPPED_DEPTH);
    let mapped = xml_mapped(&deep, &deep_type, CONTENT).unwrap();
    assert!(mapped.contains(">7<"), "{mapped}");
    let (deeper, deeper_type) = nest(MAX_MAPPED_DEPTH.saturating_add(1));
    assert_eq!(xml_mapped(&deeper, &deeper_type, CONTENT).unwrap_err().code, Code::Unsupported);
}

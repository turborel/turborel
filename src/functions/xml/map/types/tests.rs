//! Tests of SQL data types mapped to XML Names and XML Schema data types.

use super::*;
use crate::value::xml::Typing;
use crate::xml::{Syntax, Whitespace, XS_NAMESPACE, parse};

/// The catalog the tests' names are of.
const CATALOG: &str = "TURBOREL";

/// An exact type of `precision` and `scale`, written as `spelled`.
fn exact_type(precision: u32, scale: u32, spelled: Spelled) -> DataType {
    DataType::Exact { precision, scale, spelled }
}

/// An interval type from `start` to `end`, of `leading` and `fraction`
/// digits.
fn interval(start: Field, end: Field, leading: u32, fraction: u32) -> DataType {
    DataType::Interval(Qualifier::new(start, end, leading, fraction))
}

/// Whether `definitions` are well-formed within a schema document that
/// declares `xs` and `xdt`, read as XML 1.0.
fn parses(definitions: &str) -> bool {
    let document = format!(r#"<xs:schema xmlns:xs="{XS_NAMESPACE}" xmlns:xdt="{XDT_NAMESPACE}">{definitions}</xs:schema>"#);
    parse(&document, Syntax::Document, Whitespace::Preserve).is_ok()
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §9.4 GR2 to GR17, GR20, GR23]
/// each predefined type's XML Name, of its length, precision and scale
/// as written; a distinct type's of "UDT", its catalog, schema and name
/// [§9.2]; an XML type's XML; and [EXTENSION] a binary string's and an
/// interval's, by analogy with BLOB's.
#[test]
fn xml_names_of_types_9_4() {
    let cases = [
        (DataType::character(10), "CHAR_10"),
        (DataType::varying(20), "VARCHAR_20"),
        (DataType::Character { length: 5, units: CharUnits::Octets }, "VARCHAR_5_5"),
        (DataType::Lob { kind: LobKind::Character, length: 100, units: CharUnits::Characters }, "CLOB_100"),
        (DataType::Lob { kind: LobKind::Binary, length: 64, units: CharUnits::Characters }, "BLOB_64"),
        (DataType::Binary { length: 4 }, "BINARY_4"),
        (DataType::VarBinary { length: 8 }, "VARBINARY_8"),
        (exact_type(7, 2, Spelled::Numeric), "NUMERIC_7_2"),
        (exact_type(7, 2, Spelled::Decimal), "DECIMAL_7_2"),
        (exact_type(10, 0, Spelled::Integer), "INTEGER"),
        (exact_type(5, 0, Spelled::Smallint), "SMALLINT"),
        (exact_type(18, 0, Spelled::Bigint), "BIGINT"),
        (DataType::Approximate { precision: 10, spelled: Spelled::Float }, "FLOAT_10"),
        (DataType::Approximate { precision: 24, spelled: Spelled::Real }, "REAL"),
        (DataType::Approximate { precision: 53, spelled: Spelled::DoublePrecision }, "DOUBLE"),
        (DataType::Boolean, "BOOLEAN"),
        (DataType::Date, "DATE"),
        (DataType::Time { precision: 3, zoned: false }, "TIME_3"),
        (DataType::Time { precision: 0, zoned: true }, "TIME_WTZ_0"),
        (DataType::Timestamp { precision: 6, zoned: false }, "TIMESTAMP_6"),
        (DataType::Timestamp { precision: 6, zoned: true }, "TIMESTAMP_WTZ_6"),
        (interval(Field::Year, Field::Month, 4, 0), "INTERVAL_YEAR_TO_MONTH_4"),
        (interval(Field::Day, Field::Second, 2, 6), "INTERVAL_DAY_TO_SECOND_2_6"),
        (interval(Field::Second, Field::Second, 3, 0), "INTERVAL_SECOND_3_0"),
        (DataType::Xml(XmlType::Content(Typing::Untyped)), "XML"),
        (DataType::distinct("S", "MONEY", exact_type(9, 2, Spelled::Numeric)), "UDT.TURBOREL.S.MONEY"),
    ];
    for (t, name) in cases {
        assert_eq!(type_name(&t, CATALOG).as_deref(), Some(name), "{t:?}");
        assert!(mappable(&t), "{t:?}");
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §4.10.8] an XML unmappable data
/// type, XML(SEQUENCE), has no XML Name, nor [EXTENSION] DECFLOAT, an
/// array, or a distinct type of one; [§9.4 GR18, §9.2] a domain's is of
/// "Domain", its catalog, schema and name, each fully escaped [§9.1].
#[test]
fn unmappable_types_and_domains_9_4() {
    let decfloat = DataType::DecFloat { precision: 16 };
    let array = DataType::array(DataType::Boolean, 3);
    for t in [DataType::Xml(XmlType::Sequence), decfloat, array.clone(), DataType::distinct("S", "A", array)] {
        assert!(!mappable(&t) && type_name(&t, CATALOG).is_none(), "{t:?}");
    }
    let domain = Declared::of(&DataType::Boolean, Some(("S", "A.B:C")));
    assert_eq!(declared_name(&domain, CATALOG).as_deref(), Some("Domain.TURBOREL.S.A_x002E_B_x003A_C"));
    let of_xml = Declared::of(&DataType::Xml(XmlType::Document(Typing::Any)), Some(("S", "D")));
    assert_eq!(of_xml, Declared::Type(DataType::Xml(XmlType::Document(Typing::Any))), "a domain of XML maps as XML");
    assert_eq!(Declared::of(&DataType::Boolean, None).data_type(), &DataType::Boolean);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §9.5 GR8a to GR8d, GR8f, §9.6
/// GR5, GR6] each type's XML Schema data type, named: a CHARACTER of a
/// length in characters exactly so long, other strings at most; a binary
/// string in hex or base64 \[GR8b ii\]; an exact number by its digits, or
/// of INTEGER by its range \[GR8d iv\]; an approximate one and BOOLEAN the
/// built-in type restricted by nothing.
#[test]
fn simple_types_9_5() {
    let named = |t: DataType, encoding| named_type(&Declared::Type(t), CATALOG, encoding).unwrap();
    let cases = [
        (
            DataType::character(10),
            r#"<xs:simpleType name="CHAR_10"><xs:restriction base="xs:string"><xs:length value="10"/></xs:restriction></xs:simpleType>"#,
        ),
        (
            DataType::Character { length: 3, units: CharUnits::Octets },
            r#"<xs:simpleType name="VARCHAR_3_3"><xs:restriction base="xs:string"><xs:maxLength value="3"/></xs:restriction></xs:simpleType>"#,
        ),
        (
            DataType::varying(20),
            r#"<xs:simpleType name="VARCHAR_20"><xs:restriction base="xs:string"><xs:maxLength value="20"/></xs:restriction></xs:simpleType>"#,
        ),
        (
            exact_type(7, 2, Spelled::Decimal),
            r#"<xs:simpleType name="DECIMAL_7_2"><xs:restriction base="xs:decimal"><xs:totalDigits value="7"/><xs:fractionDigits value="2"/></xs:restriction></xs:simpleType>"#,
        ),
        (
            exact_type(5, 0, Spelled::Smallint),
            r#"<xs:simpleType name="SMALLINT"><xs:restriction base="xs:integer"><xs:maxInclusive value="99999"/><xs:minInclusive value="-99999"/></xs:restriction></xs:simpleType>"#,
        ),
        (
            DataType::Approximate { precision: 24, spelled: Spelled::Real },
            r#"<xs:simpleType name="REAL"><xs:restriction base="xs:float"/></xs:simpleType>"#,
        ),
        (
            DataType::Approximate { precision: 53, spelled: Spelled::Float },
            r#"<xs:simpleType name="FLOAT_53"><xs:restriction base="xs:double"/></xs:simpleType>"#,
        ),
        (DataType::Boolean, r#"<xs:simpleType name="BOOLEAN"><xs:restriction base="xs:boolean"/></xs:simpleType>"#),
    ];
    for (t, expected) in cases {
        assert_eq!(named(t, BinaryEncoding::Hex), expected);
    }
    let blob = DataType::Lob { kind: LobKind::Binary, length: 9, units: CharUnits::Characters };
    let hex =
        r#"<xs:simpleType name="BLOB_9"><xs:restriction base="xs:hexBinary"><xs:maxLength value="9"/></xs:restriction></xs:simpleType>"#;
    assert_eq!(named(blob.clone(), BinaryEncoding::Hex), hex);
    assert_eq!(named(blob, BinaryEncoding::Base64), hex.replace("hexBinary", "base64Binary"));
    assert!(named_type(&Declared::Type(DataType::DecFloat { precision: 34 }), CATALOG, BinaryEncoding::Hex).is_none());
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §9.5 GR8g to GR8l] a datetime's
/// pattern of its fields, its fraction's digits and, WITH TIME ZONE, its
/// displacement; an interval's of `xdt`'s duration types, its fields
/// alone, the leading one of its precision's digits; of no fractional
/// seconds, no fraction, and of INTERVAL SECOND its leading precision's
/// digits, as §9.8 GR9h writes them.
#[test]
fn datetime_and_interval_patterns_9_5() {
    let restricted = |t: DataType| restriction(&t, BinaryEncoding::Hex).unwrap();
    assert_eq!(
        restricted(DataType::Date),
        r#"<xs:restriction base="xs:date"><xs:pattern value="\p{Nd}{4}-\p{Nd}{2}-\p{Nd}{2}"/></xs:restriction>"#
    );
    assert_eq!(
        restricted(DataType::Time { precision: 3, zoned: true }),
        r#"<xs:restriction base="xs:time"><xs:pattern value="\p{Nd}{2}:\p{Nd}{2}:\p{Nd}{2}.\p{Nd}{3}(+|-)\p{Nd}{2}:\p{Nd}{2}"/></xs:restriction>"#
    );
    assert_eq!(
        restricted(DataType::Timestamp { precision: 0, zoned: false }),
        r#"<xs:restriction base="xs:dateTime"><xs:pattern value="\p{Nd}{4}-\p{Nd}{2}-\p{Nd}{2}T\p{Nd}{2}:\p{Nd}{2}:\p{Nd}{2}"/></xs:restriction>"#
    );
    let cases = [
        (interval(Field::Year, Field::Month, 4, 0), "yearMonthDuration", r"-?P\p{Nd}{4}Y\p{Nd}{2}M"),
        (interval(Field::Month, Field::Month, 2, 0), "yearMonthDuration", r"-?P\p{Nd}{2}M"),
        (interval(Field::Day, Field::Second, 3, 6), "dayTimeDuration", r"-?P\p{Nd}{3}DT\p{Nd}{2}H\p{Nd}{2}M\p{Nd}{2}.\p{Nd}{6}S"),
        (interval(Field::Hour, Field::Minute, 2, 0), "dayTimeDuration", r"-?PT\p{Nd}{2}H\p{Nd}{2}M"),
        (interval(Field::Minute, Field::Second, 2, 0), "dayTimeDuration", r"-?PT\p{Nd}{2}M\p{Nd}{2}S"),
        (interval(Field::Second, Field::Second, 4, 2), "dayTimeDuration", r"-?PT\p{Nd}{4}.\p{Nd}{2}S"),
    ];
    for (t, base, pattern) in cases {
        assert_eq!(restricted(t), format!(r#"<xs:restriction base="xdt:{base}"><xs:pattern value="{pattern}"/></xs:restriction>"#));
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §9.5 GR8m, GR8o, GR8r, §9.6
/// GR6b, §9.7 GR3, GR6] a domain and a distinct type restrict their
/// data type and source type by XML Name, which the collection then
/// defines too, each name once, in order; an XML type is mixed content of
/// any element unvalidated; an interval's type is of `xdt`; every
/// definition is well-formed XML.
#[test]
fn collections_of_types_9_7() {
    let money = DataType::distinct("S", "MONEY", exact_type(9, 2, Spelled::Numeric));
    let declared = [
        Declared::of(&DataType::Boolean, Some(("S", "FLAG"))),
        Declared::Type(money),
        Declared::Type(DataType::Boolean),
        Declared::Type(DataType::Xml(XmlType::Content(Typing::Untyped))),
        Declared::Type(DataType::Xml(XmlType::Document(Typing::Any))),
    ];
    let (text, xdt) = definitions(&declared, CATALOG, BinaryEncoding::Hex);
    let expected = [
        r#"<xs:simpleType name="Domain.TURBOREL.S.FLAG"><xs:restriction base="BOOLEAN"/></xs:simpleType>"#,
        r#"<xs:simpleType name="BOOLEAN"><xs:restriction base="xs:boolean"/></xs:simpleType>"#,
        r#"<xs:simpleType name="UDT.TURBOREL.S.MONEY"><xs:restriction base="NUMERIC_9_2"/></xs:simpleType>"#,
        r#"<xs:simpleType name="NUMERIC_9_2"><xs:restriction base="xs:decimal"><xs:totalDigits value="9"/><xs:fractionDigits value="2"/></xs:restriction></xs:simpleType>"#,
        r#"<xs:complexType name="XML" mixed="true"><xs:sequence><xs:any minOccurs="0" maxOccurs="unbounded" processContents="skip"/></xs:sequence></xs:complexType>"#,
    ];
    assert_eq!(text, expected.concat());
    assert!(!xdt && parses(&text));
    let (durations, of_xdt) = definitions(&[Declared::Type(interval(Field::Day, Field::Day, 2, 0))], CATALOG, BinaryEncoding::Hex);
    assert!(of_xdt && parses(&durations), "{durations}");
    let unmapped = definitions(&[Declared::Type(DataType::Xml(XmlType::Sequence))], CATALOG, BinaryEncoding::Hex);
    assert_eq!(unmapped, (String::new(), false));
}

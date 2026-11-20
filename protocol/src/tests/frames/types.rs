//! The sample frames of every data type tag, each a Description of one
//! column C of the type.

use std::sync::Arc;

use turborel::value::interval::Qualifier;
use turborel::value::lob::LobKind;
use turborel::value::types::{Field as RowField, StructuredType, UdtName, XmlSchemaNamed};
use turborel::value::xml::{Typing, XmlType};
use turborel::value::{CharUnits, DataType, Spelled};

use super::{Sample, server};
use crate::capability::DECLARED_TYPES;
use crate::{Column, Reply};

/// The sample `name` of a Description of one nullable column C of type
/// `t`, its type's tag and parameters as `means` says.
fn typed(name: &'static str, t: DataType, means: &str) -> Sample {
    let column = Column { name: Some("C".into()), data_type: t, not_null: false, declared: None };
    server(name, &[], &Reply::Description(vec![column]), &format!("Description of one column C, nullable, {means}"))
}

/// The structured type S.SHAPE, under S.FIGURE.
fn shape() -> StructuredType {
    StructuredType { name: UdtName::new("S", "SHAPE"), supertypes: vec![UdtName::new("S", "FIGURE")] }
}

/// The samples of the predefined types' tags.
fn predefined() -> Vec<Sample> {
    vec![
        typed("description-character", DataType::character(4), "tag 1 CHARACTER, length 4, units 0 characters"),
        typed(
            "description-character-octets",
            DataType::Character { length: 16, units: CharUnits::Octets },
            "tag 1 CHARACTER, length 16, units 1 octets",
        ),
        typed(
            "description-exact",
            DataType::Exact { precision: 9, scale: 2, spelled: Spelled::Decimal },
            "tag 2 exact numeric, precision 9, scale 2",
        ),
        typed(
            "description-approximate",
            DataType::Approximate { precision: 53, spelled: Spelled::DoublePrecision },
            "tag 3 approximate numeric, precision 53",
        ),
        typed("description-plugin-type", turborel::geometry::data_type(), "tag 4 a plug-in's type, name ST_GEOMETRY, no parameters"),
        typed("description-varying", DataType::varying(20), "tag 5 CHARACTER VARYING, length 20, units 0 characters"),
        typed("description-binary", DataType::Binary { length: 16 }, "tag 6 BINARY, length 16"),
        typed("description-binary-varying", DataType::VarBinary { length: 255 }, "tag 7 BINARY VARYING, length 255"),
    ]
    .into_iter()
    .chain(datetimes())
    .collect()
}

/// The samples of the datetime, interval and later predefined types' tags.
fn datetimes() -> Vec<Sample> {
    use turborel::value::datetime::Field;
    vec![
        typed("description-date", DataType::Date, "tag 8 DATE, precision 0"),
        typed("description-time", DataType::Time { precision: 3, zoned: false }, "tag 9 TIME, precision 3"),
        typed("description-time-zoned", DataType::Time { precision: 0, zoned: true }, "tag 10 TIME WITH TIME ZONE, precision 0"),
        typed("description-timestamp", DataType::Timestamp { precision: 6, zoned: false }, "tag 11 TIMESTAMP, precision 6"),
        typed(
            "description-timestamp-zoned",
            DataType::Timestamp { precision: 6, zoned: true },
            "tag 12 TIMESTAMP WITH TIME ZONE, precision 6",
        ),
        typed(
            "description-interval-year-month",
            DataType::Interval(Qualifier::new(Field::Year, Field::Month, 4, 0)),
            "tag 13 INTERVAL YEAR(4) TO MONTH, start 0, end 1, leading 4, fractional 0",
        ),
        typed(
            "description-interval-day-second",
            DataType::Interval(Qualifier::new(Field::Day, Field::Second, 2, 6)),
            "tag 13 INTERVAL DAY(2) TO SECOND(6), start 2, end 5, leading 2, fractional 6",
        ),
        typed("description-boolean", DataType::Boolean, "tag 14 BOOLEAN"),
        typed("description-decfloat", DataType::DecFloat { precision: 34 }, "tag 15 DECFLOAT, precision 34"),
        typed(
            "description-clob",
            DataType::Lob { kind: LobKind::Character, length: 1_048_576, units: CharUnits::Characters },
            "tag 18 CHARACTER LARGE OBJECT, length 1048576, units 0 characters",
        ),
        typed(
            "description-blob",
            DataType::Lob { kind: LobKind::Binary, length: 65_536, units: CharUnits::Octets },
            "tag 19 BINARY LARGE OBJECT, length 65536, units 1",
        ),
        typed("description-json", DataType::Json, "tag 25 JSON"),
        typed("description-uuid", turborel::value::types::uuid::UuidType::data_type(), "tag 27 UUID"),
    ]
}

/// The samples of the constructed and user-defined types' tags.
fn constructed() -> Vec<Sample> {
    let field = |name: &str, data_type: DataType| RowField { name: name.into(), data_type, unnamed: false };
    let schema = Arc::new(XmlSchemaNamed { schema: "S".into(), name: "PO".into(), namespace: Some("urn:po".into()), element: None });
    vec![
        typed("description-array", DataType::array(DataType::Boolean, 3), "tag 16 array, maximum cardinality 3, element tag 14 BOOLEAN"),
        typed(
            "description-row",
            DataType::row(vec![field("X", DataType::varying(3)), field("Y", DataType::Boolean)]),
            "tag 17 row, two fields, X tag 5 CHARACTER VARYING(3), Y tag 14 BOOLEAN",
        ),
        typed("description-multiset", DataType::multiset(DataType::character(2)), "tag 22 multiset, element tag 1 CHARACTER(2)"),
        typed(
            "description-structured",
            DataType::Structured(Arc::new(shape())),
            "tag 23 structured type S.SHAPE, one proper supertype S.FIGURE",
        ),
        typed("description-reference", DataType::reference(shape(), None), "tag 24 reference to S.SHAPE, scoped 0"),
        typed(
            "description-reference-scoped",
            DataType::reference(shape(), Some(UdtName::new("S", "SHAPES"))),
            "tag 24 reference to S.SHAPE, scoped 1, scope S.SHAPES",
        ),
        typed(
            "description-xml-document",
            DataType::Xml(XmlType::Document(Typing::Untyped)),
            "tag 26 XML(DOCUMENT(UNTYPED)), primary 0, secondary 0",
        ),
        typed("description-xml-content", DataType::Xml(XmlType::Content(Typing::Any)), "tag 26 XML(CONTENT(ANY)), primary 1, secondary 1"),
        typed("description-xml-sequence", DataType::Xml(XmlType::Sequence), "tag 26 XML(SEQUENCE), primary 2, secondary 0"),
        typed(
            "description-xml-schema",
            DataType::Xml(XmlType::Document(Typing::Schema(schema))),
            "tag 26 XML(DOCUMENT(XMLSCHEMA)), primary 0, secondary 2, schema S.PO, namespace 1 urn:po, element 0",
        ),
        typed(
            "description-distinct",
            DataType::distinct("S", "MONEY", DataType::varying(4)),
            "a distinct type S.MONEY written as its source, tag 5 CHARACTER VARYING(4)",
        ),
    ]
}

/// A Description of a nested type with its declared type told.
fn declared() -> Sample {
    let t = DataType::array(DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer }, 3);
    let column = Column { name: Some("C".into()), data_type: t, not_null: false, declared: None }.with_declared();
    let means =
        "Description of one column C, tag 16 array of 3, element tag 2 exact numeric (10, 0), tail 19 declared ARRAY, INTEGER ARRAY[3]";
    server("description-declared-array", &[DECLARED_TYPES], &Reply::Description(vec![column]), means)
}

/// Every data type tag's samples.
pub(super) fn samples() -> Vec<Sample> {
    let mut all = predefined();
    all.extend(constructed());
    all.push(declared());
    all
}

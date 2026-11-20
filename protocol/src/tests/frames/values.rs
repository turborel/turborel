//! The sample frames of every value tag, each a Rows reply of one row of
//! the value, and the edges of the values a decoder meets.

use std::sync::Arc;

use turborel::geometry::Geometry;
use turborel::json::Keys;
use turborel::value::datetime::{Field, Moment};
use turborel::value::interval::{Interval, Qualifier};
use turborel::value::json::Json;
use turborel::value::lob::{Lob, LobKind};
use turborel::value::types::uuid::uuid_value;
use turborel::value::types::{Structured, UdtName};
use turborel::value::xml::XmlValue;
use turborel::value::{Binary, DecFloat, Decimal, Value};

use super::super::datetime;
use super::{Sample, server};
use crate::Reply;

/// The sample `name` of Rows of one row holding `v` alone, which `means`
/// says.
fn one(name: &'static str, v: Value, means: &str) -> Sample {
    server(name, &[], &Reply::Rows(vec![vec![v]]), &format!("Rows, one row of one value, {means}"))
}

/// The samples of the predefined values' tags.
fn predefined() -> Vec<Sample> {
    vec![
        one("value-null", Value::Null, "tag 0 null"),
        one("value-character", Value::Char("abc".into()), "tag 1 character string abc"),
        one("value-character-empty", Value::Char(String::new()), "tag 1 the empty string, length 0"),
        one("value-character-utf8", Value::Char("é€".into()), "tag 1 é€, 5 octets of UTF-8"),
        one("value-exact", Value::Exact(Decimal::new(12_345, 2)), "tag 2 exact number 123.45, units 12345, scale 2"),
        one("value-exact-negative", Value::Exact(Decimal::new(-1, 0)), "tag 2 exact number -1, units -1, scale 0"),
        one(
            "value-exact-largest",
            Value::Exact(Decimal::new(i128::MAX, 0)),
            "tag 2 exact number of the largest units an i128 holds, scale 0",
        ),
        one("value-approximate", Value::Approx(1.5), "tag 3 approximate number 1.5, the bits of the double"),
        one("value-approximate-negative-zero", Value::Approx(-0.0), "tag 3 approximate number -0, its sign bit set"),
        one(
            "value-plugin",
            Geometry::from_wkt("POINT (1 2)", 4326).unwrap().to_value(),
            "tag 4 a plug-in's value, type ST_GEOMETRY, text POINT (1 2), then its bytes",
        ),
        one("value-binary", Value::Binary(Binary::new(vec![0xDE, 0xAD, 0xBE, 0xEF])), "tag 6 binary string DEADBEEF"),
        one("value-binary-empty", Value::Binary(Binary::default()), "tag 6 binary string of no octets"),
    ]
    .into_iter()
    .chain(datetimes())
    .collect()
}

/// The samples of the datetime, interval, truth and decimal floating point values' tags.
fn datetimes() -> Vec<Sample> {
    vec![
        one("value-date", datetime(Moment::Date, "2024-02-29"), "tag 8 date 2024-02-29, its midnight's instant"),
        one("value-time", datetime(Moment::Time, "12:34:56.5"), "tag 9 time 12:34:56.5, zoned 0"),
        one(
            "value-time-zoned",
            datetime(Moment::Time, "12:34:56+05:30"),
            "tag 9 time 12:34:56+05:30, zoned 1, displacement 330, the instant in UTC",
        ),
        one(
            "value-timestamp",
            datetime(Moment::Timestamp, "2024-02-29 12:34:56.123456"),
            "tag 11 timestamp 2024-02-29 12:34:56.123456, zoned 0, precision 6",
        ),
        one(
            "value-timestamp-zoned",
            datetime(Moment::Timestamp, "2024-02-29 12:34:56-08:00"),
            "tag 11 timestamp 2024-02-29 12:34:56-08:00, zoned 1, displacement -480, the instant in UTC",
        ),
        one(
            "value-timestamp-earliest",
            datetime(Moment::Timestamp, "0001-01-01 00:00:00"),
            "tag 11 timestamp 0001-01-01 00:00:00, instant 0",
        ),
        one(
            "value-interval-year-month",
            Value::Interval(Interval { units: -14, qualifier: Qualifier::new(Field::Year, Field::Month, 2, 0) }),
            "tag 13 interval -1-2 YEAR TO MONTH, its qualifier, units -14 months",
        ),
        one(
            "value-interval-day-second",
            Value::Interval(Interval { units: 93_784_500_000, qualifier: Qualifier::new(Field::Day, Field::Second, 2, 6) }),
            "tag 13 interval 1 02:03:04.5 DAY TO SECOND, its qualifier, units 93784500000 microseconds",
        ),
        one("value-true", Value::Bool(true), "tag 14 truth value true"),
        one("value-false", Value::Bool(false), "tag 14 truth value false"),
        one(
            "value-decfloat",
            Value::DecFloat(DecFloat::of_decimal(&Decimal::new(-15, 1)).unwrap()),
            "tag 15 decimal floating point -1.5, coefficient -15, exponent -1",
        ),
    ]
}

/// The samples of the constructed, large object and user-defined values'
/// tags.
fn constructed() -> Vec<Sample> {
    let shape = Arc::new(UdtName::new("S", "SHAPE"));
    let attributes: Arc<[Value]> = vec![Value::Exact(Decimal::new(3, 0)), Value::Null].into();
    vec![
        one("value-array", Value::Array(vec![Value::Exact(Decimal::new(1, 0)), Value::Null].into()), "tag 16 array of two, 1 and null"),
        one("value-array-empty", Value::Array(Vec::new().into()), "tag 16 array of no elements"),
        one("value-row", Value::Row(vec![Value::Char("x".into()), Value::Bool(true)].into()), "tag 17 row of two fields, x and true"),
        one("value-lob-character", Value::Lob(Lob::of_text("aé")), "tag 20 large object whole, kind 0 character, the octets of aé"),
        one("value-lob-binary", Value::Lob(Lob::of_octets(&[0, 0xFF])), "tag 20 large object whole, kind 1 binary, octets 00 FF"),
        one(
            "value-lob-locator",
            Value::Lob(Lob::of_locator(LobKind::Character, 7, (300_000, 200_000))),
            "tag 21 large object by locator, kind 0, locator 7, 300000 octets, 200000 characters",
        ),
        one("value-multiset", Value::Multiset(vec![Value::Char("a".into()), Value::Char("a".into())].into()), "tag 22 multiset of a and a"),
        one(
            "value-structured",
            Value::Structured(Arc::new(Structured::new(shape, attributes))),
            "tag 23 structured value of S.SHAPE, attributes 3 and null",
        ),
        one("value-reference", Value::Reference(Binary::new(vec![1, 2, 3, 4, 5, 6, 7, 8])), "tag 24 reference, octets 0102030405060708"),
        one(
            "value-json",
            Value::Json(Json::parse(r#"{"a":[1,"x"]}"#, Keys::Any).unwrap()),
            r#"tag 25 JSON {"a":[1,"x"]}, its stored form of 51 octets, as 05-protocol.md lays it out"#,
        ),
        one(
            "value-json-scalars",
            Value::Json(Json::parse(r#"[null,true,false,-2,1.50,"",{}]"#, Keys::Any).unwrap()),
            r#"tag 25 JSON [null,true,false,-2,1.50,"",{}], nodes null, true, false, integer -2, decimal units 150 scale 2, an empty string, an empty object"#,
        ),
        one("value-xml", Value::Xml(XmlValue::serialized("<a>&lt;</a>")), "tag 26 XML, its serialization <a>&lt;</a>"),
        one(
            "value-uuid",
            uuid_value(std::array::from_fn(|i| u8::try_from(i.saturating_mul(17)).unwrap())),
            "tag 27 UUID 00112233-4455-6677-8899-aabbccddeeff, 16 octets",
        ),
    ]
}

/// Every value tag's samples.
pub(super) fn samples() -> Vec<Sample> {
    let mut all = predefined();
    all.extend(constructed());
    all
}

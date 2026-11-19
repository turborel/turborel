//! Unit tests of messages cut short: every prefix of a reply's body, each
//! field of it ending early, is refused as a connection failure, never
//! read as a shorter message (RULES.md R-TEST-2).

use std::sync::Arc;

use turborel::value::lob::{Lob, LobKind};
use turborel::value::types::{self, Field, Structured, UdtName};
use turborel::value::xml::XmlValue;
use turborel::value::{Binary, DataType, Decimal, Value};

use super::{FAILED, every_reply, kind_of, reply_round_trip, unframed};
use crate::*;

/// Columns of the types whose fields nest: an array, a row, a multiset,
/// a large object and a type beyond the standard with parameters.
fn nested_columns() -> Vec<Column> {
    let column = |name: &str, data_type: DataType| Column { name: Some(name.into()), data_type, not_null: true };
    let field = |name: &str, data_type: DataType| Field { name: name.into(), data_type, unnamed: false };
    vec![
        column("A", DataType::array(DataType::array(DataType::Boolean, 2), 3)),
        column("R", DataType::row(vec![field("X", DataType::character(1)), field("Y", DataType::varying(2))])),
        column("M", DataType::multiset(DataType::Date)),
        column("C", DataType::Lob { kind: LobKind::Character, length: 10, units: turborel::value::CharUnits::Octets }),
    ]
}

/// Rows of the values whose fields nest or are counted.
fn nested_rows() -> Vec<turborel::storage::Row> {
    let of = Arc::new(UdtName::new("S", "ADDRESS"));
    let structured = Structured::new(of, vec![Value::Char("x".into()), Value::Null].into());
    vec![
        vec![Value::Array(vec![Value::Bool(true), Value::Null].into()), Value::Row(vec![Value::Char("é".into())].into())],
        vec![Value::Multiset(vec![Value::Exact(Decimal::new(5, 1))].into()), Value::Xml(XmlValue::serialized("<b/>"))],
        vec![Value::Lob(Lob::of_text("ab")), Value::Lob(Lob::of_locator(LobKind::Binary, 7, (300, 300)))],
        vec![types::unregistered("POINT", "(1, 2)", &[1, 2]), Value::Binary(Binary::new(vec![1, 2]))],
        vec![Value::Structured(Arc::new(structured))],
    ]
}

/// Verifies: [DESIGN] RULES.md D42, D45: a structured value reads back as
/// written, its type's schema and name and each attribute; values and
/// types that nest read back too.
#[test]
fn nested_values_read_back_as_written() {
    reply_round_trip(&Reply::Rows(nested_rows()));
    reply_round_trip(&Reply::Description(nested_columns()));
    let mut bytes = Vec::new();
    Reply::Rows(nested_rows()).encode(&mut bytes);
    let (kind, body) = unframed(&bytes, MAX_BODY).unwrap();
    let Reply::Rows(read) = Reply::decode(kind, body).unwrap() else { panic!("not rows") };
    let Some(Value::Structured(s)) = read.last().and_then(|r| r.first()) else { panic!("{read:?}") };
    assert_eq!((s.of.schema.as_str(), s.of.name.as_str(), s.attributes.len()), ("S", "ADDRESS", 2));
}

/// Verifies: [DESIGN] RULES.md D42, D45: every reply cut short anywhere in
/// its body, a count, a flag, a tag, a string, a type's or a value's
/// field, is refused as a connection failure (SQLCODE -919, SQLSTATE
/// 08006), never read as another message.
#[test]
fn every_prefix_of_a_reply_is_refused() {
    let mut replies = every_reply();
    replies.push(Reply::Rows(nested_rows()));
    replies.push(Reply::Description(nested_columns()));
    for reply in &replies {
        let mut bytes = Vec::new();
        reply.encode(&mut bytes);
        let (kind, body) = unframed(&bytes, MAX_BODY).unwrap();
        for end in 0..body.len() {
            let cut = Reply::decode(kind, &body[..end]).err().map(|e| kind_of(&e));
            assert_eq!(cut, Some(FAILED), "{reply:?} cut at {end} of {}", body.len());
        }
    }
}

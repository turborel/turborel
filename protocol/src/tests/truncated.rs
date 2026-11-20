//! Unit tests of messages cut short: every prefix of a reply's body, each
//! field of it ending early, is refused as a connection failure, never
//! read as a shorter message (RULES.md R-TEST-2).

use std::sync::Arc;

use turborel::value::lob::{Lob, LobKind};
use turborel::value::types::{self, Field, Structured, UdtName};
use turborel::value::xml::XmlValue;
use turborel::value::{Binary, DataType, Decimal, Value};

use super::{FAILED, every_reply, kind_of, reply_round_trip, request_round_trip, unframed};
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

/// Requests whose fields are counted, nested or of large objects.
fn counted_requests() -> Vec<Request> {
    vec![
        Request::ChangeUser { user: "Y".into(), mechanism: "password".into(), initial: vec![1, 2, 3] },
        Request::CopyIn { table: "T".into(), columns: vec!["A".into(), "B".into()], ends: true },
        Request::CopyData(nested_rows()),
        Request::Describe(Described::Cursor { statement: 3, cursor: 4 }),
        Request::Lob(LobRequest::Read { locator: 7, from: 1, count: 9, characters: true }),
        Request::Lob(LobRequest::Create(LobKind::Binary)),
        Request::Lob(LobRequest::Append { locator: 7, bytes: vec![1, 2] }),
        Request::Lob(LobRequest::Free(7)),
        Request::Lob(LobRequest::Hold(7)),
    ]
}

/// Verifies: [DESIGN] RULES.md D42, D45, D195: every request of counted
/// or nested fields, and every large object request and reply, reads
/// back as written; cut short anywhere in its body it is refused as a
/// connection failure (SQLCODE -919, SQLSTATE 08006), as is one of a
/// kind of large object, a large object request or a reply there is not.
#[test]
fn every_prefix_of_a_request_is_refused() {
    let request = |request: Request, back: usize, to: u8| {
        let mut bytes = Vec::new();
        request.encode(&mut bytes);
        let at = bytes.len() - back;
        bytes[at] = to;
        let (kind, body) = unframed(&bytes, MAX_BODY).unwrap();
        Request::decode(kind, body).err().map(|e| kind_of(&e))
    };
    assert_eq!(request(Request::Lob(LobRequest::Create(LobKind::Binary)), 1, 9), Some(FAILED), "no kind of large object is 9");
    assert_eq!(request(Request::Lob(LobRequest::Free(7)), 5, 99), Some(FAILED), "no Lob request is of kind 99");
    let mut located = Vec::new();
    Reply::Lob(LobReply::Locator(7)).encode(&mut located);
    let at = located.len() - 5;
    located[at] = 99;
    let (kind, body) = unframed(&located, MAX_BODY).unwrap();
    assert_eq!(Reply::decode(kind, body).err().map(|e| kind_of(&e)), Some(FAILED), "no Lob reply is of kind 99");
    for reply in [Reply::Lob(LobReply::Data(vec![0, 1, 2])), Reply::Lob(LobReply::Locator(9))] {
        reply_round_trip(&reply);
        cut_short(&|b| reply.encode(b), |k, b| Reply::decode(k, b).map(drop));
    }
    for counted in counted_requests() {
        request_round_trip(&counted);
        cut_short(&|b| counted.encode(b), |k, b| Request::decode(k, b).map(drop));
    }
}

/// Checks that the message `encode` writes, its body cut short anywhere,
/// is refused by `decode` as a connection failure.
fn cut_short(encode: &dyn Fn(&mut Vec<u8>), decode: fn(u8, &[u8]) -> Result<()>) {
    let mut bytes = Vec::new();
    encode(&mut bytes);
    let (kind, body) = unframed(&bytes, MAX_BODY).unwrap();
    for end in 0..body.len() {
        assert_eq!(decode(kind, &body[..end]).err().map(|e| kind_of(&e)), Some(FAILED), "kind {kind} cut at {end}");
    }
}

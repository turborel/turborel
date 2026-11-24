//! Unit tests of the wire protocol: every message read back as written,
//! byte by byte where the layout is the contract; malformed ones refused as
//! connection failures (RULES.md R-TEST-1, R-TEST-2).

mod added;
mod called;
mod declared;
mod frames;
mod geometry;
mod truncated;
mod uuid;

use turborel::params::ParameterType;
use turborel::storage::Row;
use turborel::value::datetime::{Datetime, Field, Moment, parse};
use turborel::value::interval::{Interval, Qualifier};
use turborel::value::{Binary, Decimal, Value};

use super::fields::{CHARACTER, EXACT, NULL};
use super::*;
use turborel::value::Spelled;

/// The kind and body of the one frame `bytes` should be, its header read
/// with `max`; its body is all that follows the header.
fn unframed(bytes: &[u8], max: u32) -> Result<(u8, &[u8])> {
    let (head, body) = bytes.split_first_chunk::<HEADER>().unwrap();
    let (kind, length) = header(*head, max)?;
    if body.len() != length {
        return Err(broken("its length is not its body's"));
    }
    Ok((kind, body))
}

/// A request read back, checked by the bytes it encodes to, as values in
/// it compare as SQL says.
fn request_round_trip(request: &Request) {
    let mut bytes = Vec::new();
    request.encode(&mut bytes);
    let (kind, body) = unframed(&bytes, MAX_BODY).unwrap();
    let mut again = Vec::new();
    Request::decode(kind, body).unwrap().encode(&mut again);
    assert_eq!(again, bytes, "{request:?}");
}

/// Values compare as SQL says, so a reply read back is checked by the
/// bytes it encodes to: exact to the bit.
fn reply_round_trip(reply: &Reply) {
    let mut bytes = Vec::new();
    reply.encode(&mut bytes);
    let (kind, body) = unframed(&bytes, MAX_BODY).unwrap();
    let mut again = Vec::new();
    Reply::decode(kind, body).unwrap().encode(&mut again);
    assert_eq!(again, bytes, "{reply:?}");
}

/// The code and SQLSTATE of an error.
fn kind_of(e: &SqlError) -> (i32, SqlState) {
    (e.sqlcode(), e.sqlstate())
}

const FAILED: (i32, SqlState) = (-919, SqlState::CONNECTION_FAILURE);

/// Verifies: [DESIGN] RULES.md D42, D45: every request reads back as
/// written; a frame is its kind, its body's length and the body, the fields
/// little-endian; a cancel key is never shown.
#[test]
fn requests_read_back_as_written() {
    let hello = Request::Hello {
        version: VERSION,
        user: "X".into(),
        mechanism: "trust".into(),
        initial: vec![1, 2],
        capabilities: vec!["one".into(), String::new()],
        attributes: Vec::new(),
    };
    let key = CancelKey([7; KEY_BYTES]);
    let requests = [
        hello,
        Request::Answer(vec![]),
        Request::Answer(vec![9; 300]),
        Request::Query { sql: "SELECT 1".into(), ends: false, attributes: Vec::new(), multi: false },
        Request::Query { sql: "INSERT INTO T VALUES (1)".into(), ends: true, attributes: Vec::new(), multi: false },
        Request::Prepare { statement: u32::MAX, sql: "SELECT ? FROM T".into() },
        Request::Execute {
            statement: 0,
            cursor: 0,
            attributes: Vec::new(),
            values: vec![Value::Null, Value::Char("é".into()), Value::Exact(Decimal::new(-5, 1))],
            rows: 10,
            ends: false,
        },
        Request::Execute { statement: 1, cursor: 0, values: vec![], rows: 0, ends: true, attributes: Vec::new() },
        Request::Fetch { statement: 2, cursor: 0, rows: u32::MAX },
        Request::Close { statement: 3, cursor: 0 },
        Request::Deallocate { statement: 4 },
        Request::Cancel(key),
        Request::Terminate,
    ];
    for request in &requests {
        request_round_trip(request);
    }
    assert_eq!(format!("{key:?}"), "CancelKey(..)");
    let mut cancel = Vec::new();
    Request::Cancel(key).encode(&mut cancel);
    assert_eq!(cancel, [&[b'Z', 16, 0, 0, 0][..], &[7; 16][..]].concat());
    let mut bytes = Vec::new();
    Request::Query { sql: "AB".into(), ends: true, attributes: Vec::new(), multi: false }.encode(&mut bytes);
    assert_eq!(bytes, [b'Q', 7, 0, 0, 0, 2, 0, 0, 0, b'A', b'B', 1]);
    bytes.clear();
    Request::Terminate.encode(&mut bytes);
    assert_eq!(bytes, [b'X', 0, 0, 0, 0]);
}

/// The datetime `text` writes, of `moment`.
fn datetime(moment: Moment, text: &str) -> Value {
    let w = parse(moment, text).unwrap();
    Value::Datetime(Datetime::of_local(moment, w.local, w.zone.unwrap_or(0), w.zone.is_some(), w.precision))
}

/// Rows of every kind of value, at its edges.
fn edge_rows() -> Vec<Row> {
    vec![
        vec![Value::Exact(Decimal::new(-12_345, 2)), Value::Char("abcd".into()), Value::Approx(-0.5)],
        vec![Value::Exact(Decimal::new(i128::MAX, 0)), Value::Null, Value::Approx(f64::MAX)],
        vec![Value::Binary(Binary::new(vec![0xAB, 0, 0xC0])), Value::Binary(Binary::default())],
        vec![Value::Bool(true), Value::Bool(false), Value::Null],
        vec![
            datetime(Moment::Date, "2024-02-29"),
            datetime(Moment::Time, "23:59:59.5-12:59"),
            datetime(Moment::Timestamp, "0001-01-01 00:00:00"),
        ],
        vec![Value::Interval(Interval { units: -14, qualifier: Qualifier::new(Field::Year, Field::Month, 2, 0) })],
        vec![
            Value::Multiset(vec![Value::Exact(Decimal::new(1, 0)), Value::Null, Value::Multiset(Vec::new().into())].into()),
            Value::Multiset(Vec::new().into()),
        ],
        vec![],
    ]
}

/// A nullable column of `data_type`, named `name` if it has one, its
/// declared type not told.
fn column(name: Option<&str>, data_type: DataType) -> Column {
    Column { name: name.map(Into::into), data_type, not_null: false, declared: None }
}

/// One reply of every kind, with fields at their edges.
fn every_reply() -> Vec<Reply> {
    let interval = || DataType::Interval(Qualifier::new(Field::Day, Field::Second, 9, 2));
    let columns = vec![
        Column { not_null: true, ..column(Some("K"), DataType::Exact { precision: 9, scale: 2, spelled: Spelled::Numeric }) },
        column(None, DataType::character(4)),
        column(Some("F"), DataType::Approximate { precision: 53, spelled: Spelled::Float }),
        column(Some("B"), DataType::Binary { length: 10 }),
        column(Some("V"), DataType::VarBinary { length: 3 }),
        column(Some("L"), DataType::Boolean),
        column(Some("D"), DataType::Date),
        column(Some("T"), DataType::Time { precision: 2, zoned: false }),
        column(Some("Z"), DataType::Time { precision: 0, zoned: true }),
        column(Some("S"), DataType::Timestamp { precision: 6, zoned: false }),
        column(Some("W"), DataType::Timestamp { precision: 3, zoned: true }),
        column(Some("I"), interval()),
        column(Some("I"), interval()),
        column(Some("M"), DataType::multiset(DataType::multiset(DataType::character(2)))),
    ];
    let cited = Failure {
        sqlcode: -103,
        sqlstate: "2A000".into(),
        message: "column K is ambiguous".into(),
        rule: Some("X3.135-1986 §5.7 SR3b".into()),
        position: Some(7),
        active: false,
        diagnostics: None,
    };
    let (rule, position) = (None, None);
    let plain = Failure { sqlcode: -911, sqlstate: "TB911".into(), message: String::new(), rule, position, active: true, ..cited.clone() };
    let parameters = vec![ParameterType::Varying, ParameterType::Of(DataType::Exact { precision: 5, scale: 2, spelled: Spelled::Numeric })];
    vec![
        Reply::Challenge(vec![0xff]),
        Reply::Ready {
            authorization: "X".into(),
            key: CancelKey([0xab; KEY_BYTES]),
            capabilities: vec!["one".into()],
            server: "turborel 0.0.0 (alpha)".into(),
        },
        Reply::Prepared { statement: 9, parameters, columns: columns.clone(), called: None },
        Reply::Prepared { statement: 0, parameters: vec![], columns: vec![], called: None },
        Reply::Suspended,
        Reply::Description(columns),
        Reply::Description(vec![]),
        Reply::Rows(edge_rows()),
        Reply::Complete(Completed { active: true, ..Completed::of(0, "00000") }),
        Reply::Complete(Completed::of(100, "02000")),
        Reply::Complete(Completed::of(i32::MIN, "01007")),
        Reply::Error(cited),
        Reply::Error(plain),
    ]
}

/// Verifies: [DESIGN] RULES.md D42, D45: every reply reads back as written:
/// each data type and each kind of value, a multiset nested and empty, a null, a column named and one
/// not, a parameter of each kind, an error with its rule and position and
/// one without; `row_size` is the bytes a row takes.
#[test]
fn replies_read_back_as_written() {
    for reply in &every_reply() {
        reply_round_trip(reply);
    }
    let batch = edge_rows();
    let mut sized = Vec::new();
    Reply::Rows(batch.clone()).encode(&mut sized);
    assert_eq!(HEADER + 4 + batch.iter().map(row_size).sum::<usize>(), sized.len(), "row_size is what a row takes");
    let mut bytes = Vec::new();
    Reply::Rows(vec![vec![Value::Null, Value::Exact(Decimal::new(-1, 1))]]).encode(&mut bytes);
    let row = [1, 0, 0, 0, 2, 0, 0, 0, NULL, EXACT];
    assert_eq!(bytes.get(HEADER..HEADER + row.len()), Some(&row[..]));
    assert_eq!(bytes.get(HEADER + row.len()..), Some(&[[0xff; 16].as_slice(), &[1, 0, 0, 0]].concat()[..]));
}

/// Verifies: [DESIGN] RULES.md D42: a body longer than the reader takes is
/// a connection failure (SQLCODE -919, SQLSTATE 08006) from its header
/// alone; one exactly as long is taken.
#[test]
fn a_body_past_the_limit_is_refused_from_its_header() {
    let mut bytes = Vec::new();
    Request::Query { sql: "12".into(), ends: false, attributes: Vec::new(), multi: false }.encode(&mut bytes);
    assert!(unframed(&bytes, 7).is_ok());
    assert_eq!(unframed(&bytes, 6).map_err(|e| kind_of(&e)).err(), Some(FAILED));
    assert_eq!(header([b'Q', 0xff, 0xff, 0xff, 0xff], MAX_BODY).map_err(|e| kind_of(&e)).err(), Some(FAILED));
}

/// Verifies: [DESIGN] RULES.md D42, R-ERR-1: a malformed message is a
/// connection failure, never a panic: a kind of the other side or of none,
/// a body cut short or with bytes after its end, a flag neither 0 nor 1, a
/// tag of no value or data type, a string not UTF-8, a key cut short.
#[test]
fn malformed_messages_are_connection_failures() {
    let failed = |e: SqlError| kind_of(&e);
    assert_eq!(Request::decode(b'K', &[]).map_err(failed).err(), Some(FAILED), "a reply's kind");
    assert_eq!(Reply::decode(b'Q', &[]).map_err(failed).err(), Some(FAILED), "a request's kind");
    assert_eq!(Reply::decode(0, &[]).map_err(failed).err(), Some(FAILED));
    assert_eq!(Request::decode(b'Q', &[5, 0, 0, 0, b'A']).map_err(failed).err(), Some(FAILED), "cut short");
    assert_eq!(Request::decode(b'X', &[0]).map_err(failed).err(), Some(FAILED), "a byte after its end");
    assert_eq!(Request::decode(b'Q', &[1, 0, 0, 0, 0xff]).map_err(failed).err(), Some(FAILED), "not UTF-8");
    let description = |flag: u8, tag: u8| [1, 0, 0, 0, flag, tag, 1, 0, 0, 0, 0, 0];
    assert!(Reply::decode(b'D', &description(0, CHARACTER)).is_ok());
    assert_eq!(Reply::decode(b'D', &description(2, CHARACTER)).map_err(failed).err(), Some(FAILED), "a flag of 2");
    assert_eq!(Reply::decode(b'D', &description(0, 99)).map_err(failed).err(), Some(FAILED), "no such type");
    assert_eq!(Reply::decode(b'B', &[1, 0, 0, 0, 1, 0, 0, 0, 99]).map_err(failed).err(), Some(FAILED), "no such value");
    assert_eq!(Request::decode(b'Z', &[1, 2]).map_err(failed).err(), Some(FAILED), "a key cut short");
    assert_eq!(Reply::decode(b'R', &[0, 0, 0, 0, 1]).map_err(failed).err(), Some(FAILED), "a key cut short");
    let e = Reply::decode(b'B', &[1, 0, 0, 0, 9, 0, 0, 0]).unwrap_err();
    assert!(e.msg.contains("reply received is malformed"), "{e}");
}

/// Verifies: [DESIGN] RULES.md D42, R-ERR-2: a failure carries all an
/// error says: its SQLCODE, its SQLSTATE, closer than its code's where it
/// was raised with one, its message, its rule and its position; and shows
/// them as the error does.
#[test]
fn a_failure_says_what_its_error_says() {
    let e = SqlError::at(Code::SyntaxRule, 7, "[X3.135-1986 §5.7 SR3b] column K is ambiguous");
    let f = Failure::from(&e);
    assert_eq!((f.sqlcode, f.sqlstate.as_str()), (-103_i32, "2A000"));
    assert_eq!((f.message.as_str(), f.rule.as_deref(), f.position), ("column K is ambiguous", Some("X3.135-1986 §5.7 SR3b"), Some(7)));
    assert_eq!(f.to_string(), "SQLCODE -103 (SQLSTATE 2A000): column K is ambiguous [X3.135-1986 §5.7 SR3b] (at character 7)");
    let refused = Failure::from(&broken("gone"));
    assert_eq!(refused.to_string(), "SQLCODE -919 (SQLSTATE 08006): gone");
    assert_eq!((refused.sqlcode, refused.sqlstate.as_str(), refused.rule, refused.position), (-919_i32, "08006", None, None));
}

/// Verifies: [DESIGN] RULES.md D42, D45, D79: the types and values of the
/// later kinds read back as written: DECFLOAT, CHARACTER VARYING, CLOB and
/// BLOB, an array, a row and a distinct type, sent as its source; a large
/// object held whole of each kind, one by its locator, and one neither,
/// sent as locator 0; arrays, rows and a value of a type beyond the
/// standard, unregistered; `row_size` is the bytes each row takes.
#[test]
fn later_types_and_values_read_back_as_written() {
    use turborel::value::DecFloat;
    use turborel::value::lob::{Lob, LobKind};
    let column = |name: &str, data_type: DataType| Column { name: Some(name.into()), data_type, not_null: false, declared: None };
    let field = |name: &str, data_type: DataType| turborel::value::types::Field { name: name.into(), data_type, unnamed: false };
    let columns = vec![
        column("D", DataType::DecFloat { precision: 34 }),
        column("V", DataType::varying(7)),
        column("C", DataType::Lob { kind: LobKind::Character, length: 10, units: turborel::value::CharUnits::Octets }),
        column("B", DataType::Lob { kind: LobKind::Binary, length: 1_048_576, units: turborel::value::CharUnits::Characters }),
        column("A", DataType::array(DataType::array(DataType::Boolean, 2), 3)),
        column("R", DataType::row(vec![field("X", DataType::character(1)), field("Y", DataType::Boolean)])),
    ];
    reply_round_trip(&Reply::Description(columns));
    let distinct = DataType::distinct("S", "MONEY", DataType::varying(4));
    let mut bytes = Vec::new();
    Reply::Description(vec![column("M", distinct)]).encode(&mut bytes);
    let mut source = Vec::new();
    Reply::Description(vec![column("M", DataType::varying(4))]).encode(&mut source);
    assert_eq!(bytes, source, "a distinct type is sent as its source");
    let rows: Vec<Row> = vec![
        vec![Value::DecFloat(DecFloat::of_decimal(&Decimal::new(-15, 1)).unwrap()), Value::Char("é".into())],
        vec![Value::Lob(Lob::of_text("aé")), Value::Lob(Lob::of_octets(&[0, 0xff])), Value::Lob(Lob::empty(LobKind::Binary))],
        vec![Value::Lob(Lob::of_locator(LobKind::Character, 7, (300, 200))), Value::Lob(Lob::of_reference(LobKind::Binary, 3, (9, 9)))],
        vec![Value::Array(vec![Value::Bool(true), Value::Null].into()), Value::Row(vec![Value::Char("x".into())].into())],
        vec![turborel::value::types::unregistered("POINT", "(1, 2)", &[1, 2])],
    ];
    reply_round_trip(&Reply::Rows(rows.clone()));
    let mut sized = Vec::new();
    Reply::Rows(rows.clone()).encode(&mut sized);
    assert_eq!(HEADER + 4 + rows.iter().map(row_size).sum::<usize>(), sized.len(), "row_size is what a row takes");
    let mut sent = Vec::new();
    Reply::Rows(vec![vec![Value::Lob(Lob::of_reference(LobKind::Binary, 3, (9, 9)))]]).encode(&mut sent);
    let (kind, body) = unframed(&sent, MAX_BODY).unwrap();
    let Reply::Rows(read) = Reply::decode(kind, body).unwrap() else { panic!("not rows") };
    let Some(Value::Lob(l)) = read.first().and_then(|r| r.first()) else { panic!("{read:?}") };
    assert_eq!((l.locator(), l.octets()), (Some(0), 9), "neither held nor located: locator 0");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §4.2.2, §4.2.3] [DESIGN] RULES.md
/// D42, D106: XML types read back with their modifiers, of XMLSCHEMA with
/// the registered XML Schema, namespace and element they indicate [§6.1
/// GR1], and XML values as their serialization, the text a value held in
/// its tree is serialized to; `row_size` is the bytes each row takes.
#[test]
fn xml_types_and_values_read_back_as_written() {
    use turborel::value::types::XmlSchemaNamed;
    use turborel::value::xml::{Typing, XmlType, XmlValue};
    use turborel::xml::{Sequence, Syntax, Whitespace, parse};
    let column = |name: &str, x: XmlType| Column { name: Some(name.into()), data_type: DataType::Xml(x), not_null: false, declared: None };
    let types = [
        XmlType::Sequence,
        XmlType::Document(Typing::Untyped),
        XmlType::Document(Typing::Any),
        XmlType::Content(Typing::Untyped),
        XmlType::Content(Typing::Any),
        XmlType::Document(Typing::Schema(std::sync::Arc::new(XmlSchemaNamed {
            schema: "A".into(),
            name: "S".into(),
            namespace: Some("urn:s".into()),
            element: Some("e".into()),
        }))),
        XmlType::Content(Typing::Schema(std::sync::Arc::new(XmlSchemaNamed {
            schema: "A".into(),
            name: "S".into(),
            namespace: None,
            element: None,
        }))),
    ];
    reply_round_trip(&Reply::Description(types.iter().map(|x| column("X", x.clone())).collect()));
    let held = Value::Xml(XmlValue::of(Sequence::document(parse("<a>&lt;</a>", Syntax::Document, Whitespace::Preserve).unwrap())));
    let rows: Vec<Row> = vec![vec![Value::Xml(XmlValue::serialized("<b/>")), held, Value::Null]];
    reply_round_trip(&Reply::Rows(rows.clone()));
    let mut sized = Vec::new();
    Reply::Rows(rows.clone()).encode(&mut sized);
    assert_eq!(HEADER + 4 + rows.iter().map(row_size).sum::<usize>(), sized.len(), "row_size is what a row takes");
    let (kind, body) = unframed(&sized, MAX_BODY).unwrap();
    let Reply::Rows(read) = Reply::decode(kind, body).unwrap() else { panic!("not rows") };
    let texts: Vec<String> = read[0].iter().map(ToString::to_string).collect();
    assert_eq!(texts, ["<b/>", "<a>&lt;</a>", "NULL"]);
}

/// Verifies: [DESIGN] RULES.md D42, R-ERR-1: a malformed value or type of
/// the later kinds is a connection failure: a large object of no kind, a
/// character one not UTF-8, an array type of no maximum cardinality, and a
/// row type of no fields.
#[test]
fn malformed_later_kinds_are_connection_failures() {
    let failed = |e: SqlError| kind_of(&e);
    // A Rows reply of one row of one value: the value's tag, then its bytes.
    let one = |value: &[u8]| [&[1, 0, 0, 0, 1, 0, 0, 0][..], value].concat();
    assert_eq!(Reply::decode(b'B', &one(&[20, 2, 0, 0, 0, 0])).map_err(failed).err(), Some(FAILED), "no such kind");
    assert_eq!(Reply::decode(b'B', &one(&[20, 0, 1, 0, 0, 0, 0xff])).map_err(failed).err(), Some(FAILED), "not UTF-8");
    assert!(Reply::decode(b'B', &one(&[20, 1, 1, 0, 0, 0, 0xff])).is_ok(), "a binary one of any octets");
    // A Description of one unnamed column of the type given, nullable.
    let described = |t: &[u8]| [&[1, 0, 0, 0, 0][..], t, &[0]].concat();
    assert_eq!(Reply::decode(b'D', &described(&[16, 0, 0, 0, 0, 14])).map_err(failed).err(), Some(FAILED), "cardinality 0");
    assert!(Reply::decode(b'D', &described(&[16, 1, 0, 0, 0, 14])).is_ok(), "cardinality 1");
    assert_eq!(Reply::decode(b'D', &described(&[17, 0, 0, 0, 0])).map_err(failed).err(), Some(FAILED), "no fields");
}

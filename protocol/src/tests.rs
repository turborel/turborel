//! Unit tests of the wire protocol: every message read back as written,
//! byte by byte where the layout is the contract; malformed ones refused as
//! connection failures (RULES.md R-TEST-1, R-TEST-2). Split from
//! lib.rs (RULES.md R-SIZE-4).

use turborel::params::ParameterType;
use turborel::storage::Row;
use turborel::value::datetime::{Datetime, Field, Moment, parse};
use turborel::value::interval::{Interval, Qualifier};
use turborel::value::{Bits, Decimal, Value};

use super::fields::{CHARACTER, EXACT, NULL};
use super::*;

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
fn kind_of(e: &SqlError) -> (i32, &'static str) {
    (e.sqlcode(), e.sqlstate().as_str())
}

const FAILED: (i32, &str) = (-919, "08006");

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
    };
    let key = CancelKey([7; KEY_BYTES]);
    let requests = [
        hello,
        Request::Answer(vec![]),
        Request::Answer(vec![9; 300]),
        Request::Query("SELECT 1".into()),
        Request::Prepare { statement: u32::MAX, sql: "SELECT ? FROM T".into() },
        Request::Execute { statement: 0, values: vec![Value::Null, Value::Char("é".into()), Value::Exact(Decimal::new(-5, 1))], rows: 10 },
        Request::Execute { statement: 1, values: vec![], rows: 0 },
        Request::Fetch { statement: 2, rows: u32::MAX },
        Request::Close { statement: 3 },
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
    Request::Query("AB".into()).encode(&mut bytes);
    assert_eq!(bytes, [b'Q', 6, 0, 0, 0, 2, 0, 0, 0, b'A', b'B']);
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
        vec![Value::Bits(Bits::of_bits("1010101011").unwrap()), Value::Bits(Bits::default())],
        vec![
            datetime(Moment::Date, "2024-02-29"),
            datetime(Moment::Time, "23:59:59.5-12:59"),
            datetime(Moment::Timestamp, "0001-01-01 00:00:00"),
        ],
        vec![Value::Interval(Interval { units: -14, qualifier: Qualifier::new(Field::Year, Field::Month, 2, 0) })],
        vec![],
    ]
}

/// One reply of every kind, with fields at their edges.
fn every_reply() -> Vec<Reply> {
    let columns = vec![
        Column { name: Some("K".into()), data_type: DataType::Exact { precision: 9, scale: 2 }, not_null: true },
        Column { name: None, data_type: DataType::Character { length: 4 }, not_null: false },
        Column { name: Some("F".into()), data_type: DataType::Approximate { precision: 53 }, not_null: false },
        Column { name: Some("B".into()), data_type: DataType::Bit { length: 10 }, not_null: false },
        Column { name: Some("V".into()), data_type: DataType::BitVarying { length: 3 }, not_null: false },
        Column { name: Some("D".into()), data_type: DataType::Date, not_null: false },
        Column { name: Some("T".into()), data_type: DataType::Time { precision: 2, zoned: false }, not_null: false },
        Column { name: Some("Z".into()), data_type: DataType::Time { precision: 0, zoned: true }, not_null: false },
        Column { name: Some("S".into()), data_type: DataType::Timestamp { precision: 6, zoned: false }, not_null: false },
        Column { name: Some("W".into()), data_type: DataType::Timestamp { precision: 3, zoned: true }, not_null: false },
        Column { name: Some("I".into()), data_type: DataType::Interval(Qualifier::new(Field::Day, Field::Second, 9, 2)), not_null: false },
        Column { name: Some("I".into()), data_type: DataType::Interval(Qualifier::new(Field::Day, Field::Second, 9, 2)), not_null: false },
    ];
    let cited = Failure {
        sqlcode: -103,
        sqlstate: "2A000".into(),
        message: "column K is ambiguous".into(),
        rule: Some("X3.135-1986 §5.7 SR3b".into()),
        position: Some(7),
        active: false,
    };
    let plain = Failure { sqlcode: -911, sqlstate: "TB911".into(), message: String::new(), rule: None, position: None, active: true };
    let parameters = vec![ParameterType::Varying, ParameterType::Of(DataType::Exact { precision: 5, scale: 2 })];
    vec![
        Reply::Challenge(vec![0xff]),
        Reply::Ready {
            authorization: "X".into(),
            key: CancelKey([0xab; KEY_BYTES]),
            capabilities: vec!["one".into()],
            server: "turborel 0.0.0 (alpha)".into(),
        },
        Reply::Prepared { statement: 9, parameters, columns: columns.clone() },
        Reply::Prepared { statement: 0, parameters: vec![], columns: vec![] },
        Reply::Suspended,
        Reply::Description(columns),
        Reply::Description(vec![]),
        Reply::Rows(edge_rows()),
        Reply::Complete(Completed { sqlcode: 0, sqlstate: "00000".into(), active: true }),
        Reply::Complete(Completed { sqlcode: 100, sqlstate: "02000".into(), active: false }),
        Reply::Complete(Completed { sqlcode: i32::MIN, sqlstate: "01007".into(), active: false }),
        Reply::Error(cited),
        Reply::Error(plain),
    ]
}

/// Verifies: [DESIGN] RULES.md D42, D45: every reply reads back as written:
/// each data type and each kind of value, a null, a column named and one
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
    Request::Query("12".into()).encode(&mut bytes);
    assert!(unframed(&bytes, 6).is_ok());
    assert_eq!(unframed(&bytes, 5).map_err(|e| kind_of(&e)).err(), Some(FAILED));
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
    let description = |flag: u8, tag: u8| [1, 0, 0, 0, flag, tag, 1, 0, 0, 0, 0];
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

//! Unit tests of what version 1 gained after its first form: capabilities,
//! tails, the messages added, diagnostics and compressed frames, each read
//! back as written and refused past its bounds (RULES.md R-TEST-1,
//! R-TEST-2).

use turborel::error::{Code, SqlError, SqlState, Subject};
use turborel::value::Value;

use super::{FAILED, kind_of, reply_round_trip, request_round_trip, unframed};
use crate::capability::{self, Capabilities};
use crate::compress::{COMPRESSED, DEFAULT_THRESHOLD, compress_frames, decompress};
use crate::*;

/// `request`, encoded.
fn encoded(request: &Request) -> Vec<u8> {
    let mut bytes = Vec::new();
    request.encode(&mut bytes);
    bytes
}

/// The failure of decoding the request of kind `kind` and body `body`.
fn refused(kind: u8, body: &[u8]) -> Option<(i32, SqlState)> {
    Request::decode(kind, body).err().map(|e| kind_of(&e))
}

/// `n` attributes, named A0, A1, ...
fn attributes(n: usize) -> Attributes {
    (0..n).map(|i| (format!("A{i}"), "v".to_owned())).collect()
}

/// Verifies: [DESIGN] RULES.md D185: a set of capabilities is made of the
/// names this side knows, others left out; it is granted as those both
/// have, and listed in a fixed order whatever order they were asked in.
#[test]
fn capabilities_are_the_names_both_know() {
    let asked = Capabilities::of(&["copy", "no such", capability::NOTICES]);
    assert!(asked.has(capability::COPY) && asked.has(capability::NOTICES));
    assert!(!asked.has("no such") && !asked.has(capability::PING));
    assert_eq!(asked.names(), [capability::NOTICES, capability::COPY]);
    let granted = asked.and(Capabilities::of(&[capability::COPY, capability::PING]));
    assert_eq!(granted.names(), [capability::COPY]);
    assert_eq!(Capabilities::all().names().len(), capability::ALL.len());
    assert_eq!(asked.without(capability::COPY).names(), [capability::NOTICES]);
    assert_eq!(Capabilities::NONE.names(), Vec::<String>::new());
}

/// Verifies: [DESIGN] RULES.md D185, D188, D189, D191, D192, D195: every
/// request added, and every tail of one of the first form, reads back as
/// written; a request whose tail is empty is byte for byte its first form.
#[test]
fn added_requests_read_back_as_written() {
    let requests = [
        Request::Hello {
            version: VERSION,
            user: "X".into(),
            mechanism: "trust".into(),
            initial: vec![],
            capabilities: vec![capability::PING.into()],
            attributes: attributes(MAX_ATTRIBUTES),
        },
        Request::Query { sql: "SELECT 1; SELECT 2".into(), ends: true, attributes: attributes(1), multi: true },
        Request::Query { sql: "S".into(), ends: false, attributes: Vec::new(), multi: true },
        Request::Execute { statement: 1, cursor: 7, values: vec![], rows: 2, ends: false, attributes: attributes(2) },
        Request::Fetch { statement: 1, cursor: u32::MAX, rows: 0 },
        Request::Close { statement: 1, cursor: 3 },
        Request::Describe(Described::Statement(4)),
        Request::Describe(Described::Cursor { statement: 4, cursor: 0 }),
        Request::Ping,
        Request::Reset,
        Request::ChangeUser { user: "Y".into(), mechanism: "password".into(), initial: vec![1, 2, 3] },
        Request::CopyIn { table: "S.T".into(), columns: vec!["A".into(), "B".into()], ends: true },
        Request::CopyIn { table: "T".into(), columns: vec![], ends: false },
        Request::CopyData(vec![vec![Value::Null, Value::Char("x".into())], vec![]]),
        Request::CopyEnd { abandon: true },
    ];
    for request in &requests {
        request_round_trip(request);
    }
    assert_eq!(encoded(&Request::Ping), [b'G', 0, 0, 0, 0]);
    assert_eq!(encoded(&Request::Describe(Described::Statement(2))), [b'Y', 9, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0]);
    let first = Request::Close { statement: 2, cursor: 0 };
    assert_eq!(encoded(&first), [b'C', 4, 0, 0, 0, 2, 0, 0, 0], "no tail: the first form");
    let tailed = Request::Close { statement: 2, cursor: 5 };
    assert_eq!(encoded(&tailed), [b'C', 9, 0, 0, 0, 2, 0, 0, 0, 3, 5, 0, 0, 0], "tag 3, the cursor");
    let marked = Request::Query { sql: String::new(), ends: false, attributes: Vec::new(), multi: true };
    assert_eq!(encoded(&marked), [b'Q', 6, 0, 0, 0, 0, 0, 0, 0, 0, 2], "tag 2, multi, no field");
}

/// Verifies: [DESIGN] RULES.md D185, D191: a tail's tags come in increasing
/// order, once each, each one its message takes; attributes are at most
/// `MAX_ATTRIBUTES`, each name 1 to `MAX_ATTRIBUTE_NAME` bytes and each
/// value at most `MAX_ATTRIBUTE_VALUE`; Describe of a statement names
/// cursor 0. Each refused as a connection failure. Boundaries: exactly
/// the most attributes, and the longest name and value, are taken.
#[test]
fn tails_and_attributes_are_bounded() {
    let query = |attributes: Attributes| encoded(&Request::Query { sql: "S".into(), ends: false, attributes, multi: false });
    let body = |bytes: &[u8]| bytes[HEADER..].to_vec();
    assert!(Request::decode(b'Q', &body(&query(attributes(MAX_ATTRIBUTES)))).is_ok());
    assert_eq!(refused(b'Q', &body(&query(attributes(MAX_ATTRIBUTES + 1)))), Some(FAILED), "too many");
    let longest = vec![("N".repeat(MAX_ATTRIBUTE_NAME), "v".repeat(MAX_ATTRIBUTE_VALUE))];
    assert!(Request::decode(b'Q', &body(&query(longest))).is_ok());
    assert_eq!(refused(b'Q', &body(&query(vec![("N".repeat(MAX_ATTRIBUTE_NAME + 1), String::new())]))), Some(FAILED), "a name");
    assert_eq!(refused(b'Q', &body(&query(vec![(String::new(), String::new())]))), Some(FAILED), "an empty name");
    assert_eq!(refused(b'Q', &body(&query(vec![("N".into(), "v".repeat(MAX_ATTRIBUTE_VALUE + 1))]))), Some(FAILED), "a value");
    let plain = body(&query(Vec::new()));
    assert_eq!(refused(b'Q', &[plain.as_slice(), &[3, 1, 0, 0, 0]].concat()), Some(FAILED), "a cursor of a Query");
    assert_eq!(refused(b'Q', &[plain.as_slice(), &[2, 2]].concat()), Some(FAILED), "a tag twice");
    assert_eq!(refused(b'Q', &[plain.as_slice(), &[9]].concat()), Some(FAILED), "a tag no request has");
    let execute = body(&encoded(&Request::Execute { statement: 0, cursor: 0, values: vec![], rows: 0, ends: false, attributes: vec![] }));
    let backwards = [execute.as_slice(), &[3, 1, 0, 0, 0, 1, 0, 0, 0, 0]].concat();
    assert_eq!(refused(b'E', &backwards), Some(FAILED), "out of order");
    assert_eq!(refused(b'Y', &[0, 1, 0, 0, 0, 1, 0, 0, 0]), Some(FAILED), "a statement's cursor");
    assert_eq!(refused(b'Y', &[2, 1, 0, 0, 0, 0, 0, 0, 0]), Some(FAILED), "neither");
    assert_eq!(refused(b'N', &[2]), Some(FAILED), "a flag of 2");
    assert_eq!(refused(b'G', &[0]), Some(FAILED), "a byte after Ping");
}

/// Diagnostics of every field.
fn every_field() -> Diagnostics {
    Diagnostics {
        severity: Some(ERROR.into()),
        detail: Some("d".into()),
        hint: Some(String::new()),
        context: Some("trigger S.T".into()),
        schema: Some("S".into()),
        table: Some("T".into()),
        column: Some("C".into()),
        data_type: Some("INTEGER".into()),
        constraint: Some("K".into()),
    }
}

/// A duplicate key's failure, of `diagnostics`.
fn failure(diagnostics: Option<Box<Diagnostics>>) -> Failure {
    Failure { sqlcode: -803, sqlstate: "23505".into(), message: "m".into(), rule: None, position: None, active: true, diagnostics }
}

/// Verifies: [DESIGN] RULES.md D186, D187, D189, D190, D192, D194, D195, D197, D198, D240:
/// every reply added, and every tail of one of the first form, reads back
/// as written; a Complete whose tail is empty is byte for byte its first
/// form.
#[test]
fn added_replies_read_back_as_written() {
    let diagnostics = every_field();
    let only_severity = Diagnostics { severity: Some(WARNING.into()), ..Diagnostics::default() };
    let replies = [
        Reply::Notice(Notice { sqlstate: "01004".into(), message: "cut".into(), diagnostics: only_severity }),
        Reply::Notice(Notice { sqlstate: "01T01".into(), message: String::new(), diagnostics: Diagnostics::default() }),
        Reply::Status { name: "SESSION.TIME_ZONE".into(), value: "+00:00".into() },
        Reply::Pong,
        Reply::Complete(Completed { position: Some(u64::MAX), more: true, rows: Some(0), ..Completed::of(0, "00000") }),
        Reply::Complete(Completed { more: true, ..Completed::of(100, "02000") }),
        Reply::Complete(Completed { rows: Some(2), generated: vec!["1".into(), String::new()], ..Completed::of(0, "00000") }),
        Reply::Complete(Completed {
            generated: vec!["7".into()],
            conditions: vec![failure(Some(Box::new(diagnostics.clone()))), failure(None)],
            time: Some(Timing { total: 120, queued: 3, locks: 0 }),
            ..Completed::of(0, "01003")
        }),
        Reply::Complete(Completed { time: Some(Timing { total: u64::MAX, queued: 0, locks: 1 }), ..Completed::of(100, "02000") }),
        Reply::Error(failure(Some(Box::new(diagnostics)))),
        Reply::Error(failure(Some(Box::default()))),
    ];
    for reply in &replies {
        reply_round_trip(reply);
    }
    let mut bytes = Vec::new();
    Reply::Complete(Completed::of(0, "00000")).encode(&mut bytes);
    assert_eq!(bytes.len(), HEADER + 4 + 4 + 5 + 1, "no tail: the first form");
    bytes.clear();
    Reply::Complete(Completed { position: Some(9), ..Completed::of(0, "00000") }).encode(&mut bytes);
    assert_eq!(bytes[HEADER + 14..], [4, 9, 0, 0, 0, 0, 0, 0, 0], "tag 4, the position");
    let complete = &bytes[HEADER..HEADER + 14];
    let out_of_order = [complete, &[5, 4, 1, 0, 0, 0, 0, 0, 0, 0]].concat();
    assert!(Reply::decode(b'K', &out_of_order).is_err(), "out of order");
    assert!(Reply::decode(b'K', &[complete, &[7]].concat()).is_err(), "a failure's tag");
    assert!(Reply::decode(b'G', &[1]).is_err(), "a byte after Pong");
    bytes.clear();
    Reply::Complete(Completed { generated: vec!["5".into()], ..Completed::of(0, "00000") }).encode(&mut bytes);
    assert_eq!(bytes[HEADER + 14..], [16, 1, 0, 0, 0, 1, 0, 0, 0, b'5'], "tag 16, a count and the strings");
    let short = &bytes[HEADER..bytes.len() - 1];
    assert!(Reply::decode(b'K', short).is_err(), "a generated value cut short");
}

/// Verifies: [DESIGN] RULES.md D186: a failure's diagnostics come from what
/// its error names: the schema and table, the constraint, the column, its
/// detail, hint and data type, and the trigger or routine it was raised in
/// as its context; its severity is FATAL where it ends the connection, else
/// ERROR; one about nothing has its severity alone; `plain` drops them.
#[test]
fn diagnostics_are_what_an_error_names() {
    let constraint = Subject { column: Some("K".into()), ..Subject::constraint(("S".into(), "PK".into()), ("S".into(), "T".into())) };
    let e = SqlError::new(Code::Constraint, "dup").about(constraint).with_detail("the key (1)").with_hint("h").or_of_type(&"INTEGER");
    let d = *Failure::from(&e).diagnostics.unwrap();
    assert_eq!(
        (d.severity.as_deref(), d.schema.as_deref(), d.table.as_deref(), d.constraint.as_deref()),
        (Some(ERROR), Some("S"), Some("T"), Some("PK"))
    );
    assert_eq!((d.column.as_deref(), d.detail.as_deref(), d.hint.as_deref()), (Some("K"), Some("the key (1)"), Some("h")));
    assert_eq!((d.data_type.as_deref(), d.context), (Some("INTEGER"), None));
    let typed_twice = SqlError::new(Code::Data, "x").or_of_type(&"DATE").or_of_type(&"TIME");
    assert_eq!(Failure::from(&typed_twice).diagnostics.unwrap().data_type.as_deref(), Some("DATE"), "the first type kept");
    let fired = SqlError::new(Code::Data, "x").about(Subject::trigger(("S".into(), "TR".into()), ("S".into(), "T".into())));
    assert_eq!(Failure::from(&fired).diagnostics.unwrap().context.as_deref(), Some("trigger S.TR"));
    let called = SqlError::new(Code::Data, "x").about(Subject::routine(("S".into(), "F".into()), ("S".into(), "F1".into())));
    assert_eq!(Failure::from(&called).diagnostics.unwrap().context.as_deref(), Some("routine S.F"));
    let lost = Failure::from(&broken("gone"));
    assert_eq!(lost.diagnostics.as_ref().and_then(|given| given.severity.as_deref()), Some(FATAL));
    let bare = *Failure::from(&SqlError::new(Code::Data, "x")).diagnostics.unwrap();
    assert_eq!(bare, Diagnostics { severity: Some(ERROR.into()), ..Diagnostics::default() });
    assert!(lost.plain().diagnostics.is_none());
}

/// The frames of a Query of `n` copies of `text`, then a Terminate.
fn frames(text: &str, n: usize) -> Vec<u8> {
    let mut bytes = encoded(&Request::Query { sql: text.repeat(n), ends: false, attributes: Vec::new(), multi: false });
    bytes.extend(encoded(&Request::Terminate));
    bytes
}

/// Verifies: [DESIGN] RULES.md D196: a frame of at least the threshold is
/// sent compressed, as a `z` frame of its kind, its body's length and its
/// body's zlib stream, which inflates back to it; a frame below the
/// threshold, or that would come out no shorter, is sent as it is.
/// Boundary: a frame of exactly the threshold is compressed.
#[test]
fn frames_past_a_threshold_are_compressed() {
    let bytes = frames("SELECT 1 FROM T; ", 200);
    let mut out = Vec::new();
    compress_frames(&mut out, &bytes, DEFAULT_THRESHOLD);
    let terminate = encoded(&Request::Terminate);
    assert!(out.len().saturating_mul(4) < bytes.len() && out.ends_with(&terminate), "{} of {}", out.len(), bytes.len());
    let (kind, body) = unframed(&out[..out.len() - terminate.len()], MAX_BODY).unwrap();
    assert_eq!(kind, COMPRESSED);
    let (inner, inflated) = decompress(body, MAX_BODY).unwrap();
    assert_eq!((inner, inflated.as_slice()), (b'Q', &bytes[HEADER..bytes.len() - terminate.len()]));
    let small = frames("S", 1);
    let mut kept = Vec::new();
    compress_frames(&mut kept, &small, DEFAULT_THRESHOLD);
    assert_eq!(kept, small, "below the threshold");
    let exact = frames("a", DEFAULT_THRESHOLD - HEADER - 4 - 1);
    let mut at = Vec::new();
    compress_frames(&mut at, &exact[..exact.len() - terminate.len()], DEFAULT_THRESHOLD);
    assert_eq!(exact.len() - terminate.len(), DEFAULT_THRESHOLD);
    assert_eq!(at[0], COMPRESSED, "exactly the threshold");
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let noise: Vec<u8> = (0..3000_u32)
        .map(|_| {
            state ^= state << 13_u32;
            state ^= state >> 7_u32;
            state ^= state << 17_u32;
            state.to_le_bytes()[0]
        })
        .collect();
    let mut squeezed = Vec::new();
    let sent = encoded(&Request::Answer(noise));
    compress_frames(&mut squeezed, &sent, 1);
    assert!(squeezed == sent, "no shorter compressed: {} of {}", squeezed.len(), sent.len());
}

/// Verifies: [DESIGN] RULES.md D193, D196: a compressed frame is inflated
/// no further than the length it declares, itself at most the longest body
/// taken; one declaring more, inflating to another length, holding another
/// compressed frame, or not a zlib stream, is a connection failure.
/// Boundary: a length of exactly the longest taken inflates.
#[test]
fn compressed_frames_are_inflated_within_bounds() {
    let body = vec![b'x'; 5000];
    let stream = zlib::compress(&body);
    let framed = |kind: u8, length: u32, deflated: &[u8]| [&[kind][..], &length.to_le_bytes(), deflated].concat();
    assert_eq!(decompress(&framed(b'B', 5000, &stream), 5000).unwrap().1, body, "exactly the longest");
    let failed = |r: Result<(u8, Vec<u8>)>| r.err().map(|e| kind_of(&e));
    assert_eq!(failed(decompress(&framed(b'B', 5000, &stream), 4999)), Some(FAILED), "longer than taken");
    assert_eq!(failed(decompress(&framed(b'B', 4000, &stream), MAX_BODY)), Some(FAILED), "inflates past its length");
    assert_eq!(failed(decompress(&framed(b'B', 6000, &stream), MAX_BODY)), Some(FAILED), "inflates short of it");
    assert_eq!(failed(decompress(&framed(COMPRESSED, 5000, &stream), MAX_BODY)), Some(FAILED), "nested");
    assert_eq!(failed(decompress(&framed(b'B', 3, b"abc"), MAX_BODY)), Some(FAILED), "no stream");
    assert_eq!(failed(decompress(&[b'B', 1], MAX_BODY)), Some(FAILED), "cut short");
}

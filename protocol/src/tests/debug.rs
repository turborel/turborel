//! Unit tests of the routine debugger's messages (RULES.md D1030, D1032):
//! each read back as written, an Event never ending a request's replies,
//! and every malformed one refused (RULES.md R-TEST-1, R-TEST-2).

use turborel::catalog::QualifiedName;
use turborel::debugger::{Action, CursorShown, Event, Frame, Kind, Named, Origin, Paused, Reason};
use turborel::error::SqlState;
use turborel::value::{Decimal, Value};

use super::{FAILED, kind_of, reply_round_trip, request_round_trip};
use crate::*;

/// The name `schema.name`.
fn named(schema: &str, name: &str) -> QualifiedName {
    QualifiedName { auth: schema.into(), ident: name.into() }
}

/// A pause at breakpoint 2 in a procedure that a function it called runs
/// in, the outer frame's values shown, the inner's not.
fn pause() -> Paused {
    let outer = Frame {
        origin: Some(Origin { kind: Kind::Procedure, name: named("ANN", "RAISE_1"), shown: named("ANN", "RAISE") }),
        visible: true,
        at: Some(42),
        line: Some((3, 5)),
        parameters: vec![Named { name: "PCT".into(), data_type: "INTEGER".into(), value: Value::Exact(Decimal::new(10, 0)) }],
        variables: vec![Named { name: "N".into(), data_type: "CHARACTER VARYING(10)".into(), value: Value::Null }],
        cursors: vec![CursorShown { name: "C".into(), open: true, query: "SELECT * FROM T".into() }],
    };
    let inner =
        Frame { origin: Some(Origin { kind: Kind::Trigger, name: named("BOB", "T"), shown: named("BOB", "T") }), ..Frame::default() };
    Paused { reason: Reason::Breakpoint(2), frames: vec![outer, inner, Frame::default()] }
}

/// The breakpoint 1 at line 4 of ANN.RAISE_1, conditioned if `condition`.
fn breakpoint(condition: Option<&str>) -> DebugBreakpoint {
    DebugBreakpoint { number: 1, routine: named("ANN", "RAISE_1"), line: 4, condition: condition.map(str::to_owned) }
}

/// Verifies: [EXTENSION] RULES.md D1030: every Debug request, of each
/// kind, each action and a condition given or not, reads back as written.
#[test]
fn debug_requests_read_back_as_written() {
    let mut requests = vec![
        DebugRequest::Identify,
        DebugRequest::Attach(u64::MAX),
        DebugRequest::Detach,
        DebugRequest::Clear(7),
        DebugRequest::Breakpoints,
        DebugRequest::Interrupt,
        DebugRequest::Stack,
    ];
    for kind in [Kind::Procedure, Kind::Function, Kind::Trigger] {
        let routine = named("", "P");
        requests.push(DebugRequest::Break { kind, routine: routine.clone(), line: 1, condition: None });
        requests.push(DebugRequest::Break { kind, routine, line: u32::MAX, condition: Some("N > 1".into()) });
    }
    for how in [Action::Into, Action::Over, Action::Out, Action::Continue] {
        requests.push(DebugRequest::Resume(how));
    }
    for each in requests {
        request_round_trip(&Request::Debug(each));
    }
    let mut bytes = Vec::new();
    Request::Debug(DebugRequest::Attach(2)).encode(&mut bytes);
    assert_eq!(bytes, [b'V', 9, 0, 0, 0, 1, 2, 0, 0, 0, 0, 0, 0, 0]);
}

/// Verifies: [EXTENSION] RULES.md D1030, D1032: every Debug reply and
/// every Event reads back as written, a pause with its frames, their
/// values, cursors and places among them.
#[test]
fn debug_replies_and_events_read_back_as_written() {
    let replies = [
        Reply::Debug(DebugReply::Connection(3)),
        Reply::Debug(DebugReply::Breakpoint(breakpoint(None))),
        Reply::Debug(DebugReply::Breakpoints(vec![breakpoint(None), breakpoint(Some("PCT = 10"))])),
        Reply::Debug(DebugReply::Breakpoints(Vec::new())),
        Reply::Debug(DebugReply::Stack(None)),
        Reply::Debug(DebugReply::Stack(Some(pause()))),
        Reply::Event(Event::Paused(pause())),
        Reply::Event(Event::Paused(Paused { reason: Reason::Step, frames: Vec::new() })),
        Reply::Event(Event::TimedOut),
        Reply::Event(Event::Cancelled),
        Reply::Event(Event::Ended),
    ];
    for reply in &replies {
        reply_round_trip(reply);
    }
}

/// Verifies: [EXTENSION] RULES.md D1032, D393: an Event answers no
/// request, so a reader of a request's replies reads on past one; a Debug
/// reply ends its request's.
#[test]
fn an_event_ends_no_request() {
    let mut event = Vec::new();
    Reply::Event(Event::Ended).encode(&mut event);
    let mut debugged = Vec::new();
    Reply::Debug(DebugReply::Connection(1)).encode(&mut debugged);
    let body = |bytes: &[u8]| bytes[HEADER..].to_vec();
    assert!(!Reply::ends_request(b'W', &body(&event)));
    assert!(Reply::ends_request(b'V', &body(&debugged)));
}

/// Verifies: [EXTENSION] RULES.md D1030: a Debug request or reply, or an
/// Event, of no kind, of a kind of routine, an action, a pause's reason or
/// a flag out of range, or cut short or run long, is a connection failure
/// (SQLSTATE 08006).
#[test]
fn malformed_debug_messages_are_connection_failures() {
    let requests: [&[u8]; 7] = [
        &[9],
        &[],
        &[1, 2, 0],
        &[3, 3, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0],
        &[3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 2],
        &[6, 4],
        &[0, 0],
    ];
    for body in requests {
        let e = Request::decode(b'V', body).unwrap_err();
        assert_eq!(kind_of(&e), FAILED, "{body:?}: {e:?}");
    }
    let replies: [(u8, &[u8]); 7] = [
        (b'V', &[4]),
        (b'V', &[3, 2]),
        (b'V', &[3, 1, 2, 0, 0, 0, 0]),
        (b'W', &[4]),
        (b'W', &[0, 1, 1, 0, 0, 0, 1, 3]),
        (b'W', &[1, 0]),
        (b'W', &[]),
    ];
    for (kind, body) in replies {
        let e = Reply::decode(kind, body).unwrap_err();
        assert_eq!(kind_of(&e), FAILED, "{kind} {body:?}: {e:?}");
        assert_eq!(e.sqlstate(), SqlState::CONNECTION_FAILURE);
    }
}

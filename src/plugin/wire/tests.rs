//! Unit tests of the frames of protocol 1: each message written and read
//! back, and frames too long, cut short or of no message refused
//! (RULES.md R-TEST-1, R-TEST-2).

use super::*;

/// `m` framed and read back.
fn again(m: &Message) -> Message {
    let frame = m.frame().unwrap();
    let body = read_frame(&mut frame.as_slice(), 1 << 20).unwrap().unwrap();
    Message::decode(&body).unwrap()
}

/// Verifies: [DESIGN] RULES.md D167, D279: every message of protocol 1
/// reads back as framed, its length first, then its type.
#[test]
fn messages_read_back_as_framed() {
    let hello = Hello {
        protocol: 1,
        extension: "E".into(),
        capabilities: vec![BATCH.into()],
        max_message_bytes: 9,
        max_call_bytes: 8,
        memory_bytes: 7,
    };
    let welcome = Welcome { protocol: 1, name: "E".into(), version: "1.0.0".into(), capabilities: vec![], kinds: vec![1, 5] };
    let call = Call { call: 3, kind: 1, op: 1, object: "F".into(), payload: vec![1, 2] };
    let all = [
        Message::Hello(hello),
        Message::Welcome(welcome),
        Message::Call(call),
        Message::Result { call: 3, payload: vec![9] },
        Message::Error { call: 3, state: "22012".into(), message: "zero".into() },
        Message::Cancel { call: 3 },
        Message::Shutdown,
        Message::Sql { call: 3, rows: true, text: "VALUES 1".into(), args: vec![Value::Bool(true)] },
        Message::SqlDone { call: 3, rows: vec![vec![Value::Null], vec![]] },
        Message::Statistics { call: 3, asked: Asked::Column("U.T".into(), "A".into()) },
        Message::StatisticsDone { call: 3, payload: vec![0] },
    ];
    for m in all {
        assert_eq!(format!("{:?}", again(&m)), format!("{m:?}"));
        assert_eq!(m.frame().unwrap()[LENGTH_BYTES], m.code());
    }
    let short = Message::Error { call: 1, state: "22".into(), message: String::new() };
    let Message::Error { state, .. } = again(&short) else { panic!("an ERROR") };
    assert_eq!(state, "22000", "padded to five characters");
}

/// Verifies: [DESIGN] RULES.md D167, D170: a frame longer than the most
/// taken is 38T04, unread; one cut short is 38T01; the end between frames
/// is none; a body of no message, or with bytes after its end, is 38T03.
#[test]
fn malformed_frames_are_refused() {
    let state = |e: crate::error::SqlError| e.state.map(|s| s.as_str().to_owned()).unwrap_or_default();
    let frame = Message::Shutdown.frame().unwrap();
    assert_eq!(read_frame(&mut [9_u8, 0, 0, 0].as_slice(), 4).map_err(state).err().as_deref(), Some("38T04"));
    assert_eq!(read_frame(&mut [5_u8, 0, 0, 0, 7].as_slice(), 64).map_err(state).err().as_deref(), Some("38T01"));
    assert!(read_frame(&mut [].as_slice(), 64).unwrap().is_none());
    assert_eq!(read_frame(&mut frame.as_slice(), 64).unwrap(), Some(vec![SHUTDOWN]));
    for body in [&[][..], &[99], &[SHUTDOWN, 0], &[CANCEL, 1, 2]] {
        assert_eq!(Message::decode(body).map_err(state).err().as_deref(), Some("38T03"), "{body:?}");
    }
}

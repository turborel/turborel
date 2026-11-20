//! Unit tests: each of VR's messages as a frame, and its state as a
//! journal keeps it, read back as written; each cut short at any byte, of
//! no tag, or of a log not its numbers', refused (RULES.md D57, R-TEST-1,
//! R-TEST-2).

use super::*;
use crate::Member;
use turborel::error::Code;

/// A group of two members.
fn members() -> Configuration {
    vec![Member { id: ReplicaId(1), address: "a:1".into() }, Member { id: ReplicaId(2), address: "b:2".into() }]
}

/// A mark of the next epoch, 2.
fn mark() -> Mark {
    Mark { epoch: 2, configuration: members() }
}

/// A log of two records after 4, marked at 6.
fn log() -> (Vec<Vec<u8>>, Marks) {
    (vec![vec![1], vec![2, 3]], Marks::from([(6, mark())]))
}

/// Each message, of every field.
fn every_message() -> Vec<Message> {
    let (records, marks) = log();
    let primary = PrimaryLog { after: 4, log: records.clone(), marks: marks.clone(), op: 6, commit: 5 };
    vec![
        Message::Prepare(Prepare { view: 1, record: vec![9], op: 7, commit: 6, mark: Some(mark()) }),
        Message::Prepare(Prepare { view: 1, record: Vec::new(), op: 8, commit: 6, mark: None }),
        Message::PrepareOk(PrepareOk { view: 1, op: 7, replica: ReplicaId(2) }),
        Message::Commit(Commit { view: 1, commit: 7 }),
        Message::StartViewChange(StartViewChange { view: 2, replica: ReplicaId(1) }),
        Message::DoViewChange(DoViewChange {
            view: 2,
            after: 4,
            log: records.clone(),
            marks: marks.clone(),
            normal_view: 1,
            op: 6,
            commit: 5,
            replica: ReplicaId(2),
        }),
        Message::StartView(StartView {
            view: 2,
            after: 4,
            log: records.clone(),
            marks: marks.clone(),
            op: 6,
            commit: 5,
            configuration: members(),
            began: 3,
        }),
        Message::GetState(GetState { view: 2, op: 4, replica: ReplicaId(1) }),
        Message::NewState(NewState { view: 2, after: 4, log: records, marks, op: 6, commit: 5, configuration: members(), began: 3 }),
        Message::NeedState(NeedState { view: 2, op: 1, configuration: members(), began: 3 }),
        Message::Recovery(Recovery { replica: ReplicaId(2), nonce: 77 }),
        Message::RecoveryResponse(RecoveryResponse {
            view: 2,
            nonce: 77,
            replica: ReplicaId(1),
            configuration: members(),
            began: 3,
            log: Some(primary),
        }),
        Message::RecoveryResponse(RecoveryResponse {
            view: 2,
            nonce: 78,
            replica: ReplicaId(2),
            configuration: members(),
            began: 3,
            log: None,
        }),
        Message::EpochStarted(EpochStarted { replica: ReplicaId(2), configuration: members() }),
    ]
}

/// Verifies: [EXTENSION] RULES.md D57 [VRR §4, §5.2, §7]: each message as
/// a frame is read back as written, with the epoch it was sent in, and
/// named by [VRR]'s name.
#[test]
fn each_message_is_read_as_written() {
    let names: Vec<&str> = every_message().iter().map(Message::kind).collect();
    assert_eq!(names.len(), 14);
    assert!(names.contains(&"EPOCHSTARTED") && names.contains(&"NEEDSTATE") && names.contains(&"PREPAREOK"));
    for m in every_message() {
        let frame = m.encode(5);
        assert_eq!(Message::decode(&frame).unwrap(), (5, m));
    }
}

/// Verifies: [DESIGN] RULES.md D57 robustness: a frame of each message cut
/// short at any byte, or followed by one more, is refused as a malformed
/// frame received, never read as another.
#[test]
fn each_frame_cut_short_is_refused() {
    for m in every_message() {
        let frame = m.encode(5);
        for end in 0..frame.len() {
            assert_eq!(Message::decode(&frame[..end]).unwrap_err().code, Code::Connection, "{} cut at {end}", m.kind());
        }
        let mut longer = frame.clone();
        longer.push(0);
        assert!(Message::decode(&longer).is_err(), "{} with a byte more", m.kind());
    }
}

/// Verifies: [DESIGN] RULES.md D57 robustness: a frame of no tag, a flag
/// neither 0 nor 1, a log whose length is not its op-number's distance
/// from the position before it, or a mark outside its log, is refused.
#[test]
fn a_frame_of_wrong_numbers_is_refused() {
    let mut no_tag = Message::Commit(Commit { view: 1, commit: 1 }).encode(1);
    no_tag[0] = 99;
    assert!(Message::decode(&no_tag).is_err());
    let mut flagged = Message::Prepare(Prepare { view: 1, record: Vec::new(), op: 1, commit: 0, mark: None }).encode(1);
    *flagged.last_mut().unwrap() = 2;
    assert!(Message::decode(&flagged).is_err(), "a flag of 2");
    let (records, marks) = log();
    let view = |op: u64, marked: Marks| {
        let m = StartView { view: 2, after: 4, log: records.clone(), marks: marked, op, commit: 5, configuration: members(), began: 3 };
        Message::StartView(m).encode(1)
    };
    assert!(Message::decode(&view(6, marks)).is_ok());
    assert!(Message::decode(&view(7, Marks::new())).is_err(), "two records after 4 end at 6, not 7");
    assert!(Message::decode(&view(6, Marks::from([(4, mark())]))).is_err(), "a mark at the position before the log");
    assert!(Message::decode(&view(6, Marks::from([(7, mark())]))).is_err(), "a mark past the log");
}

/// Verifies: [EXTENSION] RULES.md D57: VR's state as a journal keeps it
/// is read back as written; one of another format, cut short, followed
/// by more, or of a normal view after its view is refused as corrupt.
#[test]
fn the_state_is_read_as_written() {
    let state = ViewState { epoch: 2, began: 3, configuration: members(), view: 4, normal_view: 4, commit: 9, removed: true };
    let bytes = state.encode();
    assert_eq!(ViewState::decode(&bytes).unwrap(), state);
    for end in 0..bytes.len() {
        assert_eq!(ViewState::decode(&bytes[..end]).unwrap_err().code, Code::Corrupt, "cut at {end}");
    }
    let mut other = bytes.clone();
    other[0] = 2;
    assert!(ViewState::decode(&other).is_err(), "format 2");
    let mut longer = bytes;
    longer.push(0);
    assert!(ViewState::decode(&longer).is_err());
    let ahead = ViewState { normal_view: 5, ..state };
    assert!(ViewState::decode(&ahead.encode()).is_err(), "a normal view after its view");
}

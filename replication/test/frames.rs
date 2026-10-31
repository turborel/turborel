//! VR's frames: those malformed or misattributed refused, those out of
//! place dropped, never a panic.

use turborel_replication::{Input, Output, ReplicaEvent, ReplicaId};

use crate::common::{record, serving};

const PRIMARY: ReplicaId = ReplicaId(1);
const BACKUP: ReplicaId = ReplicaId(2);
const OTHER: ReplicaId = ReplicaId(3);

/// A frame of tag `tag`, then `values` as little-endian integers.
fn frame(tag: u8, values: &[u64]) -> Vec<u8> {
    let mut bytes = vec![tag];
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

/// A GETSTATE of view 0, op-number `op`, from `replica`.
fn get_state(op: u64, replica: ReplicaId) -> Vec<u8> {
    let mut bytes = frame(7, &[0, op]);
    bytes.extend_from_slice(&replica.0.to_le_bytes());
    bytes
}

/// Whether `out` is exactly one refusal of a frame from `from`.
fn refused(out: &[Output], from: ReplicaId) -> bool {
    matches!(out, [Output::Event(ReplicaEvent::FrameRefused { from: f, .. })] if *f == from)
}

/// Verifies: [DESIGN] RULES.md D57 robustness: a frame empty, of no tag,
/// ending too soon or followed by more bytes, or whose log is not as long
/// as its op-number says, is refused with an event naming its sender, and
/// changes nothing.
#[test]
fn malformed_frames_are_refused() {
    let mut group = serving(3);
    group.step(PRIMARY, Input::Append(record("a")));
    let prepare = group.frames.iter().find(|f| f.to == BACKUP).unwrap().bytes.clone();
    group.frames.clear();
    let mut longer = prepare.clone();
    longer.push(0);
    let mut new_state = frame(8, &[0, 0, 2, 0, 1]);
    new_state.extend_from_slice(&1u32.to_le_bytes());
    new_state.push(7);
    let cases = [vec![], vec![99], prepare[..prepare.len() - 1].to_vec(), longer, frame(3, &[0]), new_state];
    for bytes in cases {
        let out = group.step(BACKUP, Input::Received { from: PRIMARY, frame: bytes.clone() });
        assert!(refused(&out, PRIMARY), "{bytes:?}: {out:?}");
    }
    assert_eq!(group.node(BACKUP).journal.records(), Vec::<Vec<u8>>::new());
    let out = group.step(BACKUP, Input::Received { from: PRIMARY, frame: prepare });
    assert!(matches!(out[..], [Output::Persist { .. }, Output::Send { .. }]), "the frame whole is taken: {out:?}");
}

/// Verifies: [DESIGN] RULES.md D57 robustness: a frame from no member, from
/// the replica itself, a PREPARE or COMMIT from no primary of its view, and
/// a message naming a replica other than its sender, are refused.
#[test]
fn misattributed_frames_are_refused() {
    let mut group = serving(3);
    group.step(PRIMARY, Input::Append(record("a")));
    let prepare = group.frames.iter().find(|f| f.to == BACKUP).unwrap().bytes.clone();
    let cases = [
        (ReplicaId(9), BACKUP, prepare.clone()),
        (BACKUP, BACKUP, prepare.clone()),
        (OTHER, BACKUP, prepare),
        (OTHER, BACKUP, frame(3, &[0, 0])),
        (OTHER, PRIMARY, get_state(0, BACKUP)),
    ];
    for (from, to, bytes) in cases {
        let out = group.step(to, Input::Received { from, frame: bytes });
        assert!(refused(&out, from), "{from:?} to {to:?}: {out:?}");
    }
}

/// Verifies: [DESIGN] RULES.md D57 [VRR §4.1, §5.2] messages out of place
/// are dropped: of another view; a PREPARE, COMMIT or NEWSTATE to the
/// primary; a PREPAREOK to a backup; a GETSTATE past the receiver's log or
/// of another view; a NEWSTATE beginning past the receiver's log.
#[test]
fn messages_out_of_place_are_dropped() {
    let mut group = serving(3);
    let new_state = {
        let mut bytes = frame(8, &[0, 3, 4, 0, 1]);
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.push(7);
        bytes
    };
    let mut prepare_ok = frame(2, &[0, 1]);
    prepare_ok.extend_from_slice(&OTHER.0.to_le_bytes());
    let mut later_get_state = frame(7, &[1, 0]);
    later_get_state.extend_from_slice(&OTHER.0.to_le_bytes());
    let cases = [
        (PRIMARY, BACKUP, frame(3, &[3, 0])),
        (BACKUP, PRIMARY, frame(8, &[0, 0, 0, 0, 0])),
        (OTHER, BACKUP, prepare_ok),
        (OTHER, PRIMARY, get_state(5, OTHER)),
        (OTHER, PRIMARY, later_get_state),
        (PRIMARY, BACKUP, new_state),
    ];
    for (from, to, bytes) in cases {
        assert_eq!(group.step(to, Input::Received { from, frame: bytes.clone() }), Vec::new(), "{from:?} to {to:?}: {bytes:?}");
    }
    assert_eq!(group.node(BACKUP).journal.records(), Vec::<Vec<u8>>::new());
}

//! The contract tests of replication's extension points (RULES.md D57,
//! R-SAFE-3): what every `Consensus` and every `Transport` must do, as
//! their documentation states it, run against each implementation. That a
//! group of replicas stays safe and converges through faults is the
//! simulator's (`sim`).

use turborel_replication::{Input, Lsn, Output, ReplicaEvent, ReplicaId, Role, Transport};

use super::group::Make;
use super::sim::Rng;

/// Ticks within which a new group of one serves.
const SERVE_WITHIN: usize = 100;
/// Inputs fed two machines made alike.
const INPUTS: usize = 2000;

/// Checks a `Consensus` made by `make`: a new group of one serves, takes
/// appends at the next positions, persisting each, and commits each once
/// durable, never going back; frames malformed or from no member are
/// refused with an event and change nothing; and it is deterministic.
pub fn check_consensus(make: &Make) {
    serves_alone(make);
    refuses_frames(make);
    is_deterministic(make);
}

/// A new group of one serves within a few ticks; then appends, persists and
/// commits in order.
fn serves_alone(make: &Make) {
    let me = ReplicaId(1);
    let mut consensus = make(&[me], me, None);
    let mut out = Vec::new();
    for _ in 0..SERVE_WITHIN {
        consensus.step(Input::Tick, &mut out);
        if out.contains(&Output::Role(Role::Primary { view: 0 })) {
            break;
        }
    }
    assert!(out.iter().any(|o| matches!(o, Output::Role(Role::Primary { .. }))), "{}: a group of one does not serve", consensus.name());
    let mut committed = 0;
    for at in 1..=3 {
        let record = vec![at];
        let mut appended = Vec::new();
        consensus.step(Input::Append(record.clone()), &mut appended);
        let position = Lsn(u64::from(at));
        assert_eq!(appended[..2], [Output::Assigned(position), Output::Persist { at: position, record }], "{}", consensus.name());
        let mut done = Vec::new();
        consensus.step(Input::Persisted(position), &mut done);
        for output in done {
            if let Output::Committed(through) = output {
                assert!(through.0 >= committed, "{}: commit went back", consensus.name());
                committed = through.0;
            }
        }
        assert_eq!(committed, u64::from(at), "{}: a record durable in a group of one is committed", consensus.name());
    }
}

/// Frames malformed, from no member or from itself, are refused with one
/// event each, and nothing else.
fn refuses_frames(make: &Make) {
    let members = [ReplicaId(1), ReplicaId(2), ReplicaId(3)];
    let mut consensus = make(&members, ReplicaId(2), None);
    let cases = [
        (ReplicaId(1), vec![]),
        (ReplicaId(1), vec![0]),
        (ReplicaId(1), vec![u8::MAX; 9]),
        (ReplicaId(9), vec![1]),
        (ReplicaId(2), vec![1]),
    ];
    for (from, frame) in cases {
        let mut out = Vec::new();
        consensus.step(Input::Received { from, frame: frame.clone() }, &mut out);
        let refused = matches!(out[..], [Output::Event(ReplicaEvent::FrameRefused { .. })]);
        assert!(refused, "{}: a frame {frame:?} from {from:?} gave {out:?}", consensus.name());
    }
}

/// Two machines made alike, fed the same inputs at random, give the same
/// outputs.
fn is_deterministic(make: &Make) {
    let members = [ReplicaId(1), ReplicaId(2), ReplicaId(3)];
    let mut first = make(&members, ReplicaId(1), None);
    let mut second = make(&members, ReplicaId(1), None);
    let mut rng = Rng::new(0);
    let mut written = 0;
    for _ in 0..INPUTS {
        let input = match rng.below(4) {
            0 => Input::Tick,
            1 => Input::Append(rng.bits().to_le_bytes().to_vec()),
            2 => {
                written += 1;
                Input::Persisted(Lsn(written))
            }
            _ => Input::Received { from: ReplicaId(2), frame: rng.bits().to_le_bytes()[..usize::try_from(rng.below(8)).unwrap()].to_vec() },
        };
        let (mut a, mut b) = (Vec::new(), Vec::new());
        first.step(input.clone(), &mut a);
        second.step(input, &mut b);
        assert_eq!(a, b, "{}: not deterministic", first.name());
    }
}

/// Checks a `Transport` of a group whose members do not include
/// `outsider`: its members listed in order, each once; a send to each
/// taken; a send to `outsider` refused (SQLCODE -910).
pub fn check_transport(transport: &dyn Transport, outsider: ReplicaId) {
    let members = transport.members();
    assert_ne!(members.len(), 0, "a group has members");
    assert!(members.windows(2).all(|pair| pair[0] < pair[1]), "members in order, each once: {members:?}");
    for &to in members {
        transport.send(to, b"frame").unwrap();
    }
    assert_eq!(transport.send(outsider, b"frame").unwrap_err().sqlcode(), -910_i32);
}

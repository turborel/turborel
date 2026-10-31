//! VR's view change [VRR §4.2], state transfer to a later view [VRR §5.2],
//! and replicas made again from their journals, step by step.

use turborel_replication::vr::Status;
use turborel_replication::{Consensus, Input, Lsn, Output, Recovered, ReplicaEvent, ReplicaId, Role, Vr};

use crate::common::{committed, ids, options, record, run, served, serving, settle_without, vr_group};

const ONE: ReplicaId = ReplicaId(1);
const TWO: ReplicaId = ReplicaId(2);
const THREE: ReplicaId = ReplicaId(3);

/// Ticks enough for a view change to time out and complete.
const TIMEOUT: u32 = 12;

/// The tag of STARTVIEW frames.
const START_VIEW: u8 = 6;
/// The tag of GETSTATE frames.
const GET_STATE: u8 = 7;

/// A group of three whose primary, replica 1, committed "a" and was then
/// cut off, and whose backups moved to view 1, replica 2 its primary.
fn failed_over() -> turborel_test_support::replication::Group {
    let mut group = serving(3);
    group.step(ONE, Input::Append(record("a")));
    group.settle();
    run(&mut group, &[TWO, THREE], TIMEOUT, &[ONE]);
    group
}

/// The `Outcome`s among `outputs`.
fn outcomes(outputs: &[Output]) -> Vec<(u64, bool)> {
    outputs.iter().filter_map(|o| if let Output::Outcome { at, kept } = o { Some((at.0, *kept)) } else { None }).collect()
}

/// Verifies: [DESIGN] RULES.md D57 [VRR §4.2 steps 1-5] backups that hear
/// nothing from their primary start a view change; the next primary takes
/// the log of the DOVIEWCHANGEs, starts the view, has the log it took over
/// committed and delivered to its database, and only then serves; a record
/// committed in the old view keeps its position.
#[test]
fn backups_fail_over_to_the_next_primary() {
    let mut group = failed_over();
    let two = &group.node(TWO);
    assert_eq!(served(&two.outputs), vec![1]);
    assert!(two.outputs.contains(&Output::Event(ReplicaEvent::ViewChanged { view: 1, primary: TWO })));
    assert_eq!(two.applied, vec![record("a")]);
    assert!(group.node(THREE).outputs.contains(&Output::Event(ReplicaEvent::ViewChanged { view: 1, primary: TWO })));
    assert_eq!(group.step(TWO, Input::Append(record("b")))[0], Output::Assigned(Lsn(2)));
    settle_without(&mut group, &[ONE]);
    assert_eq!(committed(&group.node(TWO).outputs), vec![2]);
    assert_eq!(group.node(THREE).journal.records(), &[record("a"), record("b")]);
}

/// Verifies: [DESIGN] RULES.md D57 [VRR §4.2, §5.2] a deposed primary that
/// hears from a later view stops serving, fetches the view's log, and its
/// record not committed, replaced there, is resolved as not kept; it then
/// holds and delivers the new view's log.
#[test]
fn a_replaced_record_is_resolved_as_not_kept() {
    let mut group = serving(3);
    group.step(ONE, Input::Append(record("a")));
    group.settle();
    group.step(ONE, Input::Append(record("x")));
    group.frames.clear();
    run(&mut group, &[TWO, THREE], TIMEOUT, &[ONE]);
    group.step(TWO, Input::Append(record("y")));
    settle_without(&mut group, &[ONE]);
    run(&mut group, &[TWO], 2, &[]);
    let one = group.node(ONE);
    assert!(one.outputs.contains(&Output::Role(Role::Backup)));
    assert_eq!(outcomes(&one.outputs), vec![(2, false)]);
    assert_eq!(one.journal.records(), &[record("a"), record("y")]);
    assert_eq!(one.applied, vec![record("a"), record("y")]);
}

/// Verifies: [DESIGN] RULES.md D57 [VRR §4.2 step 3] a record its backups
/// held durably, though the deposed primary never learned it committed,
/// survives the view change and is resolved as kept.
#[test]
fn a_record_the_quorum_held_is_kept() {
    let mut group = serving(3);
    group.step(ONE, Input::Append(record("a")));
    group.settle();
    group.step(ONE, Input::Append(record("b")));
    loop {
        group.drop_frames(|f| f.to == ONE);
        if !group.deliver() {
            break;
        }
    }
    run(&mut group, &[TWO, THREE], TIMEOUT, &[ONE]);
    assert_eq!(group.node(TWO).applied, vec![record("a"), record("b")]);
    run(&mut group, &[TWO], 2, &[]);
    assert_eq!(outcomes(&group.node(ONE).outputs), vec![(2, true)]);
}

/// Verifies: [DESIGN] RULES.md D57 [VRR §5.2] a replica whose STARTVIEW was
/// lost learns of the view from its primary's PREPARE, fetches its log
/// (GETSTATE, NEWSTATE) and is normal in it.
#[test]
fn a_lost_start_view_is_made_up_by_state_transfer() {
    let mut group = serving(3);
    group.step(ONE, Input::Append(record("a")));
    group.settle();
    for _ in 0..TIMEOUT {
        for id in [TWO, THREE] {
            group.step(id, Input::Tick);
        }
        loop {
            group.drop_frames(|f| f.from == ONE || f.to == ONE || f.bytes[0] == START_VIEW);
            if !group.deliver() {
                break;
            }
        }
        if served(&group.node(TWO).outputs) == [1] {
            break;
        }
    }
    assert_eq!(served(&group.node(TWO).outputs), vec![1]);
    let three = &group.node(THREE).outputs;
    let asked = three.iter().any(|o| if let Output::Send { to: TWO, frame } = o { frame[0] == GET_STATE } else { false });
    assert!(asked, "it asked for the log");
    assert!(three.contains(&Output::Event(ReplicaEvent::ViewChanged { view: 1, primary: TWO })));
}

/// Verifies: [DESIGN] RULES.md D57 [VRR §4.2] "A view change may not
/// succeed, e.g., because the new primary fails. In this case the replicas
/// will start a further view change, with yet another primary": of five,
/// with the primaries of views 0 and 1 down, view 2's serves.
#[test]
fn a_failed_view_change_moves_to_the_next_view() {
    let mut group = vr_group(5);
    group.tick();
    group.settle();
    group.step(ONE, Input::Append(record("a")));
    group.settle();
    run(&mut group, &[THREE, ReplicaId(4), ReplicaId(5)], 3 * TIMEOUT, &[ONE, TWO]);
    assert_eq!(served(&group.node(THREE).outputs), vec![2]);
    assert_eq!(group.node(THREE).applied, vec![record("a")]);
}

/// Verifies: [DESIGN] RULES.md D57 [VRR §4.3] "If nodes record their state
/// on disk before sending messages, a node will be able to rejoin the
/// system as soon as it has reinitialized its state by reading from disk":
/// a backup that lost writes not yet durable rejoins in its view and is
/// sent them again.
#[test]
fn a_backup_made_again_rejoins_its_view() {
    let mut group = serving(3);
    group.instant_disks = false;
    group.step(ONE, Input::Append(record("a")));
    group.settle();
    group.flush(TWO);
    group.flush(ONE);
    group.settle();
    group.crash(THREE, 0);
    assert_eq!(group.node(THREE).journal.records(), Vec::<Vec<u8>>::new());
    group.instant_disks = true;
    run(&mut group, &[ONE, TWO, THREE], 2, &[]);
    assert_eq!(group.node(THREE).journal.records(), &[record("a")]);
    assert_eq!(group.node(THREE).applied, vec![record("a")]);
}

/// Verifies: [DESIGN] RULES.md D57 a primary made again never serves in its
/// old view, whose records its backups may have committed without its own
/// copy: it waits in a view change, and rejoins the next view as a backup.
#[test]
fn a_primary_made_again_does_not_serve_its_old_view() {
    let mut group = serving(3);
    group.instant_disks = false;
    group.step(ONE, Input::Append(record("a")));
    group.settle();
    group.flush(TWO);
    group.flush(THREE);
    group.settle();
    assert_eq!(committed(&group.node(ONE).outputs), vec![1]);
    group.crash(ONE, 0);
    group.instant_disks = true;
    assert_eq!(group.step(ONE, Input::Append(record("x"))), vec![Output::Event(ReplicaEvent::AppendRefused)]);
    run(&mut group, &[ONE, TWO, THREE], TIMEOUT, &[]);
    assert_eq!(served(&group.node(TWO).outputs), vec![1]);
    let one = group.node(ONE);
    assert_eq!(one.journal.records(), &[record("a")]);
    assert_eq!(served(&one.outputs), vec![0], "only before the crash");
}

/// Verifies: [DESIGN] RULES.md D57 a group of one, made again, changes view
/// alone, delivers its log to its database and serves again.
#[test]
fn a_group_of_one_serves_again_after_a_crash() {
    let mut group = vr_group(1);
    group.tick();
    group.step(ONE, Input::Append(record("a")));
    group.crash(ONE, 0);
    run(&mut group, &[ONE], TIMEOUT, &[]);
    let one = group.node(ONE);
    assert_eq!(served(&one.outputs), vec![0, 1]);
    assert_eq!(one.applied, vec![record("a")]);
}

/// Verifies: [DESIGN] RULES.md D57 robustness: a state that is malformed,
/// of another format, or names a normal view after its view, is corruption
/// (SQLCODE -916); a state well formed restarts in its view.
#[test]
fn a_damaged_state_is_refused() {
    let state = |format: u8, view: u64, normal: u64| {
        let mut bytes = vec![format];
        bytes.extend_from_slice(&view.to_le_bytes());
        bytes.extend_from_slice(&normal.to_le_bytes());
        bytes
    };
    for bytes in [vec![1], state(2, 1, 1), state(1, 1, 2), [state(1, 1, 1), vec![0]].concat()] {
        let recovered = Recovered { state: Some(bytes.clone()), records: Vec::new() };
        assert_eq!(Vr::restart(&ids(3), TWO, options(), recovered).unwrap_err().sqlcode(), -916_i32, "{bytes:?}");
    }
    let normal = Recovered { state: Some(state(1, 3, 3)), records: vec![record("a")] };
    let vr = Vr::restart(&ids(3), TWO, options(), normal).unwrap();
    assert_eq!((vr.standing().view, vr.standing().status, vr.standing().durable), (3, Status::Normal, Lsn(1)));
    let changing = Recovered { state: Some(state(1, 5, 4)), records: Vec::new() };
    assert_eq!(Vr::restart(&ids(3), TWO, options(), changing).unwrap().standing().status, Status::ViewChange);
}

/// Verifies: [DESIGN] RULES.md D57 [VRR §4.1] "If the sender is behind, the
/// receiver drops the message": a PREPARE, COMMIT, STARTVIEWCHANGE,
/// DOVIEWCHANGE or STARTVIEW of an earlier view changes nothing.
#[test]
fn messages_of_earlier_views_are_dropped() {
    let mut group = failed_over();
    let mut vr = Vr::new(&ids(3), ONE, options()).unwrap();
    let mut out = Vec::new();
    vr.step(Input::Tick, &mut out);
    vr.step(Input::Append(record("z")), &mut out);
    let old: Vec<Vec<u8>> =
        out.iter().filter_map(|o| if let Output::Send { frame, .. } = o { Some(frame.clone()) } else { None }).collect();
    for frame in old {
        assert_eq!(group.step(THREE, Input::Received { from: ONE, frame }), Vec::new());
    }
    let mut start = vec![4];
    start.extend_from_slice(&0u64.to_le_bytes());
    start.extend_from_slice(&ONE.0.to_le_bytes());
    assert_eq!(group.step(THREE, Input::Received { from: ONE, frame: start }), Vec::new());
    assert_eq!(group.node(THREE).journal.records(), &[record("a")]);
    let _ = vr.name();
}

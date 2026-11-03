//! VR's normal operation [VRR §4.1], step by step.

use turborel_replication::vr::{Standing, Status};
use turborel_replication::{Consensus, Input, Lsn, Output, ReplicaEvent, ReplicaId, Role, Vr, VrOptions};

use turborel_test_support::replication::configuration;

use crate::common::{committed, config, ids, options, record, serving, vr_group};

const PRIMARY: ReplicaId = ReplicaId(1);
const BACKUP: ReplicaId = ReplicaId(2);
const OTHER: ReplicaId = ReplicaId(3);

/// Verifies: [DESIGN] RULES.md D57 [VRR §4] the primary of view 0 is the
/// first member, which serves as it opens, its view written first and its
/// epoch told; an
/// append is given the next position and written to the journal; a group
/// of one commits it once durable, and never before.
#[test]
fn one_replica_commits_once_its_record_is_durable() {
    let mut group = vr_group(1);
    group.instant_disks = false;
    let opened = &group.node(PRIMARY).outputs;
    assert!(
        matches!(opened[..], [Output::PersistState(_), Output::Epoch { .. }, Output::Event(_), Output::Role(Role::Primary { view: 0 })]),
        "{opened:?}"
    );
    assert_eq!(group.step(PRIMARY, Input::Tick), Vec::new());
    let out = group.step(PRIMARY, Input::Append(record("a")));
    assert_eq!(out, vec![Output::Assigned(Lsn(1)), Output::Persist { at: Lsn(1), record: record("a") }]);
    assert_eq!(group.step(PRIMARY, Input::Append(record("b")))[0], Output::Assigned(Lsn(2)));
    assert_eq!(committed(&group.node(PRIMARY).outputs), Vec::<u64>::new());
    assert_eq!(committed(&group.flush(PRIMARY)), vec![1, 2]);
}

/// Verifies: [DESIGN] RULES.md D57 [VRR §4.1 steps 4, 5] a backup sends
/// PREPAREOK only once the record is durable in its journal (a departure
/// from [VRR]), and the primary counts itself toward the quorum only once
/// its own copy is durable: MC/DC of the quorum, primary durable or not
/// with one backup durable.
#[test]
fn commit_waits_for_a_quorum_holding_the_record_durably() {
    let mut group = serving(3);
    group.instant_disks = false;
    group.step(PRIMARY, Input::Append(record("a")));
    group.settle();
    // Both backups hold the record, neither durably: no PREPAREOK yet.
    assert!(group.frames.is_empty());
    group.flush(BACKUP);
    group.settle();
    assert_eq!(committed(&group.node(PRIMARY).outputs), Vec::<u64>::new(), "one backup, the primary not durable");
    assert_eq!(committed(&group.flush(PRIMARY)), vec![1], "one backup, the primary durable");
}

/// Verifies: [DESIGN] RULES.md D57 [VRR §4.1 step 5] f + 1 backups holding a
/// record durably commit it, though the primary's own copy is not durable.
#[test]
fn backups_alone_may_make_a_quorum() {
    let mut group = serving(3);
    group.instant_disks = false;
    group.step(PRIMARY, Input::Append(record("a")));
    group.settle();
    group.flush(BACKUP);
    group.flush(OTHER);
    group.settle();
    assert_eq!(committed(&group.node(PRIMARY).outputs), vec![1]);
}

/// Verifies: [DESIGN] RULES.md D57 [VRR §4.1 steps 6, 7] backups learn of
/// commits from the next PREPARE or the primary's COMMIT, and are delivered
/// the committed records in order; the primary's database, which wrote
/// them, is delivered none.
#[test]
fn backups_deliver_committed_records_in_order() {
    let mut group = serving(3);
    for text in ["a", "b", "c"] {
        group.step(PRIMARY, Input::Append(record(text)));
        group.settle();
    }
    assert_eq!(committed(&group.node(PRIMARY).outputs), vec![1, 2, 3]);
    // The last PREPARE carried commit-number 2.
    assert_eq!(group.node(BACKUP).applied, vec![record("a"), record("b")]);
    group.tick();
    group.tick();
    group.settle();
    for backup in [BACKUP, OTHER] {
        assert_eq!(group.node(backup).applied, vec![record("a"), record("b"), record("c")]);
    }
    assert_eq!(group.node(PRIMARY).applied, Vec::<Vec<u8>>::new());
    assert!(!group.node(BACKUP).outputs.iter().any(|o| matches!(o, Output::Committed(_))), "only a serving primary is told of commits");
}

/// Verifies: [DESIGN] RULES.md D57 [VRR §5.2] a backup that missed PREPAREs
/// asks for the log after its op-number (GETSTATE) and appends what it is
/// sent (NEWSTATE), so it holds the primary's log.
#[test]
fn a_backup_behind_catches_up_by_state_transfer() {
    let mut group = serving(3);
    for text in ["a", "b"] {
        group.step(PRIMARY, Input::Append(record(text)));
    }
    group.drop_frames(|f| f.to == OTHER);
    group.step(PRIMARY, Input::Append(record("c")));
    group.settle();
    assert_eq!(group.node(OTHER).journal.records(), group.node(PRIMARY).journal.records());
}

/// Verifies: [DESIGN] RULES.md D57 [VRR §4.1] the primary sends again, at
/// each heartbeat, the records a backup has not acknowledged, and a COMMIT
/// to a backup that has them all; a PREPARE held already is acknowledged
/// again.
#[test]
fn heartbeats_send_again_what_was_not_acknowledged() {
    let mut group = serving(3);
    group.step(PRIMARY, Input::Append(record("a")));
    group.drop_frames(|f| f.to == BACKUP);
    group.settle();
    group.tick();
    group.tick();
    group.settle();
    assert_eq!(group.node(BACKUP).journal.records(), &[record("a")]);
    // Its PREPAREOK lost, the backup is sent the record again, and answers.
    group.step(PRIMARY, Input::Append(record("b")));
    group.drop_frames(|f| f.from == BACKUP);
    group.tick();
    group.tick();
    let resent = group.frames.len();
    group.settle();
    assert!(resent >= 2, "{resent}");
    assert_eq!(group.node(BACKUP).journal.records(), &[record("a"), record("b")]);
}

/// Verifies: [DESIGN] RULES.md D57 robustness: an append to a replica that
/// serves as no primary is refused with an event, and nothing else.
#[test]
fn an_append_to_a_backup_is_refused() {
    let mut group = serving(3);
    assert_eq!(group.step(BACKUP, Input::Append(record("a"))), vec![Output::Event(ReplicaEvent::AppendRefused)]);
}

/// Verifies: [DESIGN] RULES.md D57 robustness: a `Persisted` that is not for
/// the next position is ignored; one for a position already durable
/// commits nothing again.
#[test]
fn a_persisted_out_of_order_is_ignored() {
    let mut group = vr_group(1);
    group.instant_disks = false;
    group.step(PRIMARY, Input::Tick);
    group.step(PRIMARY, Input::Append(record("a")));
    assert_eq!(group.step(PRIMARY, Input::Persisted(Lsn(5))), Vec::new());
    assert_eq!(committed(&group.step(PRIMARY, Input::Persisted(Lsn(1)))), vec![1]);
    assert_eq!(group.step(PRIMARY, Input::Persisted(Lsn(1))), Vec::new());
}

/// Verifies: [DESIGN] RULES.md D57 robustness: a group of no members, or
/// naming one twice, a replica not among them, and options of 0 ticks or
/// records, are refused (SQLCODE -910).
#[test]
fn bad_configurations_are_refused() {
    let zero_heartbeat = VrOptions { heartbeat_ticks: 0, ..options() };
    let zero_timeout = VrOptions { view_timeout_ticks: 0, ..options() };
    let zero_resend = VrOptions { resend_records: 0, ..options() };
    let cases = [
        (vec![], PRIMARY, options()),
        (vec![PRIMARY, PRIMARY], PRIMARY, options()),
        (ids(3), ReplicaId(4), options()),
        (ids(3), PRIMARY, zero_heartbeat),
        (ids(3), PRIMARY, zero_timeout),
        (ids(3), PRIMARY, zero_resend),
    ];
    for (members, me, opts) in cases {
        assert_eq!(Vr::new(&configuration(&members), me, opts).unwrap_err().sqlcode(), -910_i32, "{members:?} {me:?} {opts:?}");
    }
    let vr = Vr::new(&config(3), BACKUP, VrOptions::default()).unwrap();
    assert_eq!(vr.name(), "vr");
}

/// Verifies: [DESIGN] RULES.md D57 the database's applied position is kept
/// for the operator; a state installed while the replica is normal in
/// its view, past its log, moves it there: the log let go through the
/// position, its state written, and it stands at the position, committed,
/// delivered and applied, in the view still.
#[test]
fn applied_positions_are_kept_and_an_install_moves_the_replica() {
    let mut vr = Vr::new(&config(3), BACKUP, options()).unwrap();
    let mut out = Vec::new();
    vr.step(Input::Applied(Lsn(4)), &mut out);
    vr.step(Input::Applied(Lsn(2)), &mut out);
    assert_eq!(out, Vec::new());
    assert_eq!(vr.standing().applied, Lsn(4));
    vr.step(Input::Installed { holds: Lsn(9), records: Vec::new() }, &mut out);
    assert_eq!(out.len(), 2_usize, "{out:?}");
    assert_eq!(out[0], Output::Retain { through: Lsn(9) });
    assert!(matches!(out[1], Output::PersistState(_)), "{out:?}");
    let standing = vr.standing();
    assert_eq!((standing.first, standing.op, standing.commit, standing.applied), (Lsn(9), Lsn(9), Lsn(9), Lsn(9)));
    assert_eq!((standing.status, standing.primary), (Status::Normal, PRIMARY));
}

/// Verifies: [DESIGN] RULES.md D57 [VRR §4, Figure 2] a replica's standing
/// gives its view-number, status, primary, op-number, commit-number, and
/// how far its journal and database reached.
#[test]
fn a_replica_stands_where_its_steps_left_it() {
    let mut vr = Vr::new(&configuration(&[PRIMARY]), PRIMARY, options()).unwrap();
    let mut out = Vec::new();
    for input in [Input::Tick, Input::Append(record("a")), Input::Append(record("b")), Input::Persisted(Lsn(1))] {
        vr.step(input, &mut out);
    }
    let standing = Standing {
        epoch: 0,
        view: 0,
        status: Status::Normal,
        primary: PRIMARY,
        first: Lsn(0),
        op: Lsn(2),
        commit: Lsn(1),
        durable: Lsn(1),
        applied: Lsn(0),
    };
    assert_eq!(vr.standing(), standing);
}

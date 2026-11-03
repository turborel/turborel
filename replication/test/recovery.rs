//! Recovery (RULES.md D57, [VRR §4.3]): a replica whose journal holds no
//! state, lost or never written, learning the group's state from the
//! others before it takes part; a new group's members made normal at
//! once, those with empty journals waiting otherwise; and the simulator's
//! replicas losing their journals and converging all the same.

use std::sync::Arc;

use turborel_replication::vr::Status;
use turborel_replication::{Consensus, Input, Lsn, Member, Output, Recovered, ReplicaEvent, ReplicaId, Role, Vr};
use turborel_test_support::replication::sim::plans;
use turborel_test_support::replication::sim::{Plan, simulate, simulate_databases};
use turborel_test_support::replication::{Group, Make, configuration};

use crate::common::{committed, config, ids, make_vr, nonce, options, record, served, serving, settle_without};

const PRIMARY: ReplicaId = ReplicaId(1);
const BACKUP: ReplicaId = ReplicaId(2);
const THIRD: ReplicaId = ReplicaId(3);
/// The tags of RECOVERY and RECOVERYRESPONSE.
const RECOVERY: u8 = 10;
const RECOVERY_RESPONSE: u8 = 11;

/// A VR replica made as a node is, from its journal, empty at first.
fn make_from_journal(members: &[Member], me: ReplicaId, recovered: Option<Recovered>) -> Box<dyn Consensus> {
    Box::new(Vr::restart(members, me, options(), recovered.unwrap_or_default(), nonce()).unwrap())
}

/// How many views `outputs` entered.
fn views_entered(outputs: &[Output]) -> usize {
    outputs.iter().filter(|o| matches!(o, Output::Event(ReplicaEvent::ViewChanged { .. }))).count()
}

/// The frames `outputs` send, by receiver and tag.
fn sent(outputs: &[Output]) -> Vec<(ReplicaId, u8)> {
    outputs.iter().filter_map(|o| if let Output::Send { to, frame } = o { Some((*to, frame[0])) } else { None }).collect()
}

/// A group of three, five records committed and delivered everywhere.
fn five_committed() -> Group {
    let mut group = serving(3);
    for n in 1_u64..=5_u64 {
        group.step(PRIMARY, Input::Append(record(&format!("r{n}"))));
        group.settle();
    }
    group.tick();
    group.settle();
    assert_eq!(group.node(THIRD).applied.len(), 5_usize);
    group
}

/// Verifies: [EXTENSION] RULES.md D57 [VRR §4.3]: a replica made again
/// over a journal lost tells it is recovering, sends RECOVERY to every
/// other member and takes no other message meanwhile, a PREPARE among
/// them; answered by the primary with its log, the record appended
/// meanwhile in it, and by a backup without, it takes the primary's log,
/// its journal written with the records and the state, enters the view,
/// acknowledges, and follows from what its database held, the records
/// committed since delivered; a response of another nonce changes
/// nothing.
#[test]
fn a_replica_losing_its_journal_recovers_from_the_primary() {
    let mut group = five_committed();
    group.lose_journal(THIRD, 3);
    let opened = group.step(THIRD, Input::Opened(Lsn(3)));
    assert!(opened.contains(&Output::Event(ReplicaEvent::Recovering)), "{opened:?}");
    assert_eq!(sent(&opened), vec![(PRIMARY, RECOVERY), (BACKUP, RECOVERY)]);
    // A RECOVERY frame: the tag, the epoch, the nonce, the replica; a
    // RECOVERYRESPONSE: the tag, the epoch, the view, the nonce, where the
    // epoch began, the replica, the configuration, and the log if any.
    let mut stale = group.frames.front().unwrap().bytes.clone();
    stale[9] ^= 1;
    let mut other = vec![RECOVERY_RESPONSE];
    other.extend_from_slice(&[0; 16]);
    other.extend_from_slice(&stale[9..17]);
    other.extend_from_slice(&[0; 8]);
    other.extend_from_slice(&2_u32.to_le_bytes());
    other.extend_from_slice(&[0, 0, 0, 0, 0]);
    assert_eq!(group.step(THIRD, Input::Received { from: BACKUP, frame: other }), [], "another nonce");
    let mut with_log = vec![RECOVERY_RESPONSE];
    with_log.extend_from_slice(&[0; 16]);
    with_log.extend_from_slice(&stale[9..17]);
    with_log.extend_from_slice(&[0; 8]);
    with_log.extend_from_slice(&1_u32.to_le_bytes());
    with_log.extend_from_slice(&[0, 0, 0, 0, 1]);
    for number in [3_u64, 3_u64, 3_u64, 0_u64] {
        with_log.extend_from_slice(&number.to_le_bytes());
    }
    with_log.extend_from_slice(&0_u32.to_le_bytes());
    assert_eq!(group.step(THIRD, Input::Received { from: PRIMARY, frame: with_log }), [], "another nonce, the primary's log");
    group.step(PRIMARY, Input::Append(record("r6")));
    let ignored = group.frames.iter().find(|f| f.to == THIRD && f.bytes[0] == 1_u8).unwrap().clone();
    assert_eq!(group.step(THIRD, Input::Received { from: PRIMARY, frame: ignored.bytes }), [], "a PREPARE while recovering");
    group.settle();
    let third = group.node(THIRD);
    assert!(third.outputs.contains(&Output::Deliver { through: Lsn(5) }), "committed through 5: {:?}", third.outputs);
    assert_eq!(third.journal.records().len(), 6_usize, "the primary's log, the record appended meanwhile in it");
    assert!(third.journal.recovered().state.is_some(), "the state written");
    assert_eq!(views_entered(&third.outputs), 2_usize, "the view entered again: {:?}", third.outputs);
    assert_eq!(third.applied.len(), 5_usize, "delivered from what it held");
    for _ in 0_u32..2_u32 {
        group.tick();
        group.settle();
    }
    assert_eq!(group.node(THIRD).applied.len(), 6_usize, "the record appended meanwhile committed");
    assert_eq!(group.node(THIRD).applied, group.node(BACKUP).applied);
}

/// Verifies: [EXTENSION] RULES.md D57 [VRR §4.3]: a replica recovering
/// whose database holds less than the primary's window begins with
/// fetches a state transfer of the primary's database first, in the
/// primary's view, its RECOVERY sent again at each heartbeat until
/// answered; a member in a view change answers a RECOVERY with the view
/// it was last normal in, and no log, not being that view's primary.
#[test]
fn a_replica_recovering_behind_the_primarys_window_fetches_state() {
    let mut group = five_committed();
    group.step(PRIMARY, Input::Retain(Lsn(4)));
    group.lose_journal(THIRD, 2);
    let entered = views_entered(&group.node(THIRD).outputs);
    group.step(THIRD, Input::Opened(Lsn(2)));
    group.drop_frames(|f| f.from == THIRD);
    group.tick();
    group.tick();
    assert_eq!(sent(&group.flush(THIRD)), [], "sent on the tick, not the flush");
    let again = group.frames.iter().filter(|f| f.from == THIRD && f.bytes[0] == RECOVERY).count();
    assert_eq!(again, 2_usize, "RECOVERY again at the heartbeat");
    group.settle();
    let third = group.node(THIRD);
    assert!(third.outputs.contains(&Output::FetchState { from: PRIMARY }), "{:?}", third.outputs);
    assert_eq!(views_entered(&third.outputs), entered, "in no view yet");

    let mut in_view_1 = vec![1];
    for number in [0_u64, 0_u64, 1_u64, 0_u64, 0_u64] {
        in_view_1.extend_from_slice(&number.to_le_bytes());
    }
    in_view_1.extend_from_slice(&0_u32.to_le_bytes());
    in_view_1.push(0);
    let changing =
        Recovered { state: Some(in_view_1), first: Lsn(0), records: vec![record("r1")], marks: std::collections::BTreeMap::new() };
    let mut changing_backup = Vr::restart(&config(3), BACKUP, options(), changing, 7).unwrap();
    assert_eq!(changing_backup.standing().status, Status::ViewChange);
    let mut recovery = vec![RECOVERY];
    recovery.extend_from_slice(&0_u64.to_le_bytes());
    recovery.extend_from_slice(&9_u64.to_le_bytes());
    recovery.extend_from_slice(&3_u32.to_le_bytes());
    let mut out = Vec::new();
    changing_backup.step(Input::Received { from: THIRD, frame: recovery }, &mut out);
    // The response: the tag, the epoch, view 0, the nonce, where the
    // epoch began, the replica, the configuration of three at no
    // address, and no log.
    let mut response = vec![RECOVERY_RESPONSE];
    response.extend_from_slice(&[0; 16]);
    response.extend_from_slice(&9_u64.to_le_bytes());
    response.extend_from_slice(&[0; 8]);
    response.extend_from_slice(&2_u32.to_le_bytes());
    response.extend_from_slice(&3_u32.to_le_bytes());
    for member in 1_u32..=3_u32 {
        response.extend_from_slice(&member.to_le_bytes());
        response.extend_from_slice(&0_u32.to_le_bytes());
    }
    response.push(0);
    assert_eq!(out, [Output::Send { to: THIRD, frame: response }]);
}

/// Members made from empty journals over empty databases, told nothing of
/// a new group: each sends RECOVERY to the others, none answers, and all
/// wait through five heartbeats, entering no view and writing no state.
fn members_from_empty_journals_wait() {
    let make: Make = Arc::new(make_from_journal);
    let mut waiting = Group::new(&ids(3), make);
    for id in ids(3) {
        assert!(waiting.node(id).outputs.contains(&Output::Event(ReplicaEvent::Recovering)), "{id:?}");
    }
    for _ in 0_u32..5_u32 {
        waiting.tick();
        waiting.settle();
    }
    for id in ids(3) {
        let node = waiting.node(id);
        assert_eq!(views_entered(&node.outputs), 0_usize, "{id:?}: {:?}", node.outputs);
        assert!(!node.outputs.iter().any(|o| matches!(o, Output::Send { frame, .. } if frame[0] == RECOVERY_RESPONSE)), "{id:?} answered");
        assert!(node.journal.recovered().state.is_none(), "{id:?} wrote a state");
    }
}

/// Verifies: [EXTENSION] RULES.md D57: members made from empty journals
/// over empty databases, told nothing of a new group, recover: each sends
/// RECOVERY to the others, none answers, recovering itself, and all wait,
/// through any number of heartbeats, as members as empty as they could
/// be the new members of a group holding a log elsewhere; a new group's
/// members are made normal in view 0 instead (`Vr::new`): the first
/// serves and commits, and a member whose journal is then lost with its
/// database empty recovers from the primary's answer, and holds the log.
/// A group of one recovers at once, from its database alone, its log
/// beginning where the database holds it.
#[test]
fn members_made_from_empty_journals_wait_and_a_new_group_is_made_normal() {
    members_from_empty_journals_wait();
    let made: Make = Arc::new(make_vr);
    let mut group = Group::new(&ids(3), made);
    let entered =
        |of: &Group, id: ReplicaId| of.node(id).outputs.contains(&Output::Event(ReplicaEvent::ViewChanged { view: 0, primary: PRIMARY }));
    assert!(entered(&group, PRIMARY) && entered(&group, BACKUP) && entered(&group, THIRD));
    group.settle();
    assert_eq!(served(&group.node(PRIMARY).outputs), vec![0]);
    group.step(PRIMARY, Input::Append(record("a")));
    settle_without(&mut group, &[THIRD]);
    assert_eq!(committed(&group.node(PRIMARY).outputs), vec![1]);
    group.lose_journal(THIRD, 0);
    group.step(THIRD, Input::Opened(Lsn(0)));
    group.tick();
    group.tick();
    group.settle();
    let third = group.node(THIRD);
    assert_eq!(third.journal.records(), [record("a")]);
    assert_eq!(third.applied, [record("a")]);
    assert!(third.outputs.contains(&Output::Event(ReplicaEvent::Recovered { view: 0, from: PRIMARY })), "{:?}", third.outputs);

    let mut alone = Vr::restart(&configuration(&[PRIMARY]), PRIMARY, options(), Recovered::default(), 3).unwrap();
    let mut out = Vec::new();
    alone.step(Input::Opened(Lsn(7)), &mut out);
    assert!(out.contains(&Output::Retain { through: Lsn(7) }), "{out:?}");
    assert!(out.contains(&Output::Role(Role::Primary { view: 0 })), "{out:?}");
    assert_eq!((alone.standing().status, alone.standing().first, alone.standing().commit), (Status::Normal, Lsn(7), Lsn(7)));
}

/// Verifies: [EXTENSION] RULES.md D57 [VRR §4.3]: a replica recovering
/// takes the log of the primary of the latest view the responses name,
/// from that replica's response of that view: an earlier response of the
/// same replica, primary of an earlier view, is no log of the latest, so
/// the recovery waits for the primary's answer to its next RECOVERY.
#[test]
fn a_recovery_takes_no_stale_answer_of_the_latest_views_primary() {
    let mut group = five_committed();
    group.lose_journal(THIRD, 3);
    group.step(THIRD, Input::Opened(Lsn(3)));
    let recovery = group.frames.iter().find(|f| f.from == THIRD && f.to == PRIMARY).expect("the RECOVERY").clone();
    group.drop_frames(|f| f.from == THIRD);
    let answer = |of: &mut Group, from: ReplicaId| -> Vec<u8> {
        let out = of.step(from, Input::Received { from: THIRD, frame: recovery.bytes.clone() });
        out.iter()
            .find_map(|o| if let Output::Send { to, frame } = o { (*to == THIRD).then(|| frame.clone()) } else { None })
            .expect("the answer")
    };
    let stale = answer(&mut group, PRIMARY);
    assert_eq!(stale[9..17], 0_u64.to_le_bytes(), "of view 0");
    let mut later = answer(&mut group, BACKUP);
    // The view: after the tag and the epoch.
    later[9..17].copy_from_slice(&3_u64.to_le_bytes());
    assert_eq!(group.step(THIRD, Input::Received { from: PRIMARY, frame: stale.clone() }), [], "one answer");
    let waited = group.step(THIRD, Input::Received { from: BACKUP, frame: later });
    assert!(
        !waited.iter().any(|o| matches!(o, Output::Event(ReplicaEvent::Recovered { .. }))),
        "the primary's answer is of view 0: {waited:?}"
    );
    assert!(waited.is_empty(), "{waited:?}");
    let mut of_view_3 = stale;
    of_view_3[9..17].copy_from_slice(&3_u64.to_le_bytes());
    let recovered = group.step(THIRD, Input::Received { from: PRIMARY, frame: of_view_3 });
    assert!(recovered.contains(&Output::Event(ReplicaEvent::Recovered { view: 3, from: PRIMARY })), "{recovered:?}");
}

/// The plan losing journals, keeping `retain_bytes` of the log.
fn losing_journals(retain_bytes: u64) -> Plan {
    Plan { retain_bytes, ..plans::losing() }
}

/// Verifies: [EXTENSION] RULES.md D57 [VRR §4.3]: replicas crashing and
/// losing their journals, one at a time, recover from the group and
/// converge all the same, holding what the primary holds, with the log
/// kept whole and with little of it kept, state transfers then among the
/// recoveries: modelled databases, and real ones kept in files; every
/// run begins by the group bootstrapping.
#[test]
fn replicas_losing_their_journals_recover_and_converge() {
    let make: Make = Arc::new(make_vr);
    let mut lost = 0_u64;
    let mut failures = Vec::new();
    for (seed, retain_bytes) in (0..10_u64).map(|s| (s, u64::MAX)).chain((10..20_u64).map(|s| (s, 200_u64))) {
        match simulate(seed, &losing_journals(retain_bytes), &make) {
            Ok(report) => lost += report.journals_lost,
            Err(why) => failures.push(why),
        }
    }
    assert!(failures.is_empty(), "{} seeds failed:\n{}", failures.len(), failures.join("\n"));
    assert!(lost > 0_u64, "no journal lost in 20 seeds");
    let mut with_files = 0_u64;
    for seed in 0..4_u64 {
        let report = simulate_databases(seed, &losing_journals(200), &make).unwrap();
        with_files += report.replication.journals_lost;
        assert_eq!(report.compared, 2_usize);
    }
    assert!(with_files > 0_u64, "no journal lost in 4 seeds of real databases");
}

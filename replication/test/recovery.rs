//! Recovery (RULES.md D57, [VRR §4.3]): a replica whose journal holds no
//! state, lost or never written, learning the group's state from the
//! others before it takes part; a new group bootstrapping by it; and the
//! simulator's replicas losing their journals and converging all the
//! same.

use std::sync::Arc;

use turborel_replication::vr::Status;
use turborel_replication::{Consensus, Input, Lsn, Output, Recovered, ReplicaEvent, ReplicaId, Role, Vr};
use turborel_test_support::replication::sim::plans;
use turborel_test_support::replication::sim::{Plan, simulate, simulate_databases};
use turborel_test_support::replication::{Group, Make};

use crate::common::{committed, ids, make_vr, nonce, options, record, served, serving, settle_without};

const PRIMARY: ReplicaId = ReplicaId(1);
const BACKUP: ReplicaId = ReplicaId(2);
const THIRD: ReplicaId = ReplicaId(3);
/// The tags of RECOVERY and RECOVERYRESPONSE.
const RECOVERY: u8 = 10;
const RECOVERY_RESPONSE: u8 = 11;

/// A VR replica made as a node is, from its journal, empty at first.
fn make_from_journal(members: &[ReplicaId], me: ReplicaId, recovered: Option<Recovered>) -> Box<dyn Consensus> {
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
    let mut stale = group.frames.front().unwrap().bytes.clone();
    stale[1] ^= 1;
    let mut other = vec![RECOVERY_RESPONSE];
    other.extend_from_slice(&0_u64.to_le_bytes());
    other.extend_from_slice(&stale[1..9]);
    other.extend_from_slice(&2_u32.to_le_bytes());
    other.extend_from_slice(&[0, 0]);
    assert_eq!(group.step(THIRD, Input::Received { from: BACKUP, frame: other }), [], "another nonce");
    let mut with_log = vec![RECOVERY_RESPONSE];
    with_log.extend_from_slice(&0_u64.to_le_bytes());
    with_log.extend_from_slice(&stale[1..9]);
    with_log.extend_from_slice(&1_u32.to_le_bytes());
    with_log.extend_from_slice(&[0, 1]);
    for number in [3_u64, 3_u64, 3_u64, 0_u64] {
        with_log.extend_from_slice(&number.to_le_bytes());
    }
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
/// answered; a member in a view change answers no RECOVERY.
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

    let changing = Recovered { state: Some(vec![1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]), first: Lsn(0), records: Vec::new() };
    let mut silent = Vr::restart(&ids(3), BACKUP, options(), changing, 7).unwrap();
    assert_eq!(silent.standing().status, Status::ViewChange);
    let mut recovery = vec![RECOVERY];
    recovery.extend_from_slice(&9_u64.to_le_bytes());
    recovery.extend_from_slice(&3_u32.to_le_bytes());
    let mut out = Vec::new();
    silent.step(Input::Received { from: THIRD, frame: recovery }, &mut out);
    assert_eq!(out, []);
}

/// Verifies: [EXTENSION] RULES.md D57: a new group's members, each made
/// from its empty journal and opened over an empty database, recover from
/// each other: fresh, each answers the others' RECOVERY saying so, and
/// hearing from f fresh members is normal in view 0; two of three suffice,
/// the first serving and committing; the third, heard later, recovers
/// from the primary's answer and holds the log. A group of one recovers
/// at once, from its database alone, its log beginning where the
/// database holds it.
#[test]
fn a_new_group_bootstraps_by_recovery() {
    let make: Make = Arc::new(make_from_journal);
    let mut group = Group::new(&ids(3), make);
    let entered =
        |of: &Group, id: ReplicaId| of.node(id).outputs.contains(&Output::Event(ReplicaEvent::ViewChanged { view: 0, primary: PRIMARY }));
    for id in ids(3) {
        assert!(group.node(id).outputs.contains(&Output::Event(ReplicaEvent::Recovering)), "{id:?}");
        assert!(!entered(&group, id), "{id:?}");
    }
    settle_without(&mut group, &[THIRD]);
    assert!(entered(&group, PRIMARY) && entered(&group, BACKUP) && !entered(&group, THIRD));
    assert_eq!(served(&group.node(PRIMARY).outputs), vec![0]);
    group.step(PRIMARY, Input::Append(record("a")));
    settle_without(&mut group, &[THIRD]);
    assert_eq!(committed(&group.node(PRIMARY).outputs), vec![1]);
    group.tick();
    group.tick();
    group.settle();
    assert!(entered(&group, THIRD));
    let third = group.node(THIRD);
    assert_eq!(third.journal.records(), [record("a")]);
    assert_eq!(third.applied, [record("a")]);

    let mut alone = Vr::restart(&[PRIMARY], PRIMARY, options(), Recovered::default(), 3).unwrap();
    let mut out = Vec::new();
    alone.step(Input::Opened(Lsn(7)), &mut out);
    assert!(out.contains(&Output::Retain { through: Lsn(7) }), "{out:?}");
    assert!(out.contains(&Output::Role(Role::Primary { view: 0 })), "{out:?}");
    assert_eq!((alone.standing().status, alone.standing().first, alone.standing().commit), (Status::Normal, Lsn(7), Lsn(7)));
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

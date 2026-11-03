//! Reconfiguration (RULES.md D57, [VRR §7]): the group's members changed
//! while it runs, in epochs: a member added, recovering into the new
//! epoch; a member removed, leaving until the new members are normal,
//! then retired; a replica behind an epoch brought into it; what a
//! reconfiguration refuses; the marks a journal keeps; and the simulator
//! reconfiguring groups through faults, spares joining as members leave.

use std::sync::Arc;

use turborel_replication::vr::Status;
use turborel_replication::{Configuration, Consensus, Input, Lsn, Mark, Member, Output, Recovered, ReplicaEvent, ReplicaId, Role, Vr};
use turborel_test_support::replication::sim::{plans, simulate, simulate_databases};
use turborel_test_support::replication::{Frame, Group, Make, configuration};

use crate::common::{committed, config, make_vr, options, record, served, serving, settle_without};

const ONE: ReplicaId = ReplicaId(1);
const TWO: ReplicaId = ReplicaId(2);
const THREE: ReplicaId = ReplicaId(3);
const FOUR: ReplicaId = ReplicaId(4);

/// The members `ids`, each at an address of its own.
fn members(ids: &[u32]) -> Configuration {
    ids.iter().map(|&id| Member { id: ReplicaId(id), address: format!("db{id}:7423") }).collect()
}

/// The mark beginning `epoch` with the members `ids`.
fn mark(epoch: u64, ids: &[u32]) -> Mark {
    Mark { epoch, configuration: members(ids) }
}

/// The epochs `outputs` told entered, each with its members' numbers.
fn epochs(outputs: &[Output]) -> Vec<(u64, Vec<u32>)> {
    outputs
        .iter()
        .filter_map(|o| {
            if let Output::Event(ReplicaEvent::Reconfigured { epoch, configuration }) = o {
                Some((*epoch, configuration.iter().map(|m| m.id.0).collect()))
            } else {
                None
            }
        })
        .collect()
}

/// A group of three serving, two records committed everywhere.
fn with_two_records() -> Group {
    let mut group = serving(3);
    for name in ["a", "b"] {
        group.step(ONE, Input::Append(record(name)));
        group.settle();
    }
    group.tick();
    group.settle();
    group
}

/// A group of three with two records, reconfigured to four members, the
/// three in epoch 1: the primary, asked by its database to reconfigure,
/// appends the record marked with the next epoch's members, its journal
/// marking it, takes no record after, and serves on until the mark
/// commits; then every member enters epoch 1, view 0, telling it, with
/// the mark kept, and the first serves again.
fn reconfigured_to_four() -> Group {
    let mut group = with_two_records();
    let asked = group.step(ONE, Input::Reconfigure { record: record("r"), configuration: members(&[1, 2, 3, 4]) });
    assert!(asked.contains(&Output::Mark { at: Lsn(3), mark: mark(1, &[1, 2, 3, 4]) }), "{asked:?}");
    assert!(!asked.contains(&Output::Role(Role::Backup)), "serving on until the mark commits: {asked:?}");
    assert_eq!(group.node(ONE).journal.recovered().marks.get(&Lsn(3)), Some(&mark(1, &[1, 2, 3, 4])));
    let refused = group.step(ONE, Input::Append(record("late")));
    assert_eq!(refused, [Output::Event(ReplicaEvent::AppendRefused)], "no record after the reconfiguration");
    group.settle();
    for _ in 0_u32..3_u32 {
        group.tick();
        group.settle();
    }
    for id in [ONE, TWO, THREE] {
        let outputs = &group.node(id).outputs;
        assert_eq!(epochs(outputs), vec![(1, vec![1, 2, 3, 4])], "{id:?}: {outputs:?}");
        assert!(outputs.contains(&Output::Event(ReplicaEvent::ViewChanged { view: 0, primary: ONE })), "{id:?}");
        assert_eq!(group.node(id).journal.recovered().marks.get(&Lsn(3)), Some(&mark(1, &[1, 2, 3, 4])), "{id:?}: the mark kept");
    }
    assert_eq!(served(&group.node(ONE).outputs), vec![0, 0], "view 0 of each epoch");
    assert_eq!(group.node(TWO).applied, [record("a"), record("b"), record("r")]);
    group
}

/// Verifies: [EXTENSION] RULES.md D57 [VRR §7]: the primary, asked by its
/// database to reconfigure, appends the record marked with the next
/// epoch's members, its journal marking it, and takes no record after,
/// serving on until the mark commits, told to its database; then every
/// member enters epoch 1,
/// view 0, telling it, the first member of the new configuration serving
/// again once the log it took over is delivered; a fourth replica, new,
/// recovers into the epoch from the members, adopting its configuration,
/// and holds every record; records appended in the epoch commit with a
/// quorum of four and reach it.
#[test]
fn a_member_added_joins_the_next_epoch() {
    let mut group = reconfigured_to_four();
    group.add(FOUR);
    for _ in 0_u32..3_u32 {
        group.tick();
        group.settle();
    }
    let four = group.node(FOUR);
    assert_eq!(epochs(&four.outputs), vec![(1, vec![1, 2, 3, 4])], "{:?}", four.outputs);
    assert_eq!(four.applied, [record("a"), record("b"), record("r")]);
    assert_eq!(four.journal.records().len(), 3_usize);

    group.step(ONE, Input::Append(record("c")));
    group.settle();
    assert_eq!(committed(&group.node(ONE).outputs), vec![1, 2, 3, 4]);
    for _ in 0_u32..2_u32 {
        group.tick();
        group.settle();
    }
    assert_eq!(group.node(FOUR).applied, [record("a"), record("b"), record("r"), record("c")]);
    assert_eq!(group.node(THREE).applied, [record("a"), record("b"), record("r"), record("c")]);
}

/// Verifies: [EXTENSION] RULES.md D57 [VRR §7]: a member removed enters
/// the epoch leaving: it answers the new members, and once a quorum of
/// them are normal in the epoch, told by EPOCHSTARTED, not one, it is
/// removed,
/// its database retired, taking no frame after; the new epoch commits
/// with its own quorum.
#[test]
fn a_member_removed_leaves_and_is_retired() {
    let mut group = with_two_records();
    group.step(ONE, Input::Reconfigure { record: record("r"), configuration: members(&[1, 2]) });
    // The second member cut off: the first alone is normal in the epoch,
    // no quorum of the new members, so the third leaves and waits.
    settle_without(&mut group, &[TWO]);
    for _ in 0_u32..3_u32 {
        group.tick();
        settle_without(&mut group, &[TWO]);
    }
    assert_eq!(epochs(&group.node(ONE).outputs), vec![(1, vec![1, 2])], "{:?}", group.node(ONE).outputs);
    let leaving = group.node(THREE);
    assert!(!leaving.outputs.contains(&Output::Retire), "one member normal in the epoch is no quorum: {:?}", leaving.outputs);
    group.settle();
    for _ in 0_u32..3_u32 {
        group.tick();
        group.settle();
    }
    let three = group.node(THREE);
    assert_eq!(epochs(&three.outputs), vec![(1, vec![1, 2])], "{:?}", three.outputs);
    assert!(three.outputs.contains(&Output::Retire), "{:?}", three.outputs);
    assert!(three.outputs.contains(&Output::Event(ReplicaEvent::Removed)), "{:?}", three.outputs);
    assert!(
        !three
            .outputs
            .iter()
            .any(|o| matches!(o, Output::Event(ReplicaEvent::ViewChanged { view: 0, .. }) if epochs(&three.outputs).len() == 2)),
        "no view of the epoch entered"
    );
    assert_eq!(served(&group.node(ONE).outputs), vec![0, 0]);
    group.step(ONE, Input::Append(record("c")));
    group.settle();
    assert_eq!(committed(&group.node(ONE).outputs), vec![1, 2, 3, 4], "a quorum of two");
    let before = group.node(THREE).outputs.len();
    group.step(THREE, Input::Tick);
    group.step(THREE, Input::Tick);
    assert_eq!(group.node(THREE).outputs.len(), before, "removed, it does nothing");
}

/// Verifies: [EXTENSION] RULES.md D57 [VRR §7]: a member removed that
/// missed everything, behind the epoch, is answered EPOCHSTARTED as it
/// gives its view up, asks for the state, enters the epoch leaving, and
/// is removed all the same.
#[test]
fn a_member_removed_that_missed_the_epoch_learns_it_and_is_removed() {
    // One leaving that missed EPOCHSTARTED asks, by a GETSTATE at its
    // heartbeat, and is answered EPOCHSTARTED.
    let mut again = with_two_records();
    again.step(ONE, Input::Reconfigure { record: record("r"), configuration: members(&[1, 2]) });
    settle_without(&mut again, &[THREE]);
    for _ in 0_u32..3_u32 {
        again.tick();
        settle_without(&mut again, &[THREE]);
    }
    assert_eq!(epochs(&again.node(ONE).outputs), vec![(1, vec![1, 2])]);
    assert_eq!(epochs(&again.node(THREE).outputs), vec![]);
    // Its primary silent, it gives the view up after the view timeout, and
    // its STARTVIEWCHANGE is answered EPOCHSTARTED, by which it learns.
    for _ in 0_u32..12_u32 {
        again.tick();
        again.settle();
    }
    let late = again.node(THREE);
    assert_eq!(epochs(&late.outputs), vec![(1, vec![1, 2])], "{:?}", late.outputs);
    assert!(late.outputs.contains(&Output::Retire), "{:?}", late.outputs);
}

/// Verifies: [EXTENSION] RULES.md D57 [VRR §7]: an epoch is entered only
/// as its mark commits: the primary and a backup holding the mark, the
/// record before it committed and the mark acknowledged by none, enter
/// no epoch, through any number of heartbeats; the acknowledgements let
/// through, every member enters it.
#[test]
fn an_epoch_is_entered_only_as_its_mark_commits() {
    let mut group = with_two_records();
    group.step(ONE, Input::Append(record("c")));
    group.step(ONE, Input::Reconfigure { record: record("r"), configuration: members(&[1, 2, 3]) });
    // A PREPAREOK: the tag, the epoch, the view, the op-number, the
    // replica: those of the mark, at 4, dropped.
    let of_mark = |f: &Frame| f.bytes[0] == 2_u8 && f.bytes[17..25] == 4_u64.to_le_bytes();
    for _ in 0_u32..4_u32 {
        loop {
            group.drop_frames(of_mark);
            if !group.deliver() {
                break;
            }
        }
        group.tick();
    }
    assert_eq!(committed(&group.node(ONE).outputs), vec![1, 2, 3], "the record before the mark committed");
    for id in [ONE, TWO, THREE] {
        assert_eq!(epochs(&group.node(id).outputs), vec![], "{id:?}: the mark not committed: {:?}", group.node(id).outputs);
    }
    group.settle();
    for _ in 0_u32..3_u32 {
        group.tick();
        group.settle();
    }
    for id in [ONE, TWO, THREE] {
        assert_eq!(epochs(&group.node(id).outputs), vec![(1, vec![1, 2, 3])], "{id:?}");
    }
}

/// Verifies: [EXTENSION] RULES.md D57 [VRR §7]: a replica behind its
/// epoch, its commit-number before the mark the epoch began at, takes
/// part in no view change of the epoch: a STARTVIEWCHANGE of a later
/// view has it ask for the state of that view's primary, not start the
/// view change nor send a DOVIEWCHANGE.
#[test]
fn a_replica_behind_its_epoch_votes_in_no_view_change() {
    // The state: its format, epoch 1, begun at 10, view 0, normal view 0,
    // commit-number 5, no configuration, not removed.
    let mut state = vec![1_u8];
    for number in [1_u64, 10_u64, 0_u64, 0_u64, 5_u64] {
        state.extend_from_slice(&number.to_le_bytes());
    }
    state.extend_from_slice(&0_u32.to_le_bytes());
    state.push(0);
    let records = (1_u64..=8_u64).map(|n| record(&format!("r{n}"))).collect();
    let recovered = Recovered { state: Some(state), first: Lsn(0), records, marks: std::collections::BTreeMap::new() };
    let mut behind = Vr::restart(&config(3), TWO, options(), recovered, 1).unwrap();
    assert_eq!(behind.standing().status, Status::Transfer, "behind the mark, in a transfer");
    // A STARTVIEWCHANGE for view 1 of epoch 1 from the third member.
    let mut start = vec![4_u8];
    for number in [1_u64, 1_u64] {
        start.extend_from_slice(&number.to_le_bytes());
    }
    start.extend_from_slice(&3_u32.to_le_bytes());
    let mut out = Vec::new();
    behind.step(Input::Received { from: THREE, frame: start }, &mut out);
    let sent: Vec<u8> = out.iter().filter_map(|o| if let Output::Send { frame, .. } = o { Some(frame[0]) } else { None }).collect();
    assert!(!sent.contains(&4_u8) && !sent.contains(&5_u8), "no STARTVIEWCHANGE nor DOVIEWCHANGE: {out:?}");
    assert!(sent.contains(&7_u8), "the state asked for: {out:?}");
    assert_eq!(behind.standing().view, 1, "the later view learned");
}

/// Verifies: [EXTENSION] RULES.md D57 [VRR §7]: a replica that missed the
/// reconfiguration, told by a message of the later epoch, fetches the log
/// from the sender, whose NEWSTATE carries the epoch's configuration, and
/// enters the epoch as it learns the mark committed, holding every
/// record; a message of an earlier epoch tells it nothing after.
#[test]
fn a_replica_behind_an_epoch_is_brought_into_it() {
    let mut group = with_two_records();
    group.step(ONE, Input::Reconfigure { record: record("r"), configuration: members(&[1, 2, 3, 4]) });
    settle_without(&mut group, &[THREE]);
    for _ in 0_u32..3_u32 {
        group.tick();
        settle_without(&mut group, &[THREE]);
    }
    assert_eq!(epochs(&group.node(ONE).outputs), vec![(1, vec![1, 2, 3, 4])]);
    assert_eq!(epochs(&group.node(THREE).outputs), vec![]);
    let old: Vec<Vec<u8>> = group.frames.iter().filter(|f| f.to == THREE).map(|f| f.bytes.clone()).collect();
    group.drop_frames(|f| f.to == THREE || f.from == THREE);
    group.add(FOUR);
    for _ in 0_u32..3_u32 {
        group.tick();
        group.settle();
    }
    group.step(ONE, Input::Append(record("c")));
    for _ in 0_u32..4_u32 {
        group.tick();
        group.settle();
    }
    let three = group.node(THREE);
    assert_eq!(epochs(&three.outputs), vec![(1, vec![1, 2, 3, 4])], "{:?}", three.outputs);
    assert_eq!(three.applied, [record("a"), record("b"), record("r"), record("c")]);
    for frame in old {
        assert_eq!(group.step(THREE, Input::Received { from: ONE, frame }), [], "an earlier epoch's");
    }
}

/// Verifies: [EXTENSION] RULES.md D57 [VRR §7]: the mark of a
/// reconfiguration is written before the record it marks, by the primary
/// and by a backup taking the PREPARE, so a crash keeping a prefix of the
/// writes never keeps the record without its mark; a mark kept without
/// its record is void, and the backup, made again with neither, takes
/// the PREPARE again and enters the epoch with the others.
#[test]
fn a_mark_is_written_before_its_record_and_is_void_without_it() {
    let mut group = with_two_records();
    group.instant_disks = false;
    let asked = group.step(ONE, Input::Reconfigure { record: record("r"), configuration: members(&[1, 2, 3]) });
    let order = |outputs: &[Output]| {
        let marked = outputs.iter().position(|o| matches!(o, Output::Mark { at, .. } if *at == Lsn(3)));
        let written = outputs.iter().position(|o| matches!(o, Output::Persist { at, .. } if *at == Lsn(3)));
        (marked, written)
    };
    let (marked, written) = order(&asked);
    assert!(marked.is_some_and(|m| written.is_some_and(|w| m < w)), "the primary: {asked:?}");
    let prepare = group.frames.iter().find(|f| f.from == ONE && f.to == TWO).expect("the PREPARE").clone();
    let took = group.step(TWO, Input::Received { from: ONE, frame: prepare.bytes });
    let (backup_marked, backup_written) = order(&took);
    assert!(backup_marked.is_some_and(|m| backup_written.is_some_and(|w| m < w)), "the backup: {took:?}");
    assert_eq!(group.node(TWO).journal.waiting(), 2_usize, "the mark and the record wait");
    group.crash(TWO, 1);
    let recovered = group.node(TWO).journal.recovered();
    assert_eq!(recovered.records.len(), 2_usize, "the record lost");
    assert!(recovered.marks.is_empty(), "the mark, kept, is void: {:?}", recovered.marks);
    group.instant_disks = true;
    group.step(TWO, Input::Opened(Lsn(0)));
    group.settle();
    for _ in 0_u32..3_u32 {
        group.tick();
        group.settle();
    }
    for id in [ONE, TWO, THREE] {
        assert_eq!(epochs(&group.node(id).outputs), vec![(1, vec![1, 2, 3])], "{id:?}");
        assert_eq!(group.node(id).journal.recovered().marks.get(&Lsn(3)), Some(&mark(1, &[1, 2, 3])), "{id:?}");
    }
}

/// Verifies: [EXTENSION] RULES.md D57: a reconfiguration asked of a backup,
/// of a primary reconfiguring already, or to no member at all, is refused
/// as an append is; the state written names the epoch and its members and
/// the commit-number, and a replica made again from it stands in the
/// epoch, knowing the log committed through it, delivering it as it
/// opens, its marks kept until the log is let go through them.
#[test]
fn reconfigurations_out_of_place_are_refused_and_marks_survive_restarts() {
    let mut group = with_two_records();
    let refused = group.step(TWO, Input::Reconfigure { record: record("r"), configuration: members(&[1, 2]) });
    assert_eq!(refused, [Output::Event(ReplicaEvent::AppendRefused)], "a backup");
    let empty = group.step(ONE, Input::Reconfigure { record: record("r"), configuration: Vec::new() });
    assert_eq!(empty, [Output::Event(ReplicaEvent::AppendRefused)], "no member");
    group.step(ONE, Input::Reconfigure { record: record("r"), configuration: members(&[1, 2, 3]) });
    let twice = group.step(ONE, Input::Reconfigure { record: record("s"), configuration: members(&[1, 2]) });
    assert_eq!(twice, [Output::Event(ReplicaEvent::AppendRefused)], "reconfiguring already");
    group.settle();
    group.crash(TWO, usize::MAX);
    let recovered = group.node(TWO).journal.recovered();
    assert_eq!(recovered.marks.get(&Lsn(3)), Some(&mark(1, &[1, 2, 3])), "the mark durable: {recovered:?}");
    group.settle();
    for _ in 0_u32..3_u32 {
        group.tick();
        group.settle();
    }
    assert_eq!(epochs(&group.node(ONE).outputs), vec![(1, vec![1, 2, 3])]);
    group.crash(ONE, usize::MAX);
    let opened = group.step(ONE, Input::Opened(Lsn(0)));
    assert!(opened.contains(&Output::Deliver { through: Lsn(3) }), "the commit-number known from the state: {opened:?}");
    let one = group.node(ONE);
    let state = one.journal.recovered().state.expect("the state");
    assert_eq!(state[1..9], 1_u64.to_le_bytes(), "the epoch");
    assert!(epochs(&one.outputs).last().is_some_and(|(epoch, _)| *epoch == 1), "made again in the epoch: {:?}", one.outputs);
    group.step(THREE, Input::Retain(Lsn(3)));
    assert_eq!(group.node(THREE).journal.recovered().marks.len(), 0_usize, "let go with the record");
}

/// Verifies: [EXTENSION] RULES.md D57: `Vr::restart` from a state naming
/// a configuration this replica is no member of stands leaving, and asks
/// whether the epoch started; from one saying it was removed, it stands
/// removed, its database retired again, and asks nothing.
#[test]
fn a_replica_restarted_out_of_its_epochs_configuration_is_leaving() {
    let make: Make = Arc::new(make_vr);
    let _ = (make, config(3), configuration(&[ONE]), options());
    let mut group = with_two_records();
    group.step(ONE, Input::Reconfigure { record: record("r"), configuration: members(&[1, 2]) });
    group.settle();
    assert!(group.node(THREE).journal.recovered().state.is_some());
    group.crash(THREE, usize::MAX);
    let before = group.node(THREE).outputs.len();
    let _ = Recovered::default();
    group.step(THREE, Input::Tick);
    group.step(THREE, Input::Tick);
    let sent: Vec<&Output> = group.node(THREE).outputs[before..].iter().filter(|o| matches!(o, Output::Send { .. })).collect();
    assert!(!sent.is_empty(), "leaving, it asks whether the epoch started: {:?}", &group.node(THREE).outputs[before..]);
    group.settle();
    for _ in 0_u32..3_u32 {
        group.tick();
        group.settle();
    }
    assert!(group.node(THREE).outputs.contains(&Output::Event(ReplicaEvent::Removed)), "{:?}", group.node(THREE).outputs);
    group.crash(THREE, usize::MAX);
    let made_again = group.node(THREE).outputs.len();
    group.step(THREE, Input::Opened(Lsn(3)));
    group.step(THREE, Input::Tick);
    group.step(THREE, Input::Tick);
    let again = &group.node(THREE).outputs[made_again..];
    assert!(again.contains(&Output::Retire) && again.contains(&Output::Event(ReplicaEvent::Removed)), "removed again: {again:?}");
    assert!(!again.iter().any(|o| matches!(o, Output::Send { .. })), "removed, it asks nothing: {again:?}");
}

/// Verifies: [DESIGN] RULES.md D57 [VRR §7] [VRR §8.1] groups reconfigured
/// through faults at random, a spare joining as a member leaves at each,
/// the group of three moving over seven nodes, with journals lost and
/// little of the log kept: committed records survive into every epoch and
/// view; at most one primary serves each view of each epoch, the one its
/// members name; every epoch is told of the same members by all; new
/// members recover from the group, fetching its state when behind what
/// it keeps; and once the faults stop, every member's database converges
/// on the committed log, and every replica that left is removed, its
/// database retired: modelled databases, and real ones, whose retired
/// databases refuse even a read.
#[test]
fn groups_reconfigure_through_faults_and_converge() {
    let make: Make = Arc::new(make_vr);
    let (mut epochs, mut removed, mut lost, mut transfers) = (0_u64, 0_u64, 0_u64, 0_u64);
    let mut failures = Vec::new();
    for seed in 0..60_u64 {
        match simulate(seed, &plans::reconfiguring(), &make) {
            Ok(report) => {
                epochs += report.epochs;
                removed += report.removed;
                lost += report.journals_lost;
                transfers += report.transfers;
            }
            Err(why) => failures.push(why),
        }
    }
    assert!(failures.is_empty(), "{} seeds failed:\n{}", failures.len(), failures.join("\n"));
    assert!(epochs > 30_u64 && removed > 30_u64, "reconfigurations hardly met: {epochs} epochs, {removed} removed");
    assert!(lost > 0_u64 && transfers > 0_u64, "journals lost and transfers hardly met: {lost} {transfers}");
    let mut with_files = 0_u64;
    for seed in 0..6_u64 {
        let report = simulate_databases(seed, &plans::reconfiguring(), &make).unwrap();
        with_files += report.reconfigured;
        assert!(report.compared >= 2_usize, "{report:?}");
    }
    assert!(with_files > 0_u64, "no reconfiguration done in 6 seeds of real databases");
}

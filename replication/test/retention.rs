//! Log retention (RULES.md D57): a replica's log as a window, the records
//! its database holds in its files let go, in VR, in the journal and in
//! the driver; a replica behind what another retains asking for a state
//! transfer, and installing one.

use std::sync::Arc;

use turborel::disk::io::Durability;
use turborel::log::{FileLog, FileLogOptions, Log, Lsn, MemoryLog};
use turborel_replication::vr::Status;
use turborel_replication::{Consensus, Input, Journal, LogJournal, Output, Recovered, ReplicaId, Role, Vr};
use turborel_test_support::Scratch;
use turborel_test_support::database::{crc32c, os};

use crate::common::{config, ids, options, record, serving};

const PRIMARY: ReplicaId = ReplicaId(1);
const BACKUP: ReplicaId = ReplicaId(2);

/// Verifies: [EXTENSION] RULES.md D57 (log retention): a backup told
/// `Retain` lets the records through the position go, as far as they are
/// committed, its journal told, and no further than its window begins;
/// its standing names the window; a GETSTATE for a position before the
/// window is answered NEEDSTATE, by which the asker has its driver fetch a
/// state transfer, once for the GETSTATE it answers, serving no more
/// meanwhile; the transfer installed at a
/// position, its journal begins after it, its state is written, and it
/// fetches the log after.
#[test]
fn a_replica_lets_committed_records_go_and_fetches_state_when_behind() {
    let mut group = serving(3);
    for n in 1_u64..=5_u64 {
        group.step(PRIMARY, Input::Append(record(&format!("r{n}"))));
        group.settle();
    }
    let retained = group.step(BACKUP, Input::Retain(Lsn(3)));
    assert!(retained.contains(&Output::Retain { through: Lsn(3) }), "{retained:?}");
    assert_eq!(group.node(BACKUP).journal.first(), Lsn(3));
    assert_eq!(group.step(BACKUP, Input::Retain(Lsn(2))), [], "no further back");
    let beyond = group.step(BACKUP, Input::Retain(Lsn(99)));
    assert_eq!(beyond, [Output::Retain { through: Lsn(4) }], "as far as it knows committed: {beyond:?}");
    group.tick();
    group.settle();
    assert_eq!(group.step(BACKUP, Input::Retain(Lsn(99))), [Output::Retain { through: Lsn(5) }], "told the commit");
    // The primary retains too: a backup asking for the log before its
    // window is told it needs the state.
    group.step(PRIMARY, Input::Retain(Lsn(4)));
    let mut third = Vr::new(&config(3), ReplicaId(3), options()).unwrap();
    let get = {
        let mut out = Vec::new();
        third.step(Input::Received { from: PRIMARY, frame: commit_frame(5) }, &mut out);
        out.into_iter().find_map(|o| if let Output::Send { to, frame } = o { (to == PRIMARY).then_some(frame) } else { None })
    };
    let answered = group.step(PRIMARY, Input::Received { from: ReplicaId(3), frame: get.unwrap() });
    let need = answered
        .iter()
        .find_map(|o| if let Output::Send { to, frame } = o { (*to == ReplicaId(3)).then_some(frame.clone()) } else { None });
    let need = need.expect("NEEDSTATE");
    assert_eq!(need[0], 9_u8, "the NEEDSTATE tag");
    let mut out = Vec::new();
    third.step(Input::Received { from: PRIMARY, frame: need.clone() }, &mut out);
    assert!(out.contains(&Output::FetchState { from: PRIMARY }), "{out:?}");
    assert_eq!(third.standing().status, Status::Transfer);
    // The same NEEDSTATE again, the GETSTATE it answers no longer waited
    // on: it moves the replica no more.
    let mut again = Vec::new();
    third.step(Input::Received { from: PRIMARY, frame: need }, &mut again);
    assert!(!again.contains(&Output::FetchState { from: PRIMARY }), "a NEEDSTATE answering no GETSTATE waited on: {again:?}");
    let mut installed = Vec::new();
    third.step(Input::Installed { holds: Lsn(4), records: Vec::new() }, &mut installed);
    assert!(installed.contains(&Output::Retain { through: Lsn(4) }), "{installed:?}");
    assert!(installed.iter().any(|o| matches!(o, Output::PersistState(_))), "{installed:?}");
    assert!(installed.iter().any(|o| matches!(o, Output::Send { to: PRIMARY, .. })), "the log after it asked for: {installed:?}");
    assert_eq!((third.standing().first, third.standing().commit, third.standing().op), (Lsn(4), Lsn(4), Lsn(4)));
}

/// Verifies: [EXTENSION] RULES.md D57 (log retention): a replica made
/// again from a window stands at it, every record through it committed;
/// one whose database, opened, holds less than its window begins at
/// fetches the state of the primary of its view, or of another member if
/// the primary is itself, delivering nothing meanwhile.
#[test]
fn a_replica_made_again_from_a_window_stands_at_it_or_fetches_state() {
    let again = Recovered {
        state: Some(state0()),
        first: Lsn(3),
        records: vec![record("r4"), record("r5")],
        marks: std::collections::BTreeMap::new(),
    };
    let made = Vr::restart(&config(3), BACKUP, options(), again, 1).unwrap();
    let standing = made.standing();
    assert_eq!((standing.first, standing.op, standing.commit, standing.applied), (Lsn(3), Lsn(5), Lsn(3), Lsn(3)));
}

/// VR's state as written in view 0, normal there.
fn state0() -> Vec<u8> {
    let mut bytes = vec![1];
    bytes.extend_from_slice(&[0; 40]);
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.push(0);
    bytes
}

/// A COMMIT frame of view 0 through `commit`, as the primary sends it.
fn commit_frame(commit: u64) -> Vec<u8> {
    let mut bytes = vec![3];
    for number in [0_u64, 0_u64, commit] {
        bytes.extend_from_slice(&number.to_le_bytes());
    }
    bytes
}

/// Verifies: [EXTENSION] RULES.md D57 [VRR §4.2]: a view change with
/// windows: the primary crashed and made again, its backups' DOVIEWCHANGE
/// carry their windows, the new primary installs the chosen one after its
/// own records before it, and its STARTVIEW carries its window, which a
/// backup that let more go skips through its own; a backup whose
/// commit-number does not reach the primary's window asks for a state
/// transfer instead of taking the view.
#[test]
fn a_view_change_carries_windows() {
    let mut group = serving(3);
    for n in 1_u64..=4_u64 {
        group.step(PRIMARY, Input::Append(record(&format!("r{n}"))));
        group.settle();
    }
    for id in ids(3) {
        group.step(id, Input::Retain(Lsn(2)));
    }
    // The third member let more go than the next primary's window: the
    // window it is given begins before its own, and is skipped through it.
    group.step(ReplicaId(3), Input::Retain(Lsn(3)));
    group.crash(PRIMARY, usize::MAX);
    crate::common::run(&mut group, &[BACKUP, ReplicaId(3)], 12_u32, &[PRIMARY]);
    let served = group.node(BACKUP).outputs.iter().any(|o| matches!(o, Output::Role(Role::Primary { view: 1 })));
    assert!(served, "the second member serves view 1: {:?}", group.node(BACKUP).outputs);
    let third = group.node(ReplicaId(3));
    assert_eq!(third.journal.first(), Lsn(3), "the window kept");
    assert_eq!(third.journal.record(Lsn(4)), Some(&record("r4")));
    assert!(
        !third.outputs.iter().any(|o| matches!(o, Output::FetchState { .. })),
        "the window skipped through, no state asked: {:?}",
        third.outputs
    );
    // A replica of a window further back, made again, whose log reaches
    // the window but whose commit-number does not, cannot take the view
    // from the STARTVIEW alone, the records between unverified: it asks
    // for the state.
    let behind = Recovered {
        state: Some(state0()),
        first: Lsn(0),
        records: vec![record("r1"), record("x2"), record("x3")],
        marks: std::collections::BTreeMap::new(),
    };
    let mut stale = Vr::restart(&config(3), PRIMARY, options(), behind, 1).unwrap();
    // The STARTVIEW the new primary sent, with its window.
    let start = group
        .node(BACKUP)
        .outputs
        .iter()
        .find_map(|o| if let Output::Send { frame, .. } = o { (frame[0] == 6_u8).then(|| frame.clone()) } else { None });
    let mut out = Vec::new();
    stale.step(Input::Received { from: BACKUP, frame: start.expect("the STARTVIEW") }, &mut out);
    assert!(out.contains(&Output::FetchState { from: BACKUP }), "{out:?}");
    // A replica whose database, opened, holds less than its window begins
    // at fetches the state of the primary of its view first, or of another
    // member if the primary is itself, delivering nothing meanwhile.
    let window =
        || Recovered { state: Some(state0()), first: Lsn(5), records: vec![record("r6")], marks: std::collections::BTreeMap::new() };
    let mut behind_files = Vr::restart(&config(3), BACKUP, options(), window(), 1).unwrap();
    let mut of_backup = Vec::new();
    behind_files.step(Input::Opened(Lsn(2)), &mut of_backup);
    assert!(of_backup.contains(&Output::FetchState { from: PRIMARY }), "{of_backup:?}");
    assert!(!of_backup.iter().any(|o| matches!(o, Output::Deliver { .. })), "{of_backup:?}");
    let mut primary_behind = Vr::restart(&config(3), PRIMARY, options(), window(), 1).unwrap();
    let mut of_primary = Vec::new();
    primary_behind.step(Input::Opened(Lsn(2)), &mut of_primary);
    assert!(of_primary.contains(&Output::FetchState { from: BACKUP }), "{of_primary:?}");
}

/// Verifies: [EXTENSION] RULES.md D57: a journal kept in a log lets the
/// records through a position go: opened again, it holds the window, its
/// records from the position after; one kept in a file log drops the
/// segments before the first record kept, once the retention is durable;
/// a retention past every record held leaves the journal empty, beginning
/// after it.
#[test]
fn a_journal_lets_records_go_with_its_segments() {
    let log: Arc<dyn Log> = Arc::new(MemoryLog::new());
    let (mut journal, _) = LogJournal::open(Arc::clone(&log)).unwrap();
    for at in 1_u64..=4_u64 {
        journal.persist(Lsn(at), &record(&format!("r{at}"))).unwrap();
    }
    journal.persist_state(b"view").unwrap();
    journal.retain(Lsn(2)).unwrap();
    let (mut again, recovered) = LogJournal::open(Arc::clone(&log)).unwrap();
    assert_eq!(
        recovered,
        Recovered {
            state: Some(b"view".to_vec()),
            first: Lsn(2),
            records: vec![record("r3"), record("r4")],
            marks: std::collections::BTreeMap::new()
        }
    );
    again.persist(Lsn(5), &record("r5")).unwrap();
    again.retain(Lsn(9)).unwrap();
    let (_, emptied) = LogJournal::open(Arc::clone(&log)).unwrap();
    assert_eq!(
        emptied,
        Recovered { state: Some(b"view".to_vec()), first: Lsn(9), records: Vec::new(), marks: std::collections::BTreeMap::new() }
    );

    let scratch = Scratch::new("replication-retention");
    let dir = scratch.path("log");
    let options = FileLogOptions { segment_size: 4096, durability: Durability::None, ..FileLogOptions::default() };
    let files: Arc<dyn Log> = Arc::new(FileLog::create(os(), &dir, crc32c(), options).unwrap());
    let (mut on_files, _) = LogJournal::open(Arc::clone(&files)).unwrap();
    for at in 1_u64..=40_u64 {
        on_files.persist(Lsn(at), &vec![u8::try_from(at).unwrap(); 1000_usize]).unwrap();
    }
    on_files.persist_state(b"view").unwrap();
    let segments = || std::fs::read_dir(&dir).unwrap().count();
    let before = segments();
    assert!(before > 3_usize, "{before} segments");
    on_files.retain(Lsn(30)).unwrap();
    assert!(segments() < before, "segments dropped: {} of {before}", segments());
    let (_, kept) = LogJournal::open(Arc::clone(&files)).unwrap();
    assert_eq!((kept.first, kept.records.len()), (Lsn(30), 10_usize));
    assert_eq!(kept.records[0], vec![31_u8; 1000_usize]);
}

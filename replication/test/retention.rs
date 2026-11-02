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

use crate::common::{ids, options, record, serving};

const PRIMARY: ReplicaId = ReplicaId(1);
const BACKUP: ReplicaId = ReplicaId(2);

/// Verifies: [EXTENSION] RULES.md D57 (log retention): a backup told
/// `Retain` lets the records through the position go, as far as they are
/// committed, its journal told, and no further than its window begins;
/// its standing names the window; a GETSTATE for a position before the
/// window is answered NEEDSTATE, by which the asker has its driver fetch a
/// state transfer, serving no more meanwhile; the transfer installed at a
/// position, its journal begins after it, its state is written, and it
/// fetches the log after; a replica made again from a window stands at it,
/// every record through it committed.
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
    let mut third = Vr::new(&ids(3), ReplicaId(3), options()).unwrap();
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
    third.step(Input::Received { from: PRIMARY, frame: need }, &mut out);
    assert!(out.contains(&Output::FetchState { from: PRIMARY }), "{out:?}");
    assert_eq!(third.standing().status, Status::Transfer);
    let mut installed = Vec::new();
    third.step(Input::Installed(Lsn(4)), &mut installed);
    assert!(installed.contains(&Output::Retain { through: Lsn(4) }), "{installed:?}");
    assert!(installed.iter().any(|o| matches!(o, Output::PersistState(_))), "{installed:?}");
    assert!(installed.iter().any(|o| matches!(o, Output::Send { to: PRIMARY, .. })), "the log after it asked for: {installed:?}");
    assert_eq!((third.standing().first, third.standing().commit, third.standing().op), (Lsn(4), Lsn(4), Lsn(4)));

    let again = Recovered { state: None, first: Lsn(3), records: vec![record("r4"), record("r5")] };
    let made = Vr::restart(&ids(3), BACKUP, options(), again).unwrap();
    let standing = made.standing();
    assert_eq!((standing.first, standing.op, standing.commit, standing.applied), (Lsn(3), Lsn(5), Lsn(3), Lsn(3)));
}

/// A COMMIT frame of view 0 through `commit`, as the primary sends it.
fn commit_frame(commit: u64) -> Vec<u8> {
    let mut bytes = vec![3];
    bytes.extend_from_slice(&0_u64.to_le_bytes());
    bytes.extend_from_slice(&commit.to_le_bytes());
    bytes
}

/// Verifies: [EXTENSION] RULES.md D57 [VRR §4.2]: a view change with
/// windows: the primary crashed and made again, its backups' DOVIEWCHANGE
/// carry their windows, the new primary installs the chosen one after its
/// own records before it, and its STARTVIEW carries its window; a backup
/// whose log does not reach the primary's window asks for a state
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
    group.crash(PRIMARY, usize::MAX);
    crate::common::run(&mut group, &[BACKUP, ReplicaId(3)], 12_u32, &[PRIMARY]);
    let served = group.node(BACKUP).outputs.iter().any(|o| matches!(o, Output::Role(Role::Primary { view: 1 })));
    assert!(served, "the second member serves view 1: {:?}", group.node(BACKUP).outputs);
    assert_eq!(group.node(ReplicaId(3)).journal.first(), Lsn(2), "the window kept");
    assert_eq!(group.node(ReplicaId(3)).journal.record(Lsn(4)), Some(&record("r4")));
    // A replica of a window further back, made again, cannot take the
    // view from the STARTVIEW alone: it asks for the state.
    let behind = Recovered { state: None, first: Lsn(0), records: vec![record("r1")] };
    let mut stale = Vr::restart(&ids(3), PRIMARY, options(), behind).unwrap();
    let start = group.frames.iter().find(|f| f.bytes[0] == 6_u8).map(|f| f.bytes.clone());
    if let Some(frame) = start {
        let mut out = Vec::new();
        stale.step(Input::Received { from: BACKUP, frame }, &mut out);
        assert!(out.contains(&Output::FetchState { from: BACKUP }), "{out:?}");
    }
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
    assert_eq!(recovered, Recovered { state: Some(b"view".to_vec()), first: Lsn(2), records: vec![record("r3"), record("r4")] });
    again.persist(Lsn(5), &record("r5")).unwrap();
    again.retain(Lsn(9)).unwrap();
    let (_, emptied) = LogJournal::open(Arc::clone(&log)).unwrap();
    assert_eq!(emptied, Recovered { state: Some(b"view".to_vec()), first: Lsn(9), records: Vec::new() });

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

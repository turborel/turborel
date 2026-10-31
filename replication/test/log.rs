//! The replicated log of one replica (RULES.md D25, D57): the suites every
//! log passes, of `turborel-test-support`, run on it as the core runs them
//! on its built-in logs, over a journal in memory and over the core's file
//! log; then what is its own: its journal's entries, a replica made again
//! from its journal, and what it refuses.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use turborel::bytes::Put;
use turborel::error::Code;
use turborel::log::{FileLog, FileLogOptions, Log, Lsn, MemoryLog};
use turborel_replication::{
    ALONE, Consensus, Input, Journal, LogJournal, MakeAlone, Output, Recovered, ReplicaEvent, ReplicaId, ReplicatedLog, ReplicatedLogs,
    Role, STORAGE_NAME, Vr, vr_alone,
};
use turborel_test_support::Scratch;
use turborel_test_support::database::{crc32c, os};
use turborel_test_support::logs::{self, LogKind, Watched, all, contract, record};

use crate::common::options;

/// VR of the tests' options, alone.
fn make() -> MakeAlone {
    vr_alone(options())
}

/// A new replicated log whose journal is kept in `journal`.
fn over(journal: Arc<dyn Log>) -> Arc<ReplicatedLog> {
    Arc::new(ReplicatedLog::create(journal, &make()).unwrap())
}

/// A new replicated log, its journal in memory.
fn in_memory() -> Arc<ReplicatedLog> {
    over(Arc::new(MemoryLog::new()))
}

/// Replicated logs of one replica, their journals file logs.
struct Replicated;

impl LogKind for Replicated {
    fn create(&self, dir: &Path) -> Arc<dyn Log> {
        over(Arc::new(FileLog::create(os(), dir, crc32c(), FileLogOptions::default()).unwrap()))
    }
    fn open(&self, dir: &Path) -> Arc<dyn Log> {
        let (journal, _) = FileLog::open(os(), dir, &[crc32c()], &[], FileLogOptions::default()).unwrap();
        Arc::new(ReplicatedLog::open(Arc::new(journal), &make()).unwrap())
    }
    fn storage(&self) -> Option<Box<dyn turborel::db::LogStorage>> {
        Some(Box::new(ReplicatedLogs::vr(FileLogOptions::default(), options())))
    }
}

/// Verifies: [DESIGN] RULES.md D25, D57, `Log` contract, the replicated log
/// of one replica, its journal in memory and in a file log: positions,
/// reading back, syncs and truncation as the contract says; as durable as
/// its journal.
#[test]
fn the_replicated_log_keeps_the_contract() {
    let memory = in_memory();
    contract(&*memory);
    assert!(!memory.capabilities().durable);
    let scratch = Scratch::new("replicated-contract");
    let files = Replicated.create(&scratch.path("log"));
    contract(&*files);
    assert!(files.capabilities().durable);
}

/// Verifies: [DESIGN] RULES.md D25, D57, the replicated log: a
/// transaction's records appended, synced at the commit, and read back
/// after the log is opened again, decode to what was written, in order.
#[test]
fn records_go_through_the_replicated_log() {
    let scratch = Scratch::new("replicated-records");
    logs::records_go_through(&Replicated, &scratch.path("log"));
}

/// Verifies: [X3.135-1986 §8.2 GR3]; [DESIGN] RULES.md D25, D57: over the
/// replicated log, a transaction's changes are logged as made, then its
/// commit record, synced; COMMIT SYNC OFF commits unsynced, a later sync
/// covering it; a rollback and a failed statement are logged as such; a
/// read logs nothing; definitions are logged and synced before they are
/// published.
#[test]
fn the_database_writes_the_replicated_log_as_any() {
    logs::a_commit_is_its_record_made_durable(in_memory());
    logs::a_commit_need_not_wait_for_the_log(in_memory());
    logs::rollbacks_and_reads_are_logged_as_needed(in_memory());
    logs::a_failed_statement_is_logged_undone(in_memory());
    logs::definitions_are_logged_before_they_are_published(in_memory());
}

/// Verifies: [DESIGN] RULES.md D25, D57: with the replicated log, a
/// committed transaction's records are in its journal's files once COMMIT
/// returns: the log opened anew holds its changes and its commit.
#[test]
fn the_replicated_log_holds_what_was_committed() {
    let scratch = Scratch::new("replicated-holds");
    logs::a_log_holds_what_was_committed(&Replicated, &scratch.path("log"));
}

/// Verifies: [X3.135-1986 §4.16, §8.9]; [DESIGN] RULES.md D25, D57: a
/// database recovered from the replicated log after a crash holds what was
/// committed and only that, its definitions back; recovered again from the
/// log grown since, the new rows too.
#[test]
fn a_crashed_database_recovers_from_the_replicated_log() {
    let scratch = Scratch::new("replicated-recovery");
    logs::a_crashed_database_recovers_what_was_committed(&Replicated, &scratch.path("log"));
}

/// Verifies: [X3.135-1986 §4.16]; [DESIGN] RULES.md D25, D57: a database
/// kept in a directory, its log replicated (`ReplicatedLogs`), closed
/// cleanly, is reopened in place, its replica made again from its journal;
/// one stopped without closing is recovered from it.
#[test]
fn a_database_with_a_replicated_log_reopens_and_recovers() {
    let scratch = Scratch::new("replicated-reopen");
    logs::a_closed_database_reopens_in_place(&Replicated, &scratch.path("clean"));
    logs::a_database_not_closed_is_recovered(&Replicated, &scratch.path("crash"));
    assert_eq!(Replicated.storage().unwrap().name(), STORAGE_NAME);
}

/// The entries a journal kept in `log` was given, as `LogJournal` writes
/// them: a record at a position, a truncation, a state.
fn entry(tag: u8, at: u64, bytes: &[u8]) -> Vec<u8> {
    let mut entry = vec![tag];
    if tag != 3 {
        entry.put_u64(at);
    }
    entry.extend_from_slice(bytes);
    entry
}

/// Verifies: [DESIGN] RULES.md D57, the journal kept in a log: records
/// written at the position after the last, truncated, written again, and a
/// state, are what it holds once opened again; `sync` returns each record
/// made durable once, in order, those a state made durable too; an entry
/// of no tag known, cut short, or writing past the position after the
/// last, is refused (SQLCODE -916).
#[test]
fn the_journal_holds_what_its_writes_left() {
    let log: Arc<dyn Log> = Arc::new(MemoryLog::new());
    let (mut journal, recovered) = LogJournal::open(Arc::clone(&log)).unwrap();
    assert_eq!(recovered, Recovered::default());
    for n in 1..=3_u64 {
        journal.persist(Lsn(n), &record(usize::try_from(n).unwrap())).unwrap();
    }
    assert_eq!(journal.sync().unwrap(), [Lsn(1), Lsn(2), Lsn(3)]);
    assert_eq!(journal.sync().unwrap(), [], "each once");
    journal.truncate(Lsn(1)).unwrap();
    journal.persist(Lsn(2), b"again").unwrap();
    journal.persist_state(b"view").unwrap();
    assert_eq!(journal.sync().unwrap(), [Lsn(2)], "made durable by the state");
    let (_, reopened) = LogJournal::open(Arc::clone(&log)).unwrap();
    assert_eq!(reopened, Recovered { state: Some(b"view".to_vec()), records: vec![record(1), b"again".to_vec()] });
    for bad in [vec![9_u8], vec![1_u8, 1], entry(1, 4, b"past"), entry(1, 0, b"none before 1"), entry(2, 1, b"more")] {
        let damaged: Arc<dyn Log> = Arc::new(MemoryLog::new());
        damaged.append(&bad).unwrap();
        assert_eq!(LogJournal::open(damaged).err().map(|e| e.code), Some(Code::Corrupt), "{bad:?}");
    }
}

/// Verifies: [DESIGN] RULES.md D57, a replica alone made again from its
/// journal [VRR §4.3]: it does not serve again in the view it served in,
/// but takes the next view at once, there being no other replica to wait
/// for, its records where they were; appends go on after them, committed
/// as they are synced, and the view is written to its journal.
#[test]
fn a_replica_alone_rejoins_in_the_next_view() {
    let journal: Arc<dyn Log> = Arc::new(MemoryLog::new());
    let log = over(Arc::clone(&journal));
    let first: Vec<Lsn> = (0..5).map(|n| log.append(&record(n)).unwrap()).collect();
    log.sync(first[4]).unwrap();
    drop(log);
    let (_, held) = LogJournal::open(Arc::clone(&journal)).unwrap();
    assert_eq!((held.state, held.records.len()), (None, 5), "no view written in view 0");
    let again = ReplicatedLog::open(Arc::clone(&journal), &make()).unwrap();
    assert_eq!(all(&again, Lsn(0)), first.iter().copied().zip((0..5).map(record)).collect::<Vec<_>>());
    let next = again.append(b"after").unwrap();
    assert_eq!(next, Lsn(6));
    again.sync(next).unwrap();
    let (_, later) = LogJournal::open(journal).unwrap();
    assert!(later.state.is_some(), "the next view written");
    assert_eq!(later.records.len(), 6);
    assert_eq!(again.read_at(Lsn(0)).unwrap(), None);
    assert_eq!(again.read_at(next).unwrap(), Some(b"after".to_vec()));
}

/// Verifies: [DESIGN] RULES.md D57, refusals of the replicated log: a new
/// replica's journal holding an entry (SQLCODE -910); a consensus that
/// does not serve alone, as a backup of a larger group (-910); a sync the
/// group has not committed, as one of a larger group appending with no
/// backup to hear it (-915).
#[test]
fn the_replicated_log_refuses_what_it_cannot_be() {
    let used: Arc<dyn Log> = Arc::new(MemoryLog::new());
    used.append(&entry(3, 0, b"view")).unwrap();
    assert_eq!(ReplicatedLog::create(used, &make()).err().map(|e| e.code), Some(Code::Api));
    let group = [ALONE, ReplicaId(2), ReplicaId(3)];
    let backup: MakeAlone = Arc::new(move |_| Ok(Box::new(Vr::new(&group, ReplicaId(2), options())?)));
    assert_eq!(ReplicatedLog::create(Arc::new(MemoryLog::new()), &backup).err().map(|e| e.code), Some(Code::Api));
    let primary: MakeAlone = Arc::new(move |_| Ok(Box::new(Vr::new(&group, ALONE, options())?)));
    let log = ReplicatedLog::create(Arc::new(MemoryLog::new()), &primary).unwrap();
    let at = log.append(b"unheard").unwrap();
    assert_eq!(log.sync(at).err().map(|e| e.code), Some(Code::Io));
    log.sync(Lsn(0)).unwrap();
}

/// Verifies: [DESIGN] RULES.md D57, a failing journal: an append it cannot
/// write fails (SQLCODE -915), and so does every call after that changes
/// the log, as the consensus counted on what the journal lost; a sync it
/// cannot make fails so too. What the log holds is still read.
#[test]
fn a_failing_journal_stops_the_replicated_log() {
    let appends = Arc::new(Watched::default());
    let inner: Arc<dyn Log> = appends.clone();
    let log = over(inner);
    let kept = log.append(b"kept").unwrap();
    appends.fail_appends.store(true, Ordering::SeqCst);
    assert_eq!(log.append(b"lost").err().map(|e| e.code), Some(Code::Io));
    appends.fail_appends.store(false, Ordering::SeqCst);
    assert_eq!(log.append(b"after").err().map(|e| e.code), Some(Code::Io), "stopped");
    assert_eq!(log.sync(kept).err().map(|e| e.code), Some(Code::Io));
    assert_eq!(log.read_at(kept).unwrap(), Some(b"kept".to_vec()));

    let syncs = Arc::new(Watched::default());
    let failing: Arc<dyn Log> = syncs.clone();
    let unsynced = over(failing);
    let at = unsynced.append(b"unsynced").unwrap();
    syncs.fail_syncs.store(true, Ordering::SeqCst);
    assert_eq!(unsynced.sync(at).err().map(|e| e.code), Some(Code::Io));
    syncs.fail_syncs.store(false, Ordering::SeqCst);
    assert_eq!(unsynced.sync(at).err().map(|e| e.code), Some(Code::Io), "stopped");
}

/// A consensus of the test's own: it serves at its first tick, and answers
/// an append with what `answer` gives, and every output the log carries
/// out alone without changing anything.
struct Scripted {
    answer: fn(Vec<u8>) -> Vec<Output>,
}

impl Consensus for Scripted {
    fn name(&self) -> &'static str {
        "scripted"
    }
    fn step(&mut self, input: Input, out: &mut Vec<Output>) {
        match input {
            Input::Tick => out.push(Output::Role(Role::Primary { view: 0 })),
            Input::Append(record) => out.extend((self.answer)(record)),
            Input::Received { .. } | Input::Persisted(_) | Input::Applied(_) | Input::Installed(_) => {}
        }
    }
}

/// Verifies: [DESIGN] RULES.md D57, the replicated log carries out a
/// consensus's outputs as its contract says: an append refused, or given
/// no position, fails (SQLCODE -910); frames, deliveries, outcomes, state
/// fetches, events to the operator and a change of role change nothing
/// the log holds.
#[test]
fn the_replicated_log_carries_out_what_its_consensus_says() {
    let made = |answer: fn(Vec<u8>) -> Vec<Output>| {
        let journal = LogJournal::open(Arc::new(MemoryLog::new())).unwrap().0;
        ReplicatedLog::alone(Box::new(Scripted { answer }), Box::new(journal), Vec::new()).unwrap()
    };
    let refused = made(|_| vec![Output::Assigned(Lsn(1)), Output::Event(ReplicaEvent::AppendRefused)]);
    assert_eq!(refused.append(b"r").err().map(|e| e.code), Some(Code::Api));
    let silent = made(|_| vec![]);
    assert_eq!(silent.append(b"s").err().map(|e| e.code), Some(Code::Api));
    let noisy = made(|record| {
        vec![
            Output::Send { to: ReplicaId(2), frame: record.clone() },
            Output::Deliver { through: Lsn(1) },
            Output::Outcome { at: Lsn(1), kept: true },
            Output::FetchState { from: ReplicaId(2) },
            Output::Event(ReplicaEvent::ViewChanged { view: 1, primary: ALONE }),
            Output::Event(ReplicaEvent::FrameRefused { from: ReplicaId(2), why: String::new() }),
            Output::Persist { at: Lsn(1), record },
            Output::Assigned(Lsn(1)),
            Output::Role(Role::Backup),
        ]
    });
    assert_eq!(noisy.append(b"n").unwrap(), Lsn(1));
    assert_eq!(noisy.end(), Lsn(2));
    noisy.truncate_before(Lsn(2)).unwrap();
    assert_eq!(all(&noisy, Lsn(0)).len(), 1, "nothing removed: the consensus keeps its whole log");
    assert_eq!(Scripted { answer: |_| vec![] }.name(), "scripted");
}

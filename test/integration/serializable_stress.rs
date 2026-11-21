//! A differential stress test of SERIALIZABLE (RULES.md D29, D440, D441):
//! sessions on threads run transactions that each read two rows and write
//! a third from what they read, while every commit releases its locks
//! before its sync. Each committed transaction, replayed alone in commit
//! order, reads what it read and leaves what the database holds. A failed
//! sync and a crash part way through commits are injected: what the
//! database shows, or recovers, is still a replay of what committed.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

use turborel::db::{Database, OpenOptions, Session};
use turborel::error::Code;
use turborel::log::Lsn;
use turborel::log::record::Change;
use turborel::txn::LockWait;

use crate::common::{os, rows};
use crate::faults::{FaultyFs, Op, Rule};
use crate::isolation_anomalies::Slow;
use crate::logs::records;

/// Rows of the table.
const KEYS: u64 = 6;
/// Sessions writing at once.
const THREADS: u64 = 4;
/// Transactions each session tries.
const TRIES: u64 = 120;
/// What a written value is taken modulo, so it stays an INTEGER.
const MODULUS: i64 = 1_000_003;

/// A transaction as it ran: the rows it read, with their values, the row
/// it wrote, the value, and how it ended, with its commit's log position.
#[derive(Debug, Clone)]
struct Ran {
    read: [(u64, i64); 2],
    wrote: (u64, i64),
    ended: Result<u64, Code>,
}

/// Creates `T (K, V)` in `db`, `KEYS` rows with `V` 0.
fn create(db: &Database) {
    db.set_lock_wait(LockWait::Wait).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER NOT NULL PRIMARY KEY, V INTEGER)").unwrap();
    let s = db.session("A");
    for k in 0..KEYS {
        s.run(&format!("INSERT INTO T VALUES ({k}, 0)")).unwrap();
    }
    s.commit().unwrap();
}

/// The table as `db` holds it now.
fn table(db: &Database) -> BTreeMap<u64, i64> {
    let s = db.session("A");
    let read = rows(&s, "SELECT K, V FROM T").unwrap();
    s.commit().unwrap();
    read.iter().map(|r| r.split_once(',').map(|(k, v)| (k.parse().unwrap(), v.parse().unwrap())).unwrap()).collect()
}

/// The value of row `k`, as `s` reads it.
fn value(s: &Session, k: u64) -> Result<i64, Code> {
    rows(s, &format!("SELECT V FROM T WHERE K = {k}")).map(|r| r.concat().parse().unwrap())
}

/// One transaction of session `s`, its rows chosen from `seed`: reads two
/// rows, writes their sum plus one to a third, commits.
fn one(s: &Session, seed: u64) -> Option<Ran> {
    let (a, b, c) = (seed % KEYS, (seed >> 8) % KEYS, (seed >> 16) % KEYS);
    s.run("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE").unwrap();
    let read = value(s, a).and_then(|va| value(s, b).map(|vb| [(a, va), (b, vb)]));
    let Ok(read) = read else {
        let _ = s.rollback();
        return None;
    };
    let wrote = (c, (read[0].1 + read[1].1 + 1) % MODULUS);
    let written = s.run(&format!("UPDATE T SET V = {} WHERE K = {c}", wrote.1));
    let ended = written.and_then(|_| s.commit()).map_err(|e| e.code);
    if ended.is_err() {
        let _ = s.rollback();
    }
    let ended = ended.map(|()| s.canceller().served().take_committed().unwrap());
    Some(Ran { read, wrote, ended })
}

/// Runs `THREADS` sessions of `db`, each `TRIES` transactions or until a
/// commit fails other than by a conflict, and calls `midway` part way.
fn run(db: &Database, midway: impl FnOnce()) -> Vec<Ran> {
    let workers: Vec<_> = (0..THREADS)
        .map(|t| {
            let s = db.session("A");
            thread::spawn(move || {
                let mut ran = Vec::new();
                for i in 0..TRIES {
                    let seed = (t * 7919 + i * 104_729).wrapping_mul(2_654_435_761) >> 7;
                    let Some(r) = one(&s, seed) else { continue };
                    let failed = r.ended.is_err_and(|c| c != Code::Conflict);
                    ran.push(r);
                    if failed {
                        break;
                    }
                }
                ran
            })
        })
        .collect();
    thread::sleep(Duration::from_millis(150));
    midway();
    workers.into_iter().flat_map(|w| w.join().unwrap()).collect()
}

/// Replays `committed`, in order, over a table of `KEYS` zeros: each must
/// read what it read. The table left.
fn replay(committed: &[&Ran]) -> BTreeMap<u64, i64> {
    let mut model: BTreeMap<u64, i64> = (0..KEYS).map(|k| (k, 0)).collect();
    for (n, ran) in committed.iter().enumerate() {
        for (k, v) in ran.read {
            assert_eq!(model[&k], v, "transaction {n} of the commit order read {k} as {v}: {ran:?}");
        }
        model.insert(ran.wrote.0, ran.wrote.1);
    }
    model
}

/// The commit timestamps of the commit records in `log`, by position.
fn commit_order(log: &dyn turborel::log::Log) -> BTreeMap<u64, u64> {
    records(log)
        .into_iter()
        .filter_map(|(Lsn(lsn), r)| if let Change::Commit { at, .. } = r.change { Some((lsn, at)) } else { None })
        .collect()
}

/// The transactions of `ran` that committed, in commit timestamp order.
fn in_commit_order<'r>(ran: &'r [Ran], log: &dyn turborel::log::Log) -> Vec<&'r Ran> {
    let order = commit_order(log);
    let mut committed: Vec<(u64, &Ran)> = ran.iter().filter_map(|r| r.ended.ok().map(|lsn| (order[&lsn], r))).collect();
    committed.sort_by_key(|(at, _)| *at);
    committed.into_iter().map(|(_, r)| r).collect()
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.41.2; ISO/IEC 9075:1992 §4.28];
/// [DESIGN] RULES.md D29, D441: every committed SERIALIZABLE transaction,
/// replayed alone in commit timestamp order, reads what it read, and the
/// replay leaves what the database holds; the rest were refused with a
/// serialization failure (SQLCODE -911) alone. Its commits release their
/// locks before a slow sync.
#[test]
fn committed_transactions_replay_in_commit_order() {
    let db = Database::new();
    let log = Arc::new(Slow::new(Duration::from_millis(2)));
    db.set_log(Arc::clone(&log) as Arc<dyn turborel::log::Log>).unwrap();
    create(&db);
    let ran = run(&db, || {});
    assert!(ran.iter().all(|r| r.ended.is_ok() || r.ended == Err(Code::Conflict)), "{ran:?}");
    let committed = in_commit_order(&ran, &*log);
    assert!(committed.len() * 4 > ran.len(), "{} of {}", committed.len(), ran.len());
    assert_eq!(replay(&committed), table(&db));
}

/// Verifies: [DESIGN] RULES.md D25, D441: when a sync fails part way, every
/// commit after is in doubt (SQLCODE -922) or refused, and what the
/// database shows is the replay, in commit order, of the commits that
/// returned: none in doubt, nor one that read a commit in doubt, is seen.
#[test]
fn a_failed_sync_shows_only_what_returned_committed() {
    let db = Database::new();
    let log = Arc::new(Slow::new(Duration::from_millis(2)));
    db.set_log(Arc::clone(&log) as Arc<dyn turborel::log::Log>).unwrap();
    create(&db);
    let ran = run(&db, || log.failing.store(true, Ordering::SeqCst));
    let kinds = [Code::Conflict, Code::InDoubt, Code::Io, Code::CommitFailed];
    assert!(ran.iter().all(|r| r.ended.is_ok() || r.ended.is_err_and(|c| kinds.contains(&c))), "{ran:?}");
    assert!(ran.iter().any(|r| r.ended == Err(Code::InDoubt)), "{ran:?}");
    let committed = in_commit_order(&ran, &*log);
    assert_eq!(replay(&committed), table(&db));
}

/// Verifies: [DESIGN] RULES.md D25, D441, a crash part way through commits:
/// the log's syncs fail while sessions commit, and the database is opened
/// again over its files, cut back to what was synced or not: it recovers
/// every commit that returned, and of those in doubt only some, so that
/// what it holds is the replay of the commits that returned with some of
/// those in doubt.
#[test]
fn a_crash_part_way_recovers_what_returned() {
    for cut in [false, true] {
        let scratch = crate::common::Scratch::new(&format!("serializable-stress-crash-{cut}"));
        let dir = scratch.path("db");
        let (fs, faults) = FaultyFs::with_faults();
        let db = Database::new();
        db.open_files(&fs, &dir, &OpenOptions::default()).unwrap();
        create(&db);
        let ran = run(&db, || faults.inject(Rule::fail(Op::Sync).on("/log/")));
        faults.clear();
        let segment = last_segment(&dir);
        drop(db);
        if cut {
            faults.cut_to_synced(&segment);
        }
        let again = Database::new();
        again.open_files(&os(), &dir, &OpenOptions::default()).unwrap();
        let recovered = table(&again);
        let returned: Vec<&Ran> = ran.iter().filter(|r| r.ended.is_ok()).collect();
        let doubt: Vec<&Ran> = ran.iter().filter(|r| r.ended == Err(Code::InDoubt)).collect();
        assert!(recovered_some(&returned, &doubt, &recovered), "cut {cut}: {recovered:?} from {ran:?}");
    }
}

/// Whether `recovered` is the table the commits `returned` leave, with
/// some of `doubt` after them, each writing the value it wrote.
fn recovered_some(returned: &[&Ran], doubt: &[&Ran], recovered: &BTreeMap<u64, i64>) -> bool {
    let mut base: BTreeMap<u64, i64> = (0..KEYS).map(|k| (k, 0)).collect();
    let mut by_position: Vec<&&Ran> = returned.iter().collect();
    by_position.sort_by_key(|r| r.ended.unwrap_or(0));
    for ran in by_position {
        base.insert(ran.wrote.0, ran.wrote.1);
    }
    (0..1_u32 << doubt.len()).any(|chosen| {
        let mut table = base.clone();
        for (i, ran) in doubt.iter().enumerate() {
            if chosen & (1 << i) != 0 {
                table.insert(ran.wrote.0, ran.wrote.1);
            }
        }
        table == *recovered
    })
}

/// The newest segment of the log of the database in `dir`.
fn last_segment(dir: &std::path::Path) -> std::path::PathBuf {
    let mut segments: Vec<std::path::PathBuf> = std::fs::read_dir(dir.join("log")).unwrap().map(|e| e.unwrap().path()).collect();
    segments.sort();
    segments.pop().unwrap()
}

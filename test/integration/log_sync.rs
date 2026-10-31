//! The log synced at a pace of its own (RULES.md D25, D36, D55): a commit
//! of LOG.COMMIT_SYNC OFF made durable within LOG.SYNC_AFTER_MS by the job
//! `turborel-log-sync`, with no other commit, over the file log and a log
//! plug-in; the job shown, stopped by a close, and its failed sync
//! stopping the database taking changes.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use turborel::db::{Database, OpenOptions, Opened};
use turborel::error::Code;
use turborel::log::{Log, Lsn};
use turborel::threads::{JobInfo, JobPolicy, JobState};
use turborel::value::Value;

use crate::common::{Kept, Scratch, Watched, after_crash, count, number, setup};
use crate::faults::FaultyFs;

/// The label of the log's sync job.
const LOG_SYNC: &str = "turborel-log-sync";

/// The log sync job of `db`.
fn job(db: &Database) -> JobInfo {
    db.jobs().into_iter().find(|t| t.label == LOG_SYNC).unwrap()
}

/// LOG.COMMIT_SYNC OFF, as a value.
fn off() -> Value {
    Value::Char("OFF".to_owned())
}

/// The database of `setup`, under `policy`, logging to a fresh watched
/// log, a plug-in.
fn watched(policy: JobPolicy) -> (Database, Arc<Watched>) {
    let db = setup();
    db.set_job_policy(policy).unwrap();
    let log = Arc::new(Watched::default());
    db.set_log(Arc::clone(&log) as Arc<dyn Log>).unwrap();
    (db, log)
}

/// Inserts `id` into T in a transaction of its own, committed by a
/// session of LOG.COMMIT_SYNC OFF.
fn unwaited(db: &Database, id: i32) {
    let s = db.session("APP");
    s.set_setting("LOG.COMMIT_SYNC", &off()).unwrap();
    s.run(&format!("INSERT INTO T (ID) VALUES ({id})")).unwrap();
    s.commit().unwrap();
}

/// The position after the last record of `log`: of the last commit.
fn last(log: &Watched) -> Lsn {
    Lsn(log.end().0 - 1_u64)
}

/// The newest segment of the log of the database in `dir`.
fn last_segment(dir: &std::path::Path) -> std::path::PathBuf {
    let mut segments: Vec<std::path::PathBuf> = std::fs::read_dir(dir.join("log")).unwrap().map(|e| e.unwrap().path()).collect();
    segments.sort();
    segments.pop().unwrap()
}

/// Verifies: [DESIGN] RULES.md D25, D55, over the file log: a commit of
/// LOG.COMMIT_SYNC OFF leaves the log sync job due, no other commit
/// following; a failure of the system before it runs (the log cut to its
/// last sync) loses the commit, and after it runs keeps it, recovered.
#[test]
fn an_unwaited_commit_is_durable_once_its_job_runs() {
    for ran in [false, true] {
        let scratch = Scratch::new(&format!("log-sync-{ran}"));
        let dir = scratch.path("db");
        let (fs, faults) = FaultyFs::with_faults();
        let db = Database::new();
        db.open_files(&fs, &dir, &OpenOptions::default()).unwrap();
        db.set_job_policy(JobPolicy::Manual).unwrap();
        db.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (ID INTEGER)").unwrap();
        assert_eq!(job(&db).state, JobState::Idle, "no commit unsynced");
        unwaited(&db, 1_i32);
        assert_eq!(job(&db).state, JobState::Due);
        if ran {
            db.run_due().unwrap();
            let done = job(&db);
            assert_eq!((done.state, done.runs, done.last_error.is_none()), (JobState::Idle, 1_u64, true));
        }
        drop(db);
        faults.cut_to_synced(&last_segment(&dir));
        let again = Database::new();
        let opened = again.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
        assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
        let expected = usize::from(ran);
        assert_eq!(count(&again.session("APP"), "SELECT ID FROM T"), expected, "synced by its job: {ran}");
        again.close().unwrap();
    }
}

/// Verifies: [DESIGN] RULES.md D25, D55, D36, over a log plug-in: under
/// JOBS.POLICY AUTOMATIC the job's thread, started and told as an event,
/// syncs the log through a commit of LOG.COMMIT_SYNC OFF once
/// LOG.SYNC_AFTER_MS passes, with no other commit; INFORMATION_SCHEMA.JOBS
/// shows the job to the database's owner.
#[test]
fn the_job_syncs_on_its_thread_after_its_time() {
    let (db, log) = watched(JobPolicy::Automatic);
    let kept = Kept::default();
    db.set_diagnostics(Box::new(kept.clone()));
    db.set_setting("LOG.SYNC_AFTER_MS", &number(1_i32)).unwrap();
    let before = log.synced.lock().unwrap().len();
    unwaited(&db, 10_i32);
    let commit = last(&log);
    let began = Instant::now();
    while !log.synced.lock().unwrap().contains(&commit) && began.elapsed() < Duration::from_secs(30_u64) {
        std::thread::sleep(Duration::from_millis(1_u64));
    }
    assert_eq!(log.synced.lock().unwrap()[before..], [commit], "synced through the commit, once");
    assert!(job(&db).threaded, "on a thread of its own");
    let told: Vec<String> = kept.taken().into_iter().map(|(_, e)| e).collect();
    assert!(told.iter().any(|e| e.contains(LOG_SYNC) && e.contains("Started")), "{told:?}");
    let shown = "SELECT TRIM(LABEL), TRIM(THREADED) FROM INFORMATION_SCHEMA.JOBS WHERE LABEL = 'turborel-log-sync'";
    db.create_user("OWNER", b"x").unwrap();
    db.set_owner("OWNER").unwrap();
    assert_eq!(crate::common::rows(&db.session("OWNER"), shown), Ok(vec!["turborel-log-sync,YES".to_owned()]), "to the owner");
}

/// Verifies: [DESIGN] RULES.md D55, D25: a close stops and joins the log
/// sync job's thread, waiting out LOG.SYNC_AFTER_MS, told as an event, and
/// the close syncs the log itself: the commit it waited for is there when
/// opened again. A commit that waits syncs, so the job's next round finds
/// nothing; one unsynced after asks for it again.
#[test]
fn a_close_stops_the_job() {
    let scratch = Scratch::new("log-sync-close");
    let dir = scratch.path("db");
    let db = Database::new();
    let kept = Kept::default();
    db.set_diagnostics(Box::new(kept.clone()));
    db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
    db.set_setting("LOG.SYNC_AFTER_MS", &number(600_000_i32)).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (ID INTEGER)").unwrap();
    unwaited(&db, 1_i32);
    let s = db.session("APP");
    s.run("INSERT INTO T (ID) VALUES (2)").unwrap();
    s.commit().unwrap();
    unwaited(&db, 3_i32);
    let waiting = job(&db);
    assert_eq!((waiting.state, waiting.threaded, waiting.runs), (JobState::Due, true, 0_u64), "waiting out its time");
    drop(s);
    db.close().unwrap();
    assert!(!job(&db).threaded, "stopped");
    let told: Vec<String> = kept.taken().into_iter().map(|(_, e)| e).collect();
    assert!(told.iter().any(|e| e.contains(LOG_SYNC) && e.contains("Stopped")), "{told:?}");
    let again = Database::new();
    again.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
    assert_eq!(count(&again.session("APP"), "SELECT ID FROM T"), 3_usize);
    again.close().unwrap();
}

/// Verifies: [DESIGN] RULES.md D25, D55: a sync of the log failing in the
/// job stops the database taking changes, as any failed sync of the log
/// does: the job's error kept and told, the failure kept
/// (`Database::log_failure`), a change refused (SQLCODE -915) naming it;
/// reads go on.
#[test]
fn a_failed_sync_in_the_job_stops_changes() {
    let (db, log) = watched(JobPolicy::Manual);
    let kept = Kept::default();
    db.set_diagnostics(Box::new(kept.clone()));
    unwaited(&db, 10_i32);
    log.fail_syncs.store(true, Ordering::SeqCst);
    assert_eq!(db.run_due().err().map(|e| e.code), Some(Code::Io));
    log.fail_syncs.store(false, Ordering::SeqCst);
    assert_eq!(job(&db).last_error.map(|e| e.code), Some(Code::Io));
    let told: Vec<String> = kept.taken().into_iter().map(|(_, e)| e).collect();
    assert!(told.iter().any(|e| e.contains(LOG_SYNC) && e.contains("Failed")), "{told:?}");
    assert_eq!(db.log_failure().map(|e| e.code), Some(Code::Io));
    let s = db.session("APP");
    let refused = s.run("INSERT INTO T (ID) VALUES (11)").unwrap_err();
    assert_eq!(refused.code, Code::Io);
    assert!(refused.msg.contains("opened again"), "{refused}");
    s.rollback().unwrap();
    assert_eq!(count(&s, "SELECT ID FROM T WHERE ID = 10"), 1_usize, "reads go on");
}

/// Verifies: [DESIGN] RULES.md D36, D25: LOG.SYNC_AFTER_MS is a second by
/// default; 0 asks for no job, and a commit left unsynced then waits for
/// another sync: setting it again, a definition logged and synced, is
/// one. Set to 0 while the job is due, its round syncs nothing. A sync for
/// another reason (a commit that waits) makes its round sync nothing; a
/// log set in place of the one its commits were left in is the one it
/// syncs.
#[test]
fn the_setting_governs_the_job() {
    let (db, log) = watched(JobPolicy::Manual);
    assert_eq!(db.setting("LOG.SYNC_AFTER_MS").unwrap().to_string(), "1000");
    db.set_setting("LOG.SYNC_AFTER_MS", &number(0_i32)).unwrap();
    unwaited(&db, 10_i32);
    assert_eq!(job(&db).state, JobState::Idle, "none asked for");
    let commit = last(&log);
    db.reset_setting("LOG.SYNC_AFTER_MS").unwrap();
    assert!(log.synced.lock().unwrap().last() > Some(&commit), "synced through it by the setting's definition");
    unwaited(&db, 11_i32);
    assert_eq!(job(&db).state, JobState::Due, "asked for");
    db.set_setting("LOG.SYNC_AFTER_MS", &number(0_i32)).unwrap();
    let synced = log.synced.lock().unwrap().len();
    db.run_due().unwrap();
    assert_eq!(log.synced.lock().unwrap().len(), synced, "its round, under 0, syncs nothing");
    db.reset_setting("LOG.SYNC_AFTER_MS").unwrap();
    unwaited(&db, 12_i32);
    let s = db.session("APP");
    s.run("INSERT INTO T (ID) VALUES (13)").unwrap();
    s.commit().unwrap();
    let synced = log.synced.lock().unwrap().len();
    db.run_due().unwrap();
    assert_eq!(log.synced.lock().unwrap().len(), synced, "the commit that waited synced it");
    unwaited(&db, 14_i32);
    let other = Arc::new(Watched::default());
    db.set_log(Arc::clone(&other) as Arc<dyn Log>).unwrap();
    unwaited(&db, 15_i32);
    db.run_due().unwrap();
    assert_eq!(log.synced.lock().unwrap().len(), synced, "the log set aside, not synced");
    assert_eq!(*other.synced.lock().unwrap(), vec![last(&other)], "the log set in its place");
}

//! The thread manager (RULES.md D55): a database's background work run as
//! JOBS.POLICY says, each job shown with its state, its threads told as
//! events, and stopped and started again around a close.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use turborel::db::{Database, OpenOptions};
use turborel::error::{Code, SqlError};
use turborel::threads::{BackgroundJob, JobInfo, JobPolicy, JobState};
use turborel::value::{Decimal, Value};

use crate::common::{Kept, Scratch, after_crash};
use crate::faults::{FaultyFs, Rule};

/// The job labelled `label` of `db`.
fn job(db: &Database, label: &str) -> JobInfo {
    db.jobs().into_iter().find(|t| t.label == label).unwrap()
}

/// Schema A, with a column store table C of 3000 rows, committed, so its
/// rows have maintenance due: freezing.
fn written(db: &Database) {
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE C (K INTEGER) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)").unwrap();
    let a = db.session("A");
    let values: Vec<String> = (0..3000).map(|k| format!("({k})")).collect();
    a.run(&format!("INSERT INTO C VALUES {}", values.join(", "))).unwrap();
    a.commit().unwrap();
}

/// Verifies: [DESIGN] RULES.md D55, D36: under JOBS.POLICY MANUAL a
/// job asked for is due, no thread started for it, until the program runs
/// it (`Database::run_due`); its runs are counted, and no event tells of a
/// thread. JOBS.POLICY takes only its words.
#[test]
fn manual_work_waits_for_the_program() {
    let db = Database::new();
    let kept = Kept::default();
    db.set_diagnostics(Box::new(kept.clone())).unwrap();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    assert_eq!(db.setting("JOBS.POLICY").unwrap().to_string(), "MANUAL");
    written(&db);
    let due = job(&db, "turborel-maintenance");
    assert_eq!((due.state, due.threaded, due.runs), (JobState::Due, false, 0));
    db.run_due().unwrap();
    let done = job(&db, "turborel-maintenance");
    assert_eq!((done.state, done.threaded, done.last_error.is_none()), (JobState::Idle, false, true));
    assert!(done.runs >= 1, "{done:?}");
    assert!(kept.taken().iter().all(|(_, e)| !e.contains("Thread")), "no thread");
    let refused = db.set_setting("JOBS.POLICY", &Value::Char("SOMETIMES".into()));
    assert_eq!(refused.err().map(|e| e.code), Some(Code::Api));
}

/// Verifies: [DESIGN] RULES.md D55: under AUTOMATIC, the default, a job
/// asked for starts its thread, told as an event; PAUSED stops and joins
/// it, told too, and work asked for then waits, due, with no thread, until
/// AUTOMATIC starts one again.
#[test]
fn automatic_work_runs_on_threads_paused_waits() {
    let db = Database::new();
    let kept = Kept::default();
    db.set_diagnostics(Box::new(kept.clone())).unwrap();
    written(&db);
    assert!(job(&db, "turborel-maintenance").threaded, "started as it came due");
    db.set_job_policy(JobPolicy::Paused).unwrap();
    assert!(!job(&db, "turborel-maintenance").threaded, "stopped");
    let told: Vec<String> = kept.taken().into_iter().map(|(_, e)| e).filter(|e| e.contains("Thread")).collect();
    assert_eq!(told.len(), 2, "{told:?}");
    assert!(told[0].contains("Started") && told[1].contains("Stopped"), "{told:?}");
    let a = db.session("A");
    a.run("INSERT INTO C VALUES (1)").unwrap();
    a.commit().unwrap();
    let paused = job(&db, "turborel-maintenance");
    assert_eq!((paused.state, paused.threaded), (JobState::Due, false));
    db.set_job_policy(JobPolicy::Automatic).unwrap();
    assert!(job(&db, "turborel-maintenance").threaded, "started again for the work due");
}

/// Verifies: [DESIGN] RULES.md D55, D25: a close that fails, a
/// transaction open, stops no checkpoint for good: its jobs start again,
/// and a checkpoint coming due after is taken by its thread.
#[test]
fn a_failed_close_leaves_checkpoints_running() {
    let scratch = Scratch::new("threads-close");
    let db = Database::new();
    db.open_files(&after_crash(), &scratch.path("db"), &OpenOptions::default()).unwrap();
    db.set_setting("CHECKPOINT.AFTER_CHANGES", &Value::Exact(Decimal::new(1, 0))).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1)").unwrap();
    assert_eq!(db.close().err().map(|e| e.code), Some(Code::Api), "a transaction is open");
    let before = db.checkpoints_taken();
    a.commit().unwrap();
    let began = Instant::now();
    while db.checkpoints_taken() == before && began.elapsed() < Duration::from_secs(30) {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(db.checkpoints_taken() > before, "the checkpoint thread took it");
    db.close().unwrap();
}

/// Verifies: [DESIGN] RULES.md D55, D37, D36: INFORMATION_SCHEMA.JOBS
/// shows the database's owner each job of its background work, with its
/// state, whether it has a thread, its runs and panics, and its last
/// error; any other sees none. The owner pauses it in SQL, as the
/// Operator's Guide shows (`SET DATABASE JOBS.POLICY = 'PAUSED'`).
#[test]
fn threads_are_shown_to_the_owner() {
    let db = Database::new();
    db.create_user("OWNER", b"x").unwrap();
    db.set_owner("OWNER").unwrap();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    written(&db);
    let shown = "SELECT TRIM(LABEL), TRIM(STATE), TRIM(THREADED), RUNS, RESTARTS, LAST_ERROR FROM INFORMATION_SCHEMA.JOBS ORDER BY 1";
    let owner = db.session("OWNER");
    let expected = |state: &str, runs: u32| {
        Ok(vec![
            "turborel-checkpoint,IDLE,NO,0,0,NULL".to_owned(),
            "turborel-log-sync,IDLE,NO,0,0,NULL".to_owned(),
            format!("turborel-maintenance,{state},NO,{runs},0,NULL"),
            "turborel-read-ahead,IDLE,NO,0,0,NULL".to_owned(),
        ])
    };
    assert_eq!(crate::common::rows(&owner, shown), expected("DUE", 0));
    db.run_due().unwrap();
    let runs = job(&db, "turborel-maintenance").runs;
    assert_eq!(crate::common::rows(&owner, shown), expected("IDLE", u32::try_from(runs).unwrap()));
    assert_eq!(crate::common::rows(&db.session("OTHER"), shown), Ok(vec![]));
    owner.commit().unwrap();
    owner.run("SET DATABASE JOBS.POLICY = 'PAUSED'").unwrap();
    assert_eq!(db.setting("JOBS.POLICY").unwrap().to_string(), "PAUSED");
}

/// Verifies: [DESIGN] RULES.md D55, D25: the program's last handle,
/// dropped while a job's thread is in the middle of a checkpoint, stops
/// and joins it there, so the database's directory, its lock given up as
/// the drop returns, opens again at once.
/// Regression test (R-TEST-7): the job held the database as it worked,
/// and its thread, not the program's, dropped it last, giving the lock up
/// only then, so the open was refused.
#[test]
fn the_last_handle_dropped_joins_its_jobs() {
    let scratch = Scratch::new("threads-drop");
    let dir = scratch.path("db");
    let (fs, faults) = FaultyFs::with_faults();
    let db = Database::new();
    db.open_files(&fs, &dir, &OpenOptions::default()).unwrap();
    db.set_setting("CHECKPOINT.AFTER_CHANGES", &Value::Exact(Decimal::new(1, 0))).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER)").unwrap();
    // The checkpoint's thread waits inside its write of the catalog.
    faults.inject(Rule::delay(Duration::from_millis(500)).on("catalog").times(1));
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1)").unwrap();
    a.commit().unwrap();
    let began = Instant::now();
    while faults.injected() == 0 && began.elapsed() < Duration::from_secs(30) {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(faults.injected(), 1, "the checkpoint's thread is writing");
    drop(a);
    drop(db);
    let again = Database::new();
    again.open_files(&fs, &dir, &OpenOptions::default()).unwrap();
    assert_eq!(crate::common::rows(&again.session("A"), "SELECT K FROM T"), Ok(vec!["1".to_owned()]));
    again.close().unwrap();
}

/// A plug-in's job: counts its runs, and fails while told to.
struct Counting {
    label: &'static str,
    runs: Arc<AtomicU64>,
    fail: Arc<AtomicBool>,
}

impl BackgroundJob for Counting {
    fn label(&self) -> &'static str {
        self.label
    }
    fn run(&self, _db: &Database) -> turborel::error::Result<Option<Duration>> {
        self.runs.fetch_add(1_u64, Ordering::SeqCst);
        if self.fail.load(Ordering::SeqCst) {
            return Err(SqlError::new(Code::Io, "[DESIGN] the job's work failed"));
        }
        Ok(None)
    }
}

/// A `Counting` job labelled `label`.
fn counting(label: &'static str) -> Arc<Counting> {
    Arc::new(Counting { label, runs: Arc::default(), fail: Arc::default() })
}

/// Verifies: [DESIGN] RULES.md D55: a plug-in's job added to a database
/// (`Database::add_job`) is one of its jobs, shown with its state: asked
/// for under MANUAL, it is due until the program runs it; its error is
/// noted, told as an event, and returned by `run_due`; under AUTOMATIC
/// its own thread runs it. A label not of the form `turborel-<kind>`, or
/// one already taken, is refused (SQLCODE -910).
#[test]
fn a_plugins_job_runs_as_the_policy_says() {
    let db = Database::new();
    let kept = Kept::default();
    db.set_diagnostics(Box::new(kept.clone())).unwrap();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    let work = counting("turborel-counting");
    let handle = db.add_job(Arc::clone(&work) as Arc<dyn BackgroundJob>).unwrap();
    for label in ["counting", "turborel-", "turborel-counting", "turborel-checkpoint"] {
        assert_eq!(db.add_job(counting(label)).err().map(|e| e.code), Some(Code::Api), "{label}");
    }
    assert_eq!(job(&db, "turborel-counting").state, JobState::Idle);
    db.request_job(handle, Duration::ZERO);
    assert_eq!(job(&db, "turborel-counting").state, JobState::Due);
    db.run_due().unwrap();
    assert_eq!((work.runs.load(Ordering::SeqCst), job(&db, "turborel-counting").state), (1_u64, JobState::Idle));

    work.fail.store(true, Ordering::SeqCst);
    db.request_job(handle, Duration::ZERO);
    assert_eq!(db.run_due().err().map(|e| e.code), Some(Code::Io));
    assert!(job(&db, "turborel-counting").last_error.is_some());
    assert!(kept.taken().iter().any(|(_, e)| e.contains("turborel-counting") && e.contains("Failed")));

    work.fail.store(false, Ordering::SeqCst);
    db.set_job_policy(JobPolicy::Automatic).unwrap();
    db.request_job(handle, Duration::ZERO);
    let began = Instant::now();
    while work.runs.load(Ordering::SeqCst) < 3_u64 && began.elapsed() < Duration::from_secs(30_u64) {
        std::thread::sleep(Duration::from_millis(1_u64));
    }
    assert_eq!(work.runs.load(Ordering::SeqCst), 3_u64, "run by its thread");
    assert!(job(&db, "turborel-counting").threaded);
}

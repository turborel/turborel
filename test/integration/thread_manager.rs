//! The thread manager (RULES.md D55): a database's background work run as
//! JOBS.POLICY says, each job shown with its state, its threads told as
//! events, and stopped and started again around a close.

use std::time::{Duration, Instant};

use turborel::db::{Database, OpenOptions};
use turborel::error::Code;
use turborel::threads::{JobInfo, JobPolicy, JobState};
use turborel::value::Value;

use crate::common::{Kept, Scratch, after_crash};

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
    db.set_diagnostics(Box::new(kept.clone()));
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
    db.set_diagnostics(Box::new(kept.clone()));
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
    db.set_setting("CHECKPOINT.AFTER_CHANGES", &Value::Exact(turborel::value::Decimal::new(1, 0))).unwrap();
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
        Ok(vec!["turborel-checkpoint,IDLE,NO,0,0,NULL".to_owned(), format!("turborel-maintenance,{state},NO,{runs},0,NULL")])
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

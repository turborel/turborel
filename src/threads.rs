//! \[DESIGN\] The thread manager (RULES.md D55): every thread a database
//! runs, and its background work, started, run and stopped in one place,
//! each labelled, with a state an operator sees (`Database::threads`).
//!
//! Background work is a job: a function of the database, its checkpoints
//! or its maintenance, that a session asks for as it comes due. As the
//! database's THREADS.POLICY says, a job asked for is done on a thread of
//! its own, started the first time and told as an event (AUTOMATIC); only
//! when the program asks (`Database::run_due`, MANUAL); or not until the
//! policy changes (PAUSED). A job's error is noted and told; a panic in it
//! is caught and told, and its thread goes on. Once its database is
//! closing, no job's thread starts, and each is stopped and joined; a
//! close that fails lets them start again.
//!
//! The turns jobs take (`Turn`) refuse the thread already holding them, so
//! a plug-in that calls back is refused rather than left waiting for
//! itself (RULES.md R-ERR-4).

mod turn;
mod worker;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub(crate) use turn::Turn;
use worker::Worker;

use crate::db::Database;
use crate::diagnostics::Event;
use crate::error::{Code, Result, SqlError};
use crate::sync::lock_builtin;

/// \[DESIGN\] How a database's background work runs: the setting
/// THREADS.POLICY.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadPolicy {
    /// On threads of its own, as it comes due.
    Automatic,
    /// Only when the program asks (`Database::run_due`).
    Manual,
    /// Not at all, what comes due kept until the policy changes.
    Paused,
}

impl ThreadPolicy {
    /// The words THREADS.POLICY takes, in the order of the policies.
    pub const WORDS: [&'static str; 3] = ["AUTOMATIC", "MANUAL", "PAUSED"];

    /// The policy `word` names, AUTOMATIC for any other.
    pub(crate) fn of(word: &str) -> ThreadPolicy {
        if word.eq_ignore_ascii_case("MANUAL") {
            ThreadPolicy::Manual
        } else if word.eq_ignore_ascii_case("PAUSED") {
            ThreadPolicy::Paused
        } else {
            ThreadPolicy::Automatic
        }
    }

    /// The word THREADS.POLICY takes for it.
    pub fn word(self) -> &'static str {
        match self {
            ThreadPolicy::Automatic => "AUTOMATIC",
            ThreadPolicy::Manual => "MANUAL",
            ThreadPolicy::Paused => "PAUSED",
        }
    }
}

/// \[DESIGN\] What a job is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadState {
    /// Nothing asked of it.
    Idle,
    /// Asked for, not yet done.
    Due,
    /// Being done.
    Running,
}

/// \[DESIGN\] What happened to a job's thread, as an event tells.
#[derive(Debug, Clone)]
pub enum ThreadChange {
    /// Its thread started.
    Started,
    /// Its thread was stopped and joined.
    Stopped,
    /// Its work failed, with this error: it is asked for again as it comes
    /// due.
    Failed(SqlError),
    /// Its work panicked; its thread goes on.
    Restarted,
}

/// \[DESIGN\] A job, as the manager shows it.
#[derive(Debug, Clone)]
pub struct ThreadInfo {
    /// Its label: `turborel-<kind>`.
    pub label: &'static str,
    /// What it is doing.
    pub state: ThreadState,
    /// Whether a thread of its own runs it now.
    pub threaded: bool,
    /// The times its work was done.
    pub runs: u64,
    /// The times its work panicked.
    pub restarts: u64,
    /// The error of its last work, if it failed.
    pub last_error: Option<SqlError>,
}

/// The work of a job: done for the database, what it says of when to do it
/// again unasked: at once for none left undone, after a time, or not.
pub(crate) type Work = fn(&Database) -> Result<Option<Duration>>;

/// A job's place among its database's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct JobId(usize);

/// A job: its label, its work, how long after a failure its thread tries
/// again unasked, if at all, its thread, and what it did.
struct Job {
    label: &'static str,
    work: Work,
    retry: Option<Duration>,
    worker: Worker,
    record: Mutex<Record>,
}

/// What a job did, and whether it is asked for.
#[derive(Default)]
struct Record {
    due: bool,
    running: bool,
    runs: u64,
    restarts: u64,
    last_error: Option<SqlError>,
}

/// \[DESIGN\] A database's jobs: those of its own, and whether it is
/// closing.
#[derive(Default)]
pub(crate) struct Jobs {
    jobs: Mutex<Vec<Arc<Job>>>,
    closing: AtomicBool,
}

impl Jobs {
    /// The job labelled `label`, doing `work`, tried again `retry` after a
    /// failure if given: added, or the one of that label already added.
    pub(crate) fn add(&self, label: &'static str, work: Work, retry: Option<Duration>) -> JobId {
        let mut jobs = lock_builtin(&self.jobs);
        if let Some(at) = jobs.iter().position(|j| j.label == label) {
            return JobId(at);
        }
        let record = Mutex::new(Record::default());
        jobs.push(Arc::new(Job { label, work, retry, worker: Worker::default(), record }));
        JobId(jobs.len().saturating_sub(1))
    }

    fn job(&self, id: JobId) -> Option<Arc<Job>> {
        lock_builtin(&self.jobs).get(id.0).map(Arc::clone)
    }

    fn all(&self) -> Vec<Arc<Job>> {
        lock_builtin(&self.jobs).clone()
    }

    /// Asks for job `id` of `db`, which runs it as `policy` says.
    pub(crate) fn request(&self, id: JobId, db: &Database, policy: ThreadPolicy) {
        let Some(job) = self.job(id) else { return };
        lock_builtin(&job.record).due = true;
        if policy == ThreadPolicy::Automatic {
            self.threaded(&job, db);
        }
    }

    /// Wakes `job`'s thread, started if it has none, unless `db` is
    /// closing.
    fn threaded(&self, job: &Arc<Job>, db: &Database) {
        if self.closing.load(Ordering::Acquire) {
            return;
        }
        match job.worker.start(job.label, db.downgrade(), Arc::clone(job)) {
            Ok(true) => db.emit(&Event::Thread { label: job.label, change: ThreadChange::Started }),
            Ok(false) => {}
            Err(e) => db.emit(&Event::Thread { label: job.label, change: ThreadChange::Failed(e) }),
        }
        job.worker.request();
    }

    /// Does every job of `db` due, here, until none is.
    ///
    /// # Errors
    ///
    /// The first error of a job's work; the others are noted and told.
    pub(crate) fn run_due(&self, db: &Database) -> Result<()> {
        let mut failed = None;
        for job in self.all() {
            while lock_builtin(&job.record).due {
                if let Err(e) = job.run(db) {
                    failed = failed.or(Some(e));
                    break;
                }
            }
        }
        failed.map_or(Ok(()), Err)
    }

    /// Takes `policy` for `db`: a thread for each job due if AUTOMATIC;
    /// else every thread stopped and joined.
    pub(crate) fn apply(&self, db: &Database, policy: ThreadPolicy) {
        for job in self.all() {
            match policy {
                ThreadPolicy::Automatic => {
                    if lock_builtin(&job.record).due {
                        self.threaded(&job, db);
                    }
                }
                ThreadPolicy::Manual | ThreadPolicy::Paused => job.stop(db),
            }
        }
    }

    /// Stops and joins every thread of `db`, which is closing: none starts
    /// again until `reopened`.
    pub(crate) fn closing(&self, db: &Database) {
        self.closing.store(true, Ordering::Release);
        self.all().iter().for_each(|job| job.stop(db));
    }

    /// Lets `db`'s jobs start again, as `policy` says: its close failed.
    pub(crate) fn reopened(&self, db: &Database, policy: ThreadPolicy) {
        self.closing.store(false, Ordering::Release);
        self.apply(db, policy);
    }

    /// Each job, as the manager shows it.
    pub(crate) fn infos(&self) -> Vec<ThreadInfo> {
        self.all().iter().map(|job| job.info()).collect()
    }
}

impl Job {
    /// Does the work once, for `db`, if it is due: when to do it again
    /// unasked. Its error, or a panic in it, is kept and told.
    ///
    /// # Errors
    ///
    /// The work's error; SQLCODE -910 if it panicked.
    fn run(&self, db: &Database) -> Result<Option<Duration>> {
        {
            let mut record = lock_builtin(&self.record);
            record.due = false;
            record.running = true;
        }
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (self.work)(db)));
        let panicked = outcome.is_err();
        let done = outcome.unwrap_or_else(|_| {
            Err(SqlError::new(Code::Api, format!("[DESIGN] the work of {} panicked, and is asked for again as it comes due", self.label)))
        });
        {
            let mut record = lock_builtin(&self.record);
            record.running = false;
            record.runs = record.runs.saturating_add(1);
            record.restarts = record.restarts.saturating_add(u64::from(panicked));
            record.last_error = done.as_ref().err().cloned();
            record.due |= done.as_ref().is_ok_and(|again| again.is_some_and(|a| a.is_zero()));
        }
        // Told with no lock held: the sink is a plug-in (RULES.md R-ERR-4).
        if panicked {
            db.emit(&Event::Thread { label: self.label, change: ThreadChange::Restarted });
        }
        if let Err(e) = &done {
            db.emit(&Event::Thread { label: self.label, change: ThreadChange::Failed(e.clone()) });
        }
        done
    }

    /// When its thread does its work again unasked after `done`: at once
    /// while it is due, else as the work said, or its retry after a
    /// failure.
    fn next(&self, done: &Result<Option<Duration>>) -> Option<Duration> {
        if lock_builtin(&self.record).due {
            return Some(Duration::ZERO);
        }
        match done {
            Ok(again) => *again,
            Err(_) => self.retry,
        }
    }

    /// Stops and joins its thread, if it has one, telling `db`.
    fn stop(&self, db: &Database) {
        if self.worker.stop() {
            db.emit(&Event::Thread { label: self.label, change: ThreadChange::Stopped });
        }
    }

    fn info(&self) -> ThreadInfo {
        let record = lock_builtin(&self.record);
        let state = if record.running {
            ThreadState::Running
        } else if record.due {
            ThreadState::Due
        } else {
            ThreadState::Idle
        };
        ThreadInfo {
            label: self.label,
            state,
            threaded: self.worker.running(),
            runs: record.runs,
            restarts: record.restarts,
            last_error: record.last_error.clone(),
        }
    }
}

// Unit tests: the policy's words (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D55: each policy has its word, read back
    /// whatever its case; a word of none is AUTOMATIC.
    #[test]
    fn policies_have_their_words() {
        for policy in [ThreadPolicy::Automatic, ThreadPolicy::Manual, ThreadPolicy::Paused] {
            assert_eq!(ThreadPolicy::of(policy.word()), policy);
            assert_eq!(ThreadPolicy::of(&policy.word().to_ascii_lowercase()), policy);
            assert!(ThreadPolicy::WORDS.contains(&policy.word()));
        }
        assert_eq!(ThreadPolicy::of("SOMETIMES"), ThreadPolicy::Automatic);
    }
}

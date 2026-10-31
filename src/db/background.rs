//! \[DESIGN\] A database's background work (RULES.md D55): its jobs, its
//! checkpoints (`db/checkpoint.rs`), its maintenance
//! (`db/maintenance.rs`), its log's syncs (`db/log_sync.rs`) and its
//! scans' read-ahead (`disk/buffer/ahead.rs`), run by the thread manager (`threads.rs`) as
//! JOBS.POLICY says; and how a program runs them, sees them, and sets
//! the policy.

use std::sync::Arc;
use std::time::Duration;

use super::Database;
use super::checkpoint::checkpointed;
use super::log_sync::log_synced;
use super::maintenance::maintained;
use crate::error::Result;
use crate::error::{Code, SqlError};
use crate::settings::JOBS_POLICY;
use crate::threads::{BackgroundJob, JobAsker, JobHandle, JobId, JobInfo, JobPolicy, Jobs, Turn, Work};
use crate::value::Value;

/// \[DESIGN\] What every job's label begins with (RULES.md D55).
pub const LABEL_PREFIX: &str = "turborel-";

/// The label of a database's checkpoint job and thread.
const CHECKPOINT: &str = "turborel-checkpoint";
/// The label of a database's maintenance job and thread.
const MAINTENANCE: &str = "turborel-maintenance";
/// The label of a database's job and thread syncing its log at a pace of
/// its own (LOG.SYNC_AFTER_MS).
const LOG_SYNC: &str = "turborel-log-sync";
/// The label of a database's job and thread reading ahead of its scans
/// (RULES.md D30).
const READ_AHEAD: &str = "turborel-read-ahead";
/// \[DESIGN\] How long after a failure, or a round that did no work while
/// some is still due, maintenance looks again unasked: a second, as a
/// freeze may be waiting out COLUMNSTORE.FREEZE_DELAY.
pub(super) const AGAIN: Duration = Duration::from_secs(1);

/// A database's jobs, its snapshots' (`State::jobs`), and the turn its
/// maintenance takes, so its thread and `Database::maintain` take turns.
pub(super) struct Background {
    jobs: Arc<Jobs>,
    checkpoint: JobId,
    maintenance: JobId,
    log_sync: JobId,
    read_ahead: JobId,
    pub(super) maintenance_turn: Turn,
}

impl Background {
    /// The background work of a database whose snapshots share `jobs`:
    /// its checkpoint, maintenance, log sync and read-ahead jobs added.
    pub(super) fn over(jobs: Arc<Jobs>) -> Background {
        let checkpoint = jobs.add(CHECKPOINT, Work::Own(checkpointed), None);
        let maintenance = jobs.add(MAINTENANCE, Work::Own(maintained), Some(AGAIN));
        let log_sync = jobs.add(LOG_SYNC, Work::Own(log_synced), None);
        let read_ahead = jobs.add(READ_AHEAD, Work::Own(read_queued), None);
        Background { jobs, checkpoint, maintenance, log_sync, read_ahead, maintenance_turn: Turn::default() }
    }
}

/// The read-ahead job's work: the pages the database's scans asked for,
/// read into their pools (`AheadReader::read_queued`).
fn read_queued(db: &Database) -> Result<Option<Duration>> {
    db.snapshot()?.ahead.read_queued();
    Ok(None)
}

impl Database {
    /// \[DESIGN\] Does now the background work due, its checkpoints,
    /// maintenance, syncs of the log and read-ahead, as JOBS.POLICY MANUAL leaves it to the program to
    /// do: each job until it is due no more. Under the other policies too,
    /// taking turns with their threads.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the database is closed; the first error of a job's
    /// work, the others told as events.
    pub fn run_due(&self) -> Result<()> {
        self.snapshot()?;
        self.background().jobs.run_due(self)
    }

    /// \[DESIGN\] Each job of background work, with its state: its label,
    /// whether it is due or running, whether a thread of its own runs it,
    /// how often it ran, and its last error.
    pub fn jobs(&self) -> Vec<JobInfo> {
        self.background().jobs.infos()
    }

    /// \[DESIGN\] Adds `job`, a plug-in's background work, to the
    /// database's jobs (RULES.md D55): run as JOBS.POLICY says once asked
    /// for (`request_job`), and shown among them.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the database is closed, or the label does not
    /// begin with `turborel-`, or is one of its jobs' already.
    pub fn add_job(&self, job: Arc<dyn BackgroundJob>) -> Result<JobHandle> {
        self.snapshot()?;
        let label = job.label();
        let jobs = &self.background().jobs;
        if !label.starts_with(LABEL_PREFIX) || label.len() == LABEL_PREFIX.len() || jobs.has(label) {
            return Err(SqlError::new(
                Code::Api,
                format!("[DESIGN] RULES.md D55: a job is labelled turborel-<kind>, unlike any other of its database's, not {label}"),
            ));
        }
        Ok(JobHandle(jobs.add(label, Work::Plugin(job), None)))
    }

    /// \[DESIGN\] Asks for the plug-in's job `job`, `after` from now: its
    /// thread runs it then, under AUTOMATIC; under MANUAL, the program's
    /// next `run_due`; under PAUSED, once the policy changes.
    pub fn request_job(&self, job: JobHandle, after: Duration) {
        self.background().jobs.request_after(job.0, self, self.job_policy(), after);
    }

    /// \[DESIGN\] Sets JOBS.POLICY to `policy`, as `set_setting` does:
    /// taken at once, threads started or stopped as it says.
    ///
    /// # Errors
    ///
    /// As `set_setting`'s.
    pub fn set_job_policy(&self, policy: JobPolicy) -> Result<()> {
        self.set_setting("JOBS.POLICY", &Value::Char(policy.word().to_owned()))
    }

    /// JOBS.POLICY, as the database's settings now say; AUTOMATIC once
    /// it is closed.
    pub(super) fn job_policy(&self) -> JobPolicy {
        self.snapshot().map_or(JobPolicy::Automatic, |st| JobPolicy::of(st.settings.word(JOBS_POLICY)))
    }

    /// Takes JOBS.POLICY as it now is: a thread for each job due, or
    /// every thread stopped.
    pub(super) fn apply_job_policy(&self) {
        self.background().jobs.apply(self, self.job_policy());
    }

    /// Has its scans' read-ahead read by its read-ahead job, asked for
    /// through the job seam (`request_job`) by its pools (RULES.md D30).
    pub(super) fn serve_read_ahead(&self) {
        if let Ok(st) = self.snapshot() {
            st.ahead.use_job(JobAsker::new(self.downgrade(), JobHandle(self.background().read_ahead)));
        }
    }

    /// Asks for the checkpoint job.
    pub(super) fn request_checkpoint(&self) {
        self.background().jobs.request(self.background().checkpoint, self, self.job_policy());
    }

    /// Asks for the maintenance job.
    pub(super) fn request_maintenance(&self) {
        self.background().jobs.request(self.background().maintenance, self, self.job_policy());
    }

    /// Asks for the log sync job, `after` from now.
    pub(super) fn request_log_sync(&self, after: Duration) {
        self.background().jobs.request_after(self.background().log_sync, self, self.job_policy(), after);
    }

    /// Stops and joins every job's thread, the database closing: none
    /// starts until `background_reopened`.
    pub(super) fn background_closing(&self) {
        self.background().jobs.closing(self);
    }

    /// Lets the jobs start again, as JOBS.POLICY says: a close failed.
    pub(super) fn background_reopened(&self) {
        self.background().jobs.reopened(self, self.job_policy());
    }
}

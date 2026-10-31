//! \[EXTENSION\] The driver as a job of the thread manager (RULES.md D55,
//! D57): added to the replica's database (`Database::add_job`), each run a
//! turn of the driver for the database it is given, run again a tick
//! later; a transport asks for it as frames arrive
//! (`Database::request_job`). Labelled `turborel-replica`, it is shown
//! among the database's jobs, INFORMATION_SCHEMA.JOBS among them.

use std::sync::Arc;
use std::time::Duration;

use turborel::db::Database;
use turborel::error::Result;
use turborel::threads::BackgroundJob;

use super::Driver;

/// \[EXTENSION\] The label of a replica's driver job and thread.
pub const JOB_LABEL: &str = "turborel-replica";

/// \[EXTENSION\] A replica's driver as its database's job.
pub struct DriverJob {
    driver: Arc<Driver>,
    every: Duration,
}

impl DriverJob {
    /// The job of `driver`, run again `every` after each turn: its
    /// consensus's tick.
    pub fn new(driver: Arc<Driver>, every: Duration) -> DriverJob {
        DriverJob { driver, every }
    }
}

impl BackgroundJob for DriverJob {
    fn label(&self) -> &'static str {
        JOB_LABEL
    }

    fn run(&self, db: &Database) -> Result<Option<Duration>> {
        self.driver.turn(db)?;
        Ok(Some(self.every))
    }
}

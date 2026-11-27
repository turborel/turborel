//! \[DESIGN\] How a table's engine, which knows no database, tells its
//! database that a read made maintenance wanted (RULES.md D1212). A write
//! is told of by its transaction's end, which collects the table's
//! garbage and notes the table then (`txn/collect.rs`). A transaction
//! that read alone collects nothing, and its commit is the path of every
//! read by key, so nothing is asked there. The engine tells here instead,
//! once a table opened, as the first of its zones is asked for, in place
//! of the noting the database's opening did before. The note is set
//! and the maintenance job asked for, and the job, finding the note,
//! notes each table that has maintenance due (`db/maintenance.rs`).
//!
//! Kept with the database's work areas, which its engines are attached
//! to and its state holds, so no engine needs a seam of its own for it.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::sync::lock_builtin;
use crate::threads::JobAsker;

/// \[DESIGN\] The note that a read made maintenance wanted, and what asks
/// for the maintenance job.
#[derive(Default)]
pub struct Asks {
    asked: AtomicBool,
    job: Mutex<Option<JobAsker>>,
}

impl Asks {
    /// Has `job` asked for at each note from now on.
    pub(crate) fn use_job(&self, job: JobAsker) {
        *lock_builtin(&self.job) = Some(job);
    }

    /// Notes that a read made maintenance wanted, and asks for the job, if
    /// it has one. No lock is held, as asking may start the job's thread.
    pub fn ask(&self) {
        self.asked.store(true, Ordering::Release);
        let job = lock_builtin(&self.job).clone();
        if let Some(job) = job {
            job.ask();
        }
    }

    /// Whether a read made maintenance wanted since this was last asked.
    /// The note is forgotten from now on.
    pub(crate) fn take(&self) -> bool {
        self.asked.load(Ordering::Acquire) && self.asked.swap(false, Ordering::AcqRel)
    }
}

// Unit tests: a note is taken once (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D1212: a note that a read made
    /// maintenance wanted is taken once, none before one is made, and one
    /// made again is taken again.
    #[test]
    fn a_note_is_taken_once() {
        let asks = Asks::default();
        assert!(!asks.take(), "none made");
        asks.ask();
        assert!(asks.take() && !asks.take(), "taken once");
        asks.ask();
        assert!(asks.take(), "made again");
    }
}

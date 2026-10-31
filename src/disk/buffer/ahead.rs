//! \[DESIGN\] Read-ahead on a job (RULES.md D30, D55): the pools a
//! database's scans read through, served by its read-ahead job
//! (`turborel-read-ahead`), which reads the pages they ask for into the
//! pools while they go on, rather than in the scans' own reads.
//!
//! A scan asks through its pool (`BufferPool::ahead_of`,
//! `BufferPool::ahead_pages`) with an `Ahead` of its own: the pool queues
//! what it asks, bounded, and the reader asks the thread manager for its
//! job, which does `read_queued`, on a thread of its own or when the
//! program runs what is due, as JOBS.POLICY says. What a scan asked and
//! the job has not read when the scan ends is never read: the scan's
//! `Ahead` gone, the job passes it over.

use std::sync::{Arc, Mutex, Weak};

use super::BufferPool;
use crate::sync::lock_builtin;
use crate::threads::JobAsker;

/// \[DESIGN\] What reads ahead for a database's pools: the pools it
/// serves, and the job it asks to read what their scans queue. A pool it
/// does not serve reads ahead in the scan's own thread.
#[derive(Default)]
pub struct AheadReader {
    pools: Mutex<Vec<Weak<BufferPool>>>,
    job: Mutex<Option<JobAsker>>,
}

impl AheadReader {
    /// A reader serving no pool, asking no job: what its pools queue
    /// waits for `read_queued`.
    pub fn new() -> Arc<AheadReader> {
        Arc::default()
    }

    /// Serves `pool` from now on: its scans' read-ahead queued for this
    /// reader, rather than read in their own threads.
    pub fn serve(self: &Arc<Self>, pool: &Arc<BufferPool>) {
        pool.queued.serve(Arc::downgrade(self));
        let mut pools = lock_builtin(&self.pools);
        pools.retain(|p| p.strong_count() > 0);
        pools.push(Arc::downgrade(pool));
    }

    /// Asks `job` to read what its pools queue, from now on.
    pub(crate) fn use_job(&self, job: JobAsker) {
        *lock_builtin(&self.job) = Some(job);
    }

    /// Reads the pages queued in each pool it serves, of scans not yet
    /// ended: the read-ahead job's work. The pages read.
    pub fn read_queued(&self) -> usize {
        let pools: Vec<Arc<BufferPool>> = lock_builtin(&self.pools).iter().filter_map(Weak::upgrade).collect();
        pools.iter().fold(0_usize, |read, pool| read.saturating_add(pool.read_queued()))
    }

    /// Asks its job for the pages queued, if it has one; with no lock
    /// held, as asking may start the job's thread and tell an event.
    pub(super) fn ask(&self) {
        let job = lock_builtin(&self.job).clone();
        if let Some(job) = job {
            job.ask();
        }
    }
}

/// \[DESIGN\] A scan's read-ahead: whether it is going on, so pages it
/// asked for and the job has not read are passed over once it ends, and,
/// reading pages in order, the page past the last it asked for.
#[derive(Default)]
pub struct Ahead {
    /// Held while the scan is: its requests hold it weakly. Made when it
    /// first asks.
    pub(super) alive: Option<Arc<()>>,
    /// Reading in order, the page past those it asked for.
    pub(super) until: u64,
}

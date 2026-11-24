//! \[DESIGN\] The executor's workers (RULES.md D55, D290): one pool of a
//! database's, shared by all its statements, each thread named
//! `turborel-exec-<n>`, as many as EXECUTOR.WORKERS says. A statement
//! running in parallel offers the pool a task; each worker takes a morsel
//! of the first job that wants one, moving it behind the others, so the
//! statements running at once share the workers a morsel at a time, as
//! Leis, Boncz, Kemper and Neumann's dispatcher shares them in
//! "Morsel-Driven Parallelism" (2014). Their dispatch knows nothing of
//! sockets or memory nodes, as the core reads no system's topology
//! (RULES.md D3).
//!
//! No statement waits for a worker, as the thread that offered a task takes
//! its morsels too (`exec/parallel/run.rs`), so a task ends whether or not
//! a worker is free, and a task offered from within a morsel cannot wait
//! for itself. Workers are started as a task first wants more of them than
//! run, and one numbered past the setting ends once it has no morsel. The
//! pool's threads are told to end and joined as it drops with its
//! database, but for the one dropping it, which ends of itself.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};

use crate::error::{Code, Result, SqlError};

/// The name of a worker's thread, before its number.
const WORKER_NAME: &str = "turborel-exec";

/// \[DESIGN\] Work of morsels a pool's workers take a morsel at a time.
pub(crate) trait Task: Send + Sync {
    /// Whether it has a morsel a worker may take now.
    fn wanted(&self) -> bool;

    /// Whether it will never have one again.
    fn finished(&self) -> bool;

    /// Takes and runs one of its morsels, if one is free.
    fn step(&self);
}

/// A database's workers.
#[derive(Default)]
pub(crate) struct Pool {
    shared: Arc<Shared>,
}

/// What the workers share: the board of jobs and threads, and what wakes
/// them.
#[derive(Default)]
struct Shared {
    board: Mutex<Board>,
    wake: Condvar,
}

/// The jobs offered, and the threads.
#[derive(Default)]
struct Board {
    jobs: VecDeque<Arc<dyn Task>>,
    /// Each worker's thread, by number, while it runs.
    threads: Vec<Option<JoinHandle<()>>>,
    /// The workers wanted: a worker numbered past it ends.
    size: usize,
    closing: bool,
}

/// The board, a worker's panic having poisoned none of what it holds, as
/// a job's morsel runs outside its lock.
fn board(shared: &Shared) -> MutexGuard<'_, Board> {
    shared.board.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Pool {
    /// Offers `job` to the workers, `size` of them wanted, started if not
    /// running.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if a worker cannot be started; the job is offered to
    /// those that run, and its own thread takes its morsels.
    pub(crate) fn offer(&self, job: Arc<dyn Task>, size: usize) -> Result<()> {
        let mut board = board(&self.shared);
        if board.closing {
            return Ok(());
        }
        board.size = size;
        board.jobs.push_back(job);
        let started = self.start(&mut board);
        drop(board);
        self.shared.wake.notify_all();
        started
    }

    /// Starts the workers numbered below the size that do not run.
    fn start(&self, board: &mut Board) -> Result<()> {
        let size = board.size;
        if board.threads.len() < size {
            board.threads.resize_with(size, || None);
        }
        for (n, slot) in board.threads.iter_mut().enumerate().take(size) {
            if slot.is_some() {
                continue;
            }
            let shared = Arc::clone(&self.shared);
            let thread = thread::Builder::new()
                .name(format!("{WORKER_NAME}-{n}"))
                .spawn(move || work(&shared, n))
                .map_err(|e| SqlError::new(Code::Io, format!("[DESIGN] the worker {WORKER_NAME}-{n} could not be started: {e}")))?;
            *slot = Some(thread);
        }
        Ok(())
    }

    /// Wakes the idle workers, as a task offered may want more of them.
    pub(crate) fn nudge(&self) {
        self.shared.wake.notify_all();
    }

    /// How many workers run.
    #[cfg(test)]
    pub(crate) fn running(&self) -> usize {
        board(&self.shared).threads.iter().filter(|t| t.is_some()).count()
    }
}

impl Pool {
    /// Ends every worker after its morsel and joins it, as its database
    /// closes with no statement running (RULES.md D55); they start again
    /// as a task next wants them, if the close fails.
    pub(crate) fn stop(&self) {
        self.ended(false);
    }

    /// Tells each worker to end, closing the pool for good if `closing`,
    /// and joins each, but for the thread asking, a worker's whose morsel
    /// held the database last.
    fn ended(&self, closing: bool) {
        let threads: Vec<JoinHandle<()>> = {
            let mut board = board(&self.shared);
            board.closing = closing;
            board.size = 0;
            board.jobs.clear();
            board.threads.iter_mut().filter_map(Option::take).collect()
        };
        self.shared.wake.notify_all();
        for thread in threads.into_iter().filter(|t| t.thread().id() != thread::current().id()) {
            // A worker's panic is a morsel's, its job's statement told.
            let _ = thread.join();
        }
    }
}

/// Dropped with its database: each worker told to end after its morsel,
/// and joined.
impl Drop for Pool {
    fn drop(&mut self) {
        self.ended(true);
    }
}

/// Worker `n`'s loop: a morsel of the first job wanting one, until the
/// pool closes or wants fewer workers.
fn work(shared: &Shared, n: usize) {
    while let Some(job) = next_job(shared, n) {
        job.step();
    }
}

/// The job worker `n` takes a morsel of next, moved behind the others;
/// none once it is to end.
fn next_job(shared: &Shared, n: usize) -> Option<Arc<dyn Task>> {
    let mut board = board(shared);
    loop {
        if board.closing || n >= board.size {
            if let Some(slot) = board.threads.get_mut(n) {
                // Its handle dropped, it ends detached: it holds the board
                // alone, and nothing of a database.
                *slot = None;
            }
            return None;
        }
        board.jobs.retain(|j| !j.finished());
        if let Some(job) = board.jobs.iter().position(|j| j.wanted()).and_then(|at| board.jobs.remove(at)) {
            board.jobs.push_back(Arc::clone(&job));
            return Some(job);
        }
        board = shared.wake.wait(board).unwrap_or_else(PoisonError::into_inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A job of `left` morsels, counting those run.
    struct Counting {
        left: AtomicUsize,
        done: AtomicUsize,
    }

    impl Task for Counting {
        fn wanted(&self) -> bool {
            self.left.load(Ordering::Acquire) > 0
        }

        fn finished(&self) -> bool {
            !self.wanted()
        }

        fn step(&self) {
            let mut left = self.left.load(Ordering::Acquire);
            while left > 0 {
                match self.left.compare_exchange(left, left.saturating_sub(1), Ordering::AcqRel, Ordering::Acquire) {
                    Ok(_) => {
                        self.done.fetch_add(1, Ordering::AcqRel);
                        return;
                    }
                    Err(now) => left = now,
                }
            }
        }
    }

    /// Verifies: [DESIGN] RULES.md D290 the workers take every morsel of
    /// a task offered, each once, and end as the pool drops.
    #[test]
    fn workers_take_every_morsel() {
        let pool = Pool::default();
        let job = Arc::new(Counting { left: AtomicUsize::new(1000), done: AtomicUsize::new(0) });
        let task: Arc<dyn Task> = job.clone();
        pool.offer(task, 3).unwrap();
        while job.wanted() {
            job.step();
        }
        assert!(pool.running() <= 3);
        // A worker may have taken the last morsel and not yet counted it,
        // so the count is read once the drop has joined every worker.
        drop(pool);
        assert_eq!(job.done.load(Ordering::Acquire), 1000);
    }

    /// Verifies: [DESIGN] RULES.md D290 a pool wanting fewer workers
    /// wants the fewer, those numbered past it ending as they wake.
    #[test]
    fn fewer_workers_wanted_end() {
        let pool = Pool::default();
        let none = Arc::new(Counting { left: AtomicUsize::new(0), done: AtomicUsize::new(0) });
        let task: Arc<dyn Task> = none;
        pool.offer(Arc::clone(&task), 4).unwrap();
        pool.offer(task, 1).unwrap();
        assert_eq!(board(&pool.shared).size, 1);
    }
}

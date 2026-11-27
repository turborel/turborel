//! \[DESIGN\] The executor's workers (RULES.md D55, D290): one pool of a
//! database's, shared by all its statements, each thread named
//! `turborel-exec-<n>`, as many as EXECUTOR.WORKERS says. A statement
//! running in parallel offers the pool a task, and how many workers serve
//! it, one fewer than its degree (RULES.md D1246); each worker takes a
//! morsel of the first job it serves that wants one, moving it behind the
//! others, so the
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
use std::thread::{self, JoinHandle, ThreadId};

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

/// \[DESIGN\] (RULES.md D1246) A job offered, and the workers that serve
/// it: `most` of them, numbered on from `from` and round past the pool's
/// last. The jobs a thread offers are served by the same workers, the
/// lowest numbers of a thread offering alone, so the threads busy for a
/// statement are the same ones from one of its runs to the next, and
/// whatever places threads on processors can give each its own. Jobs of
/// threads offering at once are numbered on from one another's.
struct Offered {
    task: Arc<dyn Task>,
    /// The thread that offered it.
    owner: ThreadId,
    from: usize,
    most: usize,
}

impl Offered {
    /// Whether worker `n` of a pool of `size` serves it.
    fn serves(&self, n: usize, size: usize) -> bool {
        let from = self.from.checked_rem(size).unwrap_or(0);
        n.saturating_add(size).saturating_sub(from).checked_rem(size).is_some_and(|after| after < self.most)
    }
}

/// The jobs offered, and the threads.
#[derive(Default)]
struct Board {
    jobs: VecDeque<Offered>,
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

impl Board {
    /// The number of the first worker that serves a job `owner` offers
    /// now: that of a job it offered and still has here, else past the
    /// workers serving each other thread's jobs.
    fn first_for(&self, owner: ThreadId) -> usize {
        let mut others: Vec<(ThreadId, usize)> = Vec::new();
        for job in &self.jobs {
            if job.owner == owner {
                return job.from;
            }
            if others.iter().all(|(seen, _)| *seen != job.owner) {
                others.push((job.owner, job.most));
            }
        }
        others.iter().fold(0_usize, |sum, (_, most)| sum.saturating_add(*most)).checked_rem(self.size).unwrap_or(0)
    }
}

impl Pool {
    /// Offers `job` to `most` of the workers, `size` of them wanted in
    /// all, started if not running.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if a worker cannot be started; the job is offered to
    /// those that run, and its own thread takes its morsels.
    pub(crate) fn offer(&self, job: Arc<dyn Task>, (size, most): (usize, usize)) -> Result<()> {
        let mut board = board(&self.shared);
        if board.closing {
            return Ok(());
        }
        board.size = size;
        board.jobs.retain(|j| !j.task.finished());
        let owner = thread::current().id();
        let from = board.first_for(owner);
        board.jobs.push_back(Offered { task: job, owner, from, most });
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

/// Worker `n`'s loop: a morsel of the first job it serves wanting one,
/// until the pool closes or wants fewer workers.
fn work(shared: &Shared, n: usize) {
    while let Some(job) = next_job(shared, n) {
        job.step();
    }
}

/// The job worker `n` takes a morsel of next, of those it serves, moved
/// behind the others; none once it is to end.
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
        board.jobs.retain(|j| !j.task.finished());
        let size = board.size;
        if let Some(job) = board.jobs.iter().position(|j| j.serves(n, size) && j.task.wanted()).and_then(|at| board.jobs.remove(at)) {
            let task = Arc::clone(&job.task);
            board.jobs.push_back(job);
            return Some(task);
        }
        board = shared.wake.wait(board).unwrap_or_else(PoisonError::into_inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

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
        pool.offer(task, (3, 3)).unwrap();
        while job.wanted() {
            job.step();
        }
        assert!(pool.running() <= 3);
        // A worker may have taken the last morsel and not yet counted it,
        // so the count is read once the drop has joined every worker.
        drop(pool);
        assert_eq!(job.done.load(Ordering::Acquire), 1000);
    }

    /// A job of `left` morsels, wanted once `open`, that notes the name of
    /// each thread taking one; not finished until every morsel is taken.
    struct Named {
        open: AtomicBool,
        left: AtomicUsize,
        names: Mutex<Vec<String>>,
    }

    impl Named {
        /// A job of `left` morsels, wanted if `open`.
        fn of(left: usize, open: bool) -> Arc<Named> {
            Arc::new(Named { open: AtomicBool::new(open), left: AtomicUsize::new(left), names: Mutex::new(Vec::new()) })
        }
    }

    impl Task for Named {
        fn wanted(&self) -> bool {
            self.open.load(Ordering::Acquire) && !self.finished()
        }

        fn finished(&self) -> bool {
            self.left.load(Ordering::Acquire) == 0
        }

        fn step(&self) {
            if self.left.try_update(Ordering::AcqRel, Ordering::Acquire, |left| left.checked_sub(1)).is_ok() {
                let name = thread::current().name().unwrap_or_default().to_owned();
                self.names.lock().unwrap().push(name);
            }
        }
    }

    /// Verifies: [DESIGN] RULES.md D1246 a job is served by as many
    /// workers as it asks, numbered on from its first and round past the
    /// pool's last: of a pool of 4, a job of 2 from worker 0 by workers 0
    /// and 1, one from worker 3 by workers 3 and 0, one asking as many as
    /// the pool by all, and one asking none by none.
    #[test]
    fn a_job_is_served_by_its_workers_alone() {
        let job = |from, most| Offered {
            task: Arc::new(Counting { left: AtomicUsize::new(0), done: AtomicUsize::new(0) }),
            owner: thread::current().id(),
            from,
            most,
        };
        let serving = |from, most| (0..4).filter(|&n| job(from, most).serves(n, 4)).collect::<Vec<usize>>();
        assert_eq!(serving(0, 2), [0, 1]);
        assert_eq!(serving(3, 2), [0, 3]);
        assert_eq!(serving(1, 4), [0, 1, 2, 3]);
        assert_eq!(serving(2, 0), Vec::<usize>::new());
        // A first numbered past a pool since made smaller counts round it.
        assert_eq!(serving(6, 1), [2]);
    }

    /// Verifies: [DESIGN] RULES.md D1246 the jobs a thread offers are
    /// served by the same workers, the lowest numbers of a thread offering
    /// alone, and another thread's job offered meanwhile is numbered on
    /// past them: of a pool of 4, a thread's two jobs of 2 workers both
    /// from worker 0, another thread's from worker 2, and a third's round
    /// the pool from worker 0 again. Every morsel of a job is taken by a
    /// worker that serves it.
    #[test]
    fn a_threads_jobs_keep_their_workers() {
        let pool = Pool::default();
        // Not wanted until opened, so each stays offered while the next is.
        let (first, second) = (Named::of(200, false), Named::of(200, false));
        let offered = |job: &Arc<Named>, most: usize| {
            let task: Arc<dyn Task> = job.clone();
            pool.offer(task, (4, most)).unwrap();
        };
        let by_another = |most: usize| {
            let job = Named::of(1, false);
            thread::scope(|s| s.spawn(|| offered(&job, most)).join().unwrap());
        };
        by_another(0);
        offered(&first, 2);
        offered(&second, 2);
        by_another(3);
        by_another(1);
        let placed: Vec<(usize, usize)> = board(&pool.shared).jobs.iter().map(|j| (j.from, j.most)).collect();
        assert_eq!(
            placed,
            [(0, 0), (0, 2), (0, 2), (2, 3), (1, 1)],
            "a thread's own share workers; another's are numbered on, round the pool"
        );
        for job in [&first, &second] {
            job.open.store(true, Ordering::Release);
        }
        pool.nudge();
        while !(first.finished() && second.finished()) {
            thread::yield_now();
        }
        drop(pool);
        for job in [&first, &second] {
            let names = job.names.lock().unwrap();
            assert_eq!(names.len(), 200);
            assert!(names.iter().all(|n| n == "turborel-exec-0" || n == "turborel-exec-1"), "{names:?}");
        }
    }

    /// Verifies: [DESIGN] RULES.md D290 a pool wanting fewer workers
    /// wants the fewer, those numbered past it ending as they wake.
    #[test]
    fn fewer_workers_wanted_end() {
        let pool = Pool::default();
        let none = Arc::new(Counting { left: AtomicUsize::new(0), done: AtomicUsize::new(0) });
        let task: Arc<dyn Task> = none;
        pool.offer(Arc::clone(&task), (4, 4)).unwrap();
        pool.offer(task, (1, 1)).unwrap();
        assert_eq!(board(&pool.shared).size, 1);
    }
}

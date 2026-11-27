//! \[DESIGN\] A run of morsels (RULES.md D290): work cut into numbered
//! morsels that the database's workers and the statement's own thread take
//! at once, each the next number not taken, by one atomic step, as the
//! dispatcher of Leis et al., "Morsel-Driven Parallelism" (2014), hands
//! them out. Their results are given back in the order of their numbers,
//! so what reads them sees what one thread taking them in turn would, the
//! rows before an error, then the error, as a statement run alone raises
//! it (RULES.md D15).
//!
//! No more than `degree` threads work for a run at once (RULES.md D1246).
//! Its reader's thread is one of them whatever it does, so the workers
//! run one morsel fewer than the degree at once, and the reader takes the
//! morsels left: it takes one itself whenever it has to wait, so a run
//! ends though every worker is busy, and what it does with a result
//! between two of them it does beside one worker fewer than the degree.
//! No more than `window` morsels are taken past the one the reader waits
//! for, so a reader that stops early, as a cursor closed or a FETCH FIRST
//! met does, leaves little read for nothing. Each morsel looks at the statement's
//! cancellation first (RULES.md D44), and after one fails none past it is
//! taken. A panic in a morsel is its failure, so the reader never waits
//! for a morsel that will not end.

use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

use crate::db::State;
use crate::error::{Code, Result, SqlError};
use crate::noted;
use crate::threads::{Pool, Task};
use crate::txn::{Stop, TxId};

/// \[DESIGN\] The morsels a run takes past the one its reader waits for, by
/// thread it runs on: enough that no worker waits while the reader takes
/// one run's results, few enough that a reader stopping early wastes few.
const AHEAD_PER_THREAD: usize = 2;

/// The morsels a run of `degree` threads takes at most past the one its
/// reader waits for, that one among them.
pub(super) fn window(degree: usize) -> usize {
    degree.max(1).saturating_mul(AHEAD_PER_THREAD)
}

/// The work of each morsel, by its number.
pub(in crate::exec) type Work<T> = Box<dyn Fn(usize) -> Result<T> + Send + Sync>;

/// A run of morsels, shared by the workers taking them and its reader.
struct Run<T> {
    work: Work<T>,
    count: usize,
    degree: usize,
    window: usize,
    stop: Option<Arc<Stop>>,
    /// The number of the next morsel to take.
    taken: AtomicUsize,
    /// The morsels the pool's workers run now, the reader's own not
    /// counted.
    running: AtomicUsize,
    /// The number of the next morsel the reader is given.
    given: AtomicUsize,
    /// The least number of a morsel that failed, none past it taken.
    failed: AtomicUsize,
    /// Set once its reader is gone, after which no morsel is taken.
    ended: AtomicBool,
    done: Mutex<BTreeMap<usize, Result<T>>>,
    landed: Condvar,
    pool: Arc<Pool>,
}

/// What a run's reader holds: the run's results, in order. Dropped, the
/// run takes no more morsels.
pub(in crate::exec) struct Morsels<T: Send + 'static> {
    run: Arc<Run<T>>,
}

/// The results done, a panic in a morsel having poisoned none, as a
/// morsel's work runs outside the lock.
fn done_of<T>(run: &Run<T>) -> MutexGuard<'_, BTreeMap<usize, Result<T>>> {
    run.done.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Adds one to `count` if it is below `limit`: what it was.
fn below(count: &AtomicUsize, limit: usize) -> Option<usize> {
    let mut seen = count.load(Ordering::Acquire);
    loop {
        if seen >= limit {
            return None;
        }
        match count.compare_exchange_weak(seen, seen.saturating_add(1), Ordering::AcqRel, Ordering::Acquire) {
            Ok(was) => return Some(was),
            Err(now) => seen = now,
        }
    }
}

impl<T: Send + 'static> Run<T> {
    /// The number of a morsel it may take now: the next not taken, within
    /// the window and not past one that failed.
    fn claim(&self) -> Option<usize> {
        if self.ended.load(Ordering::Acquire) {
            return None;
        }
        let ahead = self.given.load(Ordering::Acquire).saturating_add(self.window);
        let limit = self.count.min(ahead).min(self.failed.load(Ordering::Acquire).saturating_add(1));
        below(&self.taken, limit)
    }

    /// \[DESIGN\] (RULES.md D1246) The morsels the pool's workers run at
    /// once at most: one fewer than the degree. The reader's thread is the
    /// last of the degree's whatever it does, a morsel as it waits or its
    /// own work on the results between two of them, so no more threads
    /// than the degree are busy for the run.
    fn workers(&self) -> usize {
        self.degree.saturating_sub(1)
    }

    /// Runs morsel `m`, claimed, and keeps its result.
    fn run(&self, m: usize) {
        let checked = self.stop.as_ref().map_or(Ok(()), |s| s.check_now());
        let result = checked.and_then(|()| {
            catch_unwind(AssertUnwindSafe(|| (self.work)(m)))
                .unwrap_or_else(|_| Err(SqlError::new(Code::Api, "[DESIGN] a morsel of a parallel query failed")))
        });
        if result.is_err() {
            self.failed.fetch_min(m, Ordering::AcqRel);
        }
        done_of(self).insert(m, result);
        self.landed.notify_all();
    }

    /// The result of the next morsel in order, taking morsels itself while
    /// it waits; none past the last, or the one that failed.
    fn next(&self) -> Option<Result<T>> {
        let want = self.given.load(Ordering::Acquire);
        if want >= self.count || want > self.failed.load(Ordering::Acquire) {
            return None;
        }
        loop {
            if let Some(result) = done_of(self).remove(&want) {
                self.given.store(want.saturating_add(1), Ordering::Release);
                // The window moved on: idle workers may take more.
                self.pool.nudge();
                return Some(result);
            }
            if let Some(m) = self.claim() {
                self.run(m);
                continue;
            }
            let done = done_of(self);
            if !done.contains_key(&want) {
                // Woken as any morsel lands; `want` is running, as no
                // morsel could be claimed.
                drop(self.landed.wait(done).unwrap_or_else(PoisonError::into_inner));
            }
        }
    }
}

impl<T: Send + 'static> Task for Run<T> {
    fn wanted(&self) -> bool {
        let ahead = self.given.load(Ordering::Acquire).saturating_add(self.window);
        let limit = self.count.min(ahead).min(self.failed.load(Ordering::Acquire).saturating_add(1));
        !self.ended.load(Ordering::Acquire)
            && self.running.load(Ordering::Acquire) < self.workers()
            && self.taken.load(Ordering::Acquire) < limit
    }

    fn finished(&self) -> bool {
        let limit = self.count.min(self.failed.load(Ordering::Acquire).saturating_add(1));
        self.ended.load(Ordering::Acquire) || self.taken.load(Ordering::Acquire) >= limit
    }

    fn step(&self) {
        if below(&self.running, self.workers()).is_none() {
            return;
        }
        if let Some(m) = self.claim() {
            self.run(m);
        }
        self.running.fetch_sub(1, Ordering::AcqRel);
    }
}

impl<T: Send + 'static> Morsels<T> {
    /// A run of `count` morsels, each `work` of its number, on up to
    /// `degree` threads, the statement's own and one fewer of `db`'s
    /// workers, for the statement of `tx`.
    pub(in crate::exec) fn start(db: &State, tx: TxId, (count, degree): (usize, usize), work: Work<T>) -> Morsels<T> {
        let degree = degree.max(1);
        // [EXTENSION] the notes of the operator that starts it, each
        // worker's work noted of it (RULES.md D620).
        let work: Work<T> = match noted::noting() {
            Some(notes) => Box::new(move |m| noted::within(Some(Arc::clone(&notes)), || work(m))),
            None => work,
        };
        let run = Arc::new(Run {
            work,
            count,
            degree,
            window: window(degree),
            stop: db.tx.stop_of(tx),
            taken: AtomicUsize::new(0),
            running: AtomicUsize::new(0),
            given: AtomicUsize::new(0),
            failed: AtomicUsize::new(usize::MAX),
            ended: AtomicBool::new(false),
            done: Mutex::new(BTreeMap::new()),
            landed: Condvar::new(),
            pool: Arc::clone(db.jobs.workers()),
        });
        if degree > 1 && count > 1 {
            let workers = crate::physical::workers_of(db);
            // A worker not started leaves its morsels to the others and to
            // the statement's own thread, which takes them as it waits.
            let task: Arc<dyn Task> = run.clone();
            let _ = db.jobs.workers().offer(task, (workers, run.workers()));
        }
        Morsels { run }
    }
}

impl<T: Send + 'static> Iterator for Morsels<T> {
    type Item = Result<T>;

    fn next(&mut self) -> Option<Result<T>> {
        self.run.next()
    }
}

impl<T: Send + 'static> Drop for Morsels<T> {
    fn drop(&mut self) {
        self.run.ended.store(true, Ordering::Release);
        // Woken, the workers let go of the run, and of what its work holds.
        self.run.pool.nudge();
    }
}

// Unit tests: who takes a run's morsels (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::exec::Db;
    use crate::txn::NO_TRANSACTION;

    /// A gate morsels wait at: how many wait, and whether it is open.
    #[derive(Default)]
    struct Gate {
        state: Mutex<(usize, bool)>,
        moved: Condvar,
    }

    impl Gate {
        /// Waits at the gate until it opens.
        fn pass(&self) {
            let mut state = self.state.lock().unwrap();
            state.0 = state.0.saturating_add(1);
            self.moved.notify_all();
            while !state.1 {
                state = self.moved.wait(state).unwrap();
            }
        }

        /// Waits until `n` morsels wait at the gate.
        fn waiting(&self, n: usize) {
            let mut state = self.state.lock().unwrap();
            while state.0 < n {
                state = self.moved.wait(state).unwrap();
            }
        }

        /// Opens the gate.
        fn open(&self) {
            self.state.lock().unwrap().1 = true;
            self.moved.notify_all();
        }
    }

    /// A run of `count` morsels of `degree`, each waiting at `gate`, with
    /// no worker of a pool offered it.
    fn gated(count: usize, degree: usize, gate: &Arc<Gate>) -> Run<usize> {
        let at = Arc::clone(gate);
        Run {
            work: Box::new(move |m| {
                at.pass();
                Ok(m)
            }),
            count,
            degree,
            window: window(degree),
            stop: None,
            taken: AtomicUsize::new(0),
            running: AtomicUsize::new(0),
            given: AtomicUsize::new(0),
            failed: AtomicUsize::new(usize::MAX),
            ended: AtomicBool::new(false),
            done: Mutex::new(BTreeMap::new()),
            landed: Condvar::new(),
            pool: Arc::new(Pool::default()),
        }
    }

    /// Verifies: [DESIGN] RULES.md D1246 the workers of a pool run one
    /// morsel fewer at once than a run's degree, and the reader's thread
    /// the last. Of a run of degree 3 with two workers each in a morsel, a
    /// third worker's step takes none and the run wants no worker, while
    /// the reader still takes the next morsel; of degree 2, one worker in
    /// a morsel is all it wants; and once a morsel ends a worker takes
    /// another.
    ///
    /// Decision: fewer workers run than the degree less one, or as many;
    /// the taker is a worker, or the reader.
    #[test]
    fn workers_take_one_morsel_fewer_than_the_degree() {
        for degree in [3_usize, 2] {
            let gate = Arc::new(Gate::default());
            let run = gated(64, degree, &gate);
            let workers = degree.saturating_sub(1);
            std::thread::scope(|s| {
                for inside in 1..=workers {
                    assert!(run.wanted(), "a worker is wanted below the cap, degree {degree}");
                    s.spawn(|| run.step());
                    // Each waits in its morsel before the next is asked.
                    gate.waiting(inside);
                }
                assert!(!run.wanted(), "no worker more is wanted, degree {degree}");
                run.step();
                assert_eq!(run.taken.load(Ordering::Acquire), workers, "a worker past the cap takes none, degree {degree}");
                assert_eq!(run.claim(), Some(workers), "the reader takes the next, degree {degree}");
                gate.open();
            });
            assert!(run.wanted(), "a worker is wanted again once the morsels ended");
            run.step();
            assert_eq!(run.taken.load(Ordering::Acquire), workers.saturating_add(2));
        }
    }

    /// Verifies: [DESIGN] RULES.md D1246, D290 a run of degree 1 wants no
    /// worker and is offered to none: its reader takes every morsel on
    /// its own thread, in order.
    #[test]
    fn a_run_of_one_thread_is_its_readers_alone() {
        let db = Database::new();
        let state = Db::from(db.snapshot().unwrap());
        let reader = std::thread::current().id();
        let work = Box::new(move |m: usize| Ok((m, std::thread::current().id() == reader)));
        let run = Morsels::start(&state, NO_TRANSACTION, (40, 1), work);
        assert!(!run.run.wanted(), "no worker is wanted");
        let taken: Vec<(usize, bool)> = run.collect::<Result<_>>().unwrap();
        assert_eq!(taken, (0..40).map(|m| (m, true)).collect::<Vec<_>>());
    }
}

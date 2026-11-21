//! \[DESIGN\] A run of morsels (RULES.md D290): work cut into numbered
//! morsels that the database's workers and the statement's own thread take
//! at once, each the next number not taken, by one atomic step, as the
//! dispatcher of Leis et al., "Morsel-Driven Parallelism" (2014), hands
//! them out. Their results are given back in the order of their numbers,
//! so what reads them sees what one thread taking them in turn would, the
//! rows before an error, then the error, as a statement run alone raises
//! it (RULES.md D15).
//!
//! No more than `degree` morsels run at once, nor more than `window`
//! past the one the reader waits for, so a reader that stops early, as a
//! cursor closed or a FETCH FIRST met does, leaves little read for
//! nothing. The reader takes morsels itself while it waits, so a run ends
//! though every worker is busy. Each morsel looks at the statement's
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

/// The work of each morsel, by its number.
type Work<T> = Box<dyn Fn(usize) -> Result<T> + Send + Sync>;

/// A run of morsels, shared by the workers taking them and its reader.
struct Run<T> {
    work: Work<T>,
    count: usize,
    degree: usize,
    window: usize,
    stop: Option<Arc<Stop>>,
    /// The number of the next morsel to take.
    taken: AtomicUsize,
    /// The morsels running now.
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
pub(super) struct Morsels<T: Send + 'static> {
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
    /// The number of a morsel it may take now, counted as running.
    fn claim(&self) -> Option<usize> {
        if self.ended.load(Ordering::Acquire) {
            return None;
        }
        below(&self.running, self.degree)?;
        let ahead = self.given.load(Ordering::Acquire).saturating_add(self.window);
        let limit = self.count.min(ahead).min(self.failed.load(Ordering::Acquire).saturating_add(1));
        let got = below(&self.taken, limit);
        if got.is_none() {
            self.running.fetch_sub(1, Ordering::AcqRel);
        }
        got
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
        self.running.fetch_sub(1, Ordering::AcqRel);
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
            && self.running.load(Ordering::Acquire) < self.degree
            && self.taken.load(Ordering::Acquire) < limit
    }

    fn finished(&self) -> bool {
        let limit = self.count.min(self.failed.load(Ordering::Acquire).saturating_add(1));
        self.ended.load(Ordering::Acquire) || self.taken.load(Ordering::Acquire) >= limit
    }

    fn step(&self) {
        if let Some(m) = self.claim() {
            self.run(m);
        }
    }
}

impl<T: Send + 'static> Morsels<T> {
    /// A run of `count` morsels, each `work` of its number, on up to
    /// `degree` threads of `db`'s workers and the statement's own, for the
    /// statement of `tx`.
    pub(super) fn start(db: &State, tx: TxId, (count, degree): (usize, usize), work: Work<T>) -> Morsels<T> {
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
            window: degree.saturating_mul(AHEAD_PER_THREAD),
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
            let _ = db.jobs.workers().offer(task, workers);
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

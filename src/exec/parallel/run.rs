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
//! met does, leaves little read for nothing. A reader that holds all its
//! input, as a grouping does, may ask a longer lead (`Lead`, RULES.md
//! D1262), taken only while the bytes of the results landed and not yet
//! given leave room, so its threads do not wait on the order of the
//! results behind one slow morsel. Each morsel looks at the statement's
//! cancellation first (RULES.md D44), and after one fails none past it is
//! taken. A panic in a morsel is its failure, so the reader never waits
//! for a morsel that will not end.
//!
//! A reader that drops its run waits for the morsels running to end,
//! each looking at the statement's cancellation on its rows, then lets
//! go of the work and of the results it did not read, so what they hold
//! of the grant is given back before the statement returns, not when a
//! worker next wakes (RULES.md D1245).

use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, RwLock};

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

/// \[DESIGN\] (RULES.md D1262) How far a run may lead its reader past the
/// window every run has: `wide` morsels a thread in place of
/// `AHEAD_PER_THREAD`, while the bytes of the results landed and not yet
/// given, each `size` of it, are fewer than `room` says there is for
/// them now. A reader that holds all its input, whose threads would
/// else wait on the order of the results behind one slow morsel, asks
/// it; the bytes pace the threads and decide nothing else.
pub(in crate::exec) struct Lead<T> {
    /// The morsels a thread, past the reader's.
    pub(in crate::exec) wide: usize,
    /// The bytes a result holds.
    pub(in crate::exec) size: fn(&T) -> usize,
    /// The bytes there is room for ahead of the reader, now.
    pub(in crate::exec) room: Box<dyn Fn() -> usize + Send + Sync>,
}

impl<T> Lead<T> {
    /// \[DESIGN\] (RULES.md D1282) The lead of a run whose reader keeps
    /// every result until it has the last, `wide` morsels a thread, its
    /// results counted as no bytes. What such a run finishes early its
    /// reader would hold a moment later all the same, so nothing paces
    /// it. A run over an operator's partitions asks it, so that the
    /// threads do not wait behind one partition that takes long.
    pub(in crate::exec) fn unmetered(wide: usize) -> Lead<T> {
        Lead { wide, size: |_| 0, room: Box::new(|| usize::MAX) }
    }
}

/// A run's lead, and the bytes it holds ahead of its reader now.
struct Led<T> {
    lead: Lead<T>,
    held: AtomicUsize,
}

/// A run of morsels, shared by the workers taking them and its reader.
struct Run<T> {
    /// The work of each morsel, read by each morsel running and let go
    /// by the reader once none is.
    work: RwLock<Option<Work<T>>>,
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
    /// Its lead past the window, if its reader asked one.
    led: Option<Led<T>>,
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
        below(&self.taken, self.limit())
    }

    /// The number past the last morsel that may be taken now: within the
    /// window past the one the reader waits for, or the lead its reader
    /// asked while the bytes held ahead leave room (RULES.md D1262), and
    /// not past one that failed.
    fn limit(&self) -> usize {
        let roomy = |led: &&Led<T>| led.held.load(Ordering::Acquire) < (led.lead.room)();
        let reach = self.led.as_ref().filter(roomy).map_or(self.window, |led| self.degree.saturating_mul(led.lead.wide).max(self.window));
        let ahead = self.given.load(Ordering::Acquire).saturating_add(reach);
        self.count.min(ahead).min(self.failed.load(Ordering::Acquire).saturating_add(1))
    }

    /// \[DESIGN\] (RULES.md D1246) The morsels the pool's workers run at
    /// once at most: one fewer than the degree. The reader's thread is the
    /// last of the degree's whatever it does, a morsel as it waits or its
    /// own work on the results between two of them, so no more threads
    /// than the degree are busy for the run.
    fn workers(&self) -> usize {
        self.degree.saturating_sub(1)
    }

    /// Runs morsel `m`, claimed, and keeps its result; nothing if its
    /// reader has let go of the work.
    fn run(&self, m: usize) {
        let held = self.work.read().unwrap_or_else(PoisonError::into_inner);
        if let Some(work) = held.as_ref() {
            let checked = self.stop.as_ref().map_or(Ok(()), |s| s.check_now());
            let result = checked.and_then(|()| {
                catch_unwind(AssertUnwindSafe(|| work(m)))
                    .unwrap_or_else(|_| Err(SqlError::new(Code::Api, "[DESIGN] a morsel of a parallel query failed")))
            });
            if result.is_err() {
                self.failed.fetch_min(m, Ordering::AcqRel);
            }
            if let Some(led) = &self.led {
                led.held.fetch_add(self.bytes_of(&result), Ordering::AcqRel);
            }
            done_of(self).insert(m, result);
        }
        drop(held);
        self.landed.notify_all();
    }

    /// The bytes `result` holds ahead of the reader, of a run that leads.
    fn bytes_of(&self, result: &Result<T>) -> usize {
        self.led.as_ref().zip(result.as_ref().ok()).map_or(0, |(led, landed)| (led.lead.size)(landed))
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
                if let Some(led) = &self.led {
                    led.held.fetch_sub(self.bytes_of(&result), Ordering::AcqRel);
                }
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
        !self.ended.load(Ordering::Acquire)
            && self.running.load(Ordering::Acquire) < self.workers()
            && self.taken.load(Ordering::Acquire) < self.limit()
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
    pub(in crate::exec) fn start(db: &State, tx: TxId, run: (usize, usize), work: Work<T>) -> Morsels<T> {
        Morsels::led(db, tx, run, work, None)
    }

    /// \[DESIGN\] (RULES.md D1262) As `start`, the run leading its reader
    /// as `lead` says, if given.
    pub(in crate::exec) fn led(db: &State, tx: TxId, (count, degree): (usize, usize), work: Work<T>, lead: Option<Lead<T>>) -> Morsels<T> {
        let degree = degree.max(1);
        // [EXTENSION] the notes of the operator that starts it, each
        // worker's work noted of it (RULES.md D620).
        let work: Work<T> = match noted::noting() {
            Some(notes) => Box::new(move |m| noted::within(Some(Arc::clone(&notes)), || work(m))),
            None => work,
        };
        let run = Arc::new(Run {
            work: RwLock::new(Some(work)),
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
            led: lead.map(|lead| Led { lead, held: AtomicUsize::new(0) }),
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
        // Woken, the idle workers let go of the run.
        self.run.pool.nudge();
        // Once the morsels running end, as a cancelled statement's do at
        // their next row, the work and the results not read are let go
        // here, what they hold given back before the statement goes on.
        // The wait is for morsels running on workers alone. The reader's
        // thread, which is the one dropping, runs a morsel only inside
        // `next`, and the last slot the cap leaves it (RULES.md D1246) is
        // a slot, not a morsel kept for it: a morsel is taken by the
        // thread that runs it at once. A worker paced by the lead (RULES.md
        // D1262) is in no morsel either, as it takes none and goes back to
        // the pool, which lets go of a run that has ended.
        let work = self.run.work.write().unwrap_or_else(PoisonError::into_inner).take();
        drop(work);
        let unread = std::mem::take(&mut *done_of(&self.run));
        drop(unread);
        if let Some(led) = &self.run.led {
            led.held.store(0, Ordering::Release);
        }
    }
}

// Unit tests: who takes a run's morsels, how far it leads its reader, and
// what a run dropped lets go of (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests;

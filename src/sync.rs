//! \[DESIGN\] Locks on state that threads share (RULES.md D27, R-ERR-4).
//!
//! A `std` lock is poisoned when a thread panics while holding it: the data
//! it guards may be half changed. Production code does not panic (R-ERR-1),
//! so poisoning means a plug-in panicked. Taking a poisoned lock is reported
//! as SQLCODE -910 rather than unwrapped, so the database refuses to go on
//! with state it cannot trust.
//!
//! And the gate checkpoints shut (`Gate`), and the stripe of each thread
//! (`thread_stripe`).

use std::sync::{Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::error::{Code, SqlError};

/// The error for a lock whose holder panicked.
fn poisoned<T>(_: PoisonError<T>) -> SqlError {
    SqlError::new(Code::Api, "[DESIGN] a thread panicked while it held shared state, which may be inconsistent (RULES.md D27)")
}

/// Locks `mutex`.
///
/// # Errors
///
/// SQLCODE -910 if a thread panicked while holding it.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, SqlError> {
    mutex.lock().map_err(poisoned)
}

/// Takes `lock` for reading.
///
/// # Errors
///
/// SQLCODE -910 if a thread panicked while writing it.
pub(crate) fn read<T>(lock: &RwLock<T>) -> Result<RwLockReadGuard<'_, T>, SqlError> {
    lock.read().map_err(poisoned)
}

/// Takes `lock` for writing.
///
/// # Errors
///
/// SQLCODE -910 if a thread panicked while writing it.
pub(crate) fn write<T>(lock: &RwLock<T>) -> Result<RwLockWriteGuard<'_, T>, SqlError> {
    lock.write().map_err(poisoned)
}

/// Locks a mutex of a built-in engine or index that runs no plug-in code
/// while holding it, so no panic can poison it; if one somehow did, the data
/// is used as it is. For methods whose signature cannot report an error.
pub(crate) fn lock_builtin<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

// Unit tests: a lock poisoned by a panicking thread is reported, not
// unwrapped (RULES.md R-ERR-1, R-TEST-2 robustness).
#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex, RwLock};
    use std::thread;

    use super::*;

    /// Runs `hold`, which panics while holding a lock, on another thread.
    fn poison(hold: impl FnOnce() + Send + 'static) {
        assert!(thread::spawn(hold).join().is_err(), "the holder panicked");
    }

    /// Verifies: [DESIGN] RULES.md D27: taking a mutex or a read-write lock
    /// whose holder panicked gives SQLCODE -910; a built-in's own lock is used
    /// as it is.
    #[test]
    fn poisoned_locks_are_reported() {
        let mutex = Arc::new(Mutex::new(0_u8));
        let held = Arc::clone(&mutex);
        poison(move || drop((held.lock(), panic_now())));
        assert_eq!(lock(&mutex).err().map(|e| e.code), Some(Code::Api));
        assert_eq!(*lock_builtin(&mutex), 0);

        let rw = Arc::new(RwLock::new(0_u8));
        let writer = Arc::clone(&rw);
        poison(move || drop((writer.write(), panic_now())));
        assert_eq!(read(&rw).err().map(|e| e.code), Some(Code::Api));
        assert_eq!(write(&rw).err().map(|e| e.code), Some(Code::Api));
    }

    /// Panics, as a plug-in might.
    fn panic_now() {
        panic!("a plug-in panicked");
    }
}

/// Stripes of a `Gate`: threads taking it shared rarely share one.
const GATE_STRIPES: u64 = 16;

/// The stripe the next thread to take a gate is given.
static NEXT_STRIPE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

std::thread_local! {
    /// The gates this thread holds, by address: taking one again is free.
    static HELD: std::cell::RefCell<Vec<usize>> = const { std::cell::RefCell::new(Vec::new()) };
    /// This thread's stripe of every gate: given in turn, so threads at
    /// once take different stripes.
    static STRIPE: u64 = NEXT_STRIPE.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % GATE_STRIPES;
}

/// A stripe, alone on its cache line, so taking one never slows a thread
/// taking another.
#[repr(align(128))]
#[derive(Default)]
struct Stripe(RwLock<()>);

/// \[DESIGN\] A lock taken shared by every operation that changes what a
/// checkpoint writes, and exclusively by a checkpoint (`db/checkpoint.rs`)
/// and an index build as it publishes (`db/indexes.rs`), so neither sees
/// an operation half done. A shared holder takes one of
/// several stripes, given to its thread in turn, each on a cache line of
/// its own, so holders on different threads rarely meet; the exclusive holder takes them all, in order. A thread
/// holding the gate takes it again without waiting, as a checkpoint
/// waiting for it would otherwise wait for itself. It guards no data, so a
/// panic while holding it poisons nothing that matters.
pub(crate) struct Gate {
    stripes: Box<[Stripe]>,
}

impl Default for Gate {
    fn default() -> Self {
        Gate { stripes: (0..GATE_STRIPES).map(|_| Stripe::default()).collect() }
    }
}

/// The gate held shared, until dropped.
pub(crate) struct Entered<'a> {
    held: Option<(usize, RwLockReadGuard<'a, ()>)>,
}

impl Drop for Entered<'_> {
    fn drop(&mut self) {
        if let Some((gate, _)) = &self.held {
            HELD.with(|held| {
                let mut held = held.borrow_mut();
                if let Some(at) = held.iter().rposition(|g| g == gate) {
                    held.swap_remove(at);
                }
            });
        }
    }
}

/// The gate held exclusively, until dropped. Its thread counts as holding
/// it, so taking it shared there does not wait.
pub(crate) struct Shut<'a> {
    gate: usize,
    _stripes: Vec<RwLockWriteGuard<'a, ()>>,
}

impl Drop for Shut<'_> {
    fn drop(&mut self) {
        HELD.with(|held| {
            let mut held = held.borrow_mut();
            if let Some(at) = held.iter().rposition(|g| *g == self.gate) {
                held.swap_remove(at);
            }
        });
    }
}

impl Gate {
    fn address(&self) -> usize {
        std::ptr::from_ref(self).addr()
    }

    /// Takes the gate shared, unless this thread holds it already.
    pub(crate) fn enter(&self) -> Entered<'_> {
        let me = self.address();
        if HELD.with(|held| held.borrow().contains(&me)) {
            return Entered { held: None };
        }
        let stripe = usize::try_from(STRIPE.with(|s| *s)).ok().and_then(|i| self.stripes.get(i));
        let Some(stripe) = stripe else { return Entered { held: None } };
        let guard = stripe.0.read().unwrap_or_else(PoisonError::into_inner);
        HELD.with(|held| held.borrow_mut().push(me));
        Entered { held: Some((me, guard)) }
    }

    /// Takes the gate exclusively, waiting for every shared holder. The
    /// thread must not hold it shared.
    pub(crate) fn shut(&self) -> Shut<'_> {
        let stripes = self.stripes.iter().map(|s| s.0.write().unwrap_or_else(PoisonError::into_inner)).collect();
        let gate = self.address();
        HELD.with(|held| held.borrow_mut().push(gate));
        Shut { gate, _stripes: stripes }
    }
}

/// \[DESIGN\] The thread's stripe of `stripes`: given in turn as threads
/// first ask, so threads working at once seldom share one, for counters
/// and marks kept by stripe, each on cache lines of its own.
pub(crate) fn thread_stripe(stripes: usize) -> usize {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    thread_local! {
        static GIVEN: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    }
    let given = GIVEN.with(|given| {
        given.get().unwrap_or_else(|| {
            let next = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            given.set(Some(next));
            next
        })
    });
    given.checked_rem(stripes).unwrap_or_default()
}

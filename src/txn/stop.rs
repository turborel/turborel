//! \[DESIGN\] Cancelling a running statement (RULES.md D44). A session's
//! `Stop` is shared with a `Canceller` any thread may hold. Each call into
//! the session, and each fetch of its cursors, is a window: the first
//! window opened while none is takes a new generation; a cancel marks the
//! generation running then, and only it, so one that arrives as a statement
//! ends never stops the next.
//!
//! A statement looks at it as it reads each row of a table or an index
//! (`exec.rs`), and while it waits for a lock (`txn/locks.rs`): a
//! cancelled one fails there (SQLCODE -920), and is undone as any failed
//! statement is [X3.135-1986 §7.3 GR3b i]; its transaction goes on.
//!
//! A generation may have a deadline, STATEMENT_TIMEOUT after it begins
//! (`set_timeout`): past it, the generation is cancelled as by a cancel,
//! and fails saying so. The clock is read every `CLOCK_EVERY` looks as rows
//! are read, so a row costs no more than a count, and at every look while
//! a lock is waited for.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

use crate::error::{Code, Result, SqlError};

/// The flag a session's statements look at.
#[derive(Debug)]
pub struct Stop {
    /// The windows open now.
    open: AtomicUsize,
    /// The generation of the windows open now, or of the last.
    generation: AtomicU64,
    /// The generation a cancel marked.
    cancelled: AtomicU64,
    /// The generation its deadline passed in.
    expired: AtomicU64,
    /// STATEMENT_TIMEOUT, in milliseconds, for the generations that begin
    /// from now on; 0 for none.
    timeout: AtomicU64,
    /// LOCK_TIMEOUT, in milliseconds; 0 for none.
    lock_timeout: AtomicU64,
    /// The deadline of the generation running, in nanoseconds since
    /// `epoch`; 0 for none.
    deadline: AtomicU64,
    /// The looks since the clock was last read.
    looks: AtomicU64,
    epoch: Instant,
}

/// \[DESIGN\] Looks at a `Stop` as rows are read between two readings of
/// the clock: a reading costs as much as reading a row or two, a look a
/// count.
const CLOCK_EVERY: u64 = 64;

impl Default for Stop {
    fn default() -> Self {
        Stop {
            open: AtomicUsize::new(0),
            generation: AtomicU64::new(1),
            cancelled: AtomicU64::new(0),
            expired: AtomicU64::new(0),
            timeout: AtomicU64::new(0),
            lock_timeout: AtomicU64::new(0),
            deadline: AtomicU64::new(0),
            looks: AtomicU64::new(0),
            epoch: Instant::now(),
        }
    }
}

/// The error of a statement cancelled (SQLCODE -920).
fn cancelled() -> SqlError {
    SqlError::new(Code::Cancelled, "[DESIGN] the statement was cancelled")
}

/// The error of a statement past its deadline (SQLCODE -920).
fn expired(timeout: u64) -> SqlError {
    SqlError::new(Code::Cancelled, format!("[DESIGN] the statement ran longer than STATEMENT_TIMEOUT, {timeout} ms, and was cancelled"))
}

impl Stop {
    /// Opens a window; closed when the window returned is dropped.
    pub(crate) fn window(&self) -> Window<'_> {
        if self.open.fetch_add(1, Ordering::SeqCst) == 0 {
            let timeout = self.timeout.load(Ordering::SeqCst);
            let deadline = (timeout > 0).then(|| self.now().saturating_add(timeout.saturating_mul(NANOS_PER_MILLI)));
            self.deadline.store(deadline.unwrap_or(0), Ordering::SeqCst);
            self.generation.fetch_add(1, Ordering::SeqCst);
        }
        Window(self)
    }

    /// Makes `millis`, if not 0, the deadline of each generation that
    /// begins from now on, after it begins (STATEMENT_TIMEOUT).
    pub(crate) fn set_timeout(&self, millis: u64) {
        self.timeout.store(millis, Ordering::SeqCst);
    }

    /// Makes `millis`, if not 0, the longest a statement from now on waits
    /// for a lock (LOCK_TIMEOUT).
    pub(crate) fn set_lock_timeout(&self, millis: u64) {
        self.lock_timeout.store(millis, Ordering::SeqCst);
    }

    /// The longest a statement waits for a lock, if there is a limit.
    pub(crate) fn lock_timeout(&self) -> Option<std::time::Duration> {
        let millis = self.lock_timeout.load(Ordering::SeqCst);
        (millis > 0).then(|| std::time::Duration::from_millis(millis))
    }

    /// Nanoseconds since `epoch`, never 0.
    fn now(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_nanos()).unwrap_or(u64::MAX).max(1)
    }

    /// Cancels what runs in the windows open now, if any is.
    pub fn cancel(&self) {
        // The generation is read first: if the windows close and others
        // open after, it is theirs no more, and they are not cancelled.
        let generation = self.generation.load(Ordering::SeqCst);
        if self.open.load(Ordering::SeqCst) > 0 {
            self.cancelled.store(generation, Ordering::SeqCst);
        }
    }

    /// Whether what runs now was cancelled, or ran past its deadline: the
    /// clock read every `CLOCK_EVERY` looks.
    ///
    /// # Errors
    ///
    /// SQLCODE -920 if it was, or did.
    pub(crate) fn check(&self) -> Result<()> {
        let read_clock = self.looks.fetch_add(1, Ordering::Relaxed).is_multiple_of(CLOCK_EVERY);
        self.looked(read_clock)
    }

    /// As `check`, the clock read now: as a wait for a lock looks.
    ///
    /// # Errors
    ///
    /// As `check`.
    pub(crate) fn check_now(&self) -> Result<()> {
        self.looked(true)
    }

    fn looked(&self, read_clock: bool) -> Result<()> {
        if self.open.load(Ordering::SeqCst) == 0 {
            return Ok(());
        }
        let generation = self.generation.load(Ordering::SeqCst);
        if self.cancelled.load(Ordering::SeqCst) == generation {
            return Err(cancelled());
        }
        let deadline = self.deadline.load(Ordering::SeqCst);
        let passed = self.expired.load(Ordering::SeqCst) == generation || (read_clock && deadline > 0 && self.now() >= deadline);
        if passed {
            self.expired.store(generation, Ordering::SeqCst);
            return Err(expired(self.timeout.load(Ordering::SeqCst)));
        }
        Ok(())
    }
}

/// Nanoseconds in a millisecond.
const NANOS_PER_MILLI: u64 = 1_000_000;

/// An open window of a `Stop`.
pub(crate) struct Window<'a>(&'a Stop);

impl Drop for Window<'_> {
    fn drop(&mut self) {
        self.0.open.fetch_sub(1, Ordering::SeqCst);
    }
}

/// `rows`, each read only while `stop`, if any, is not cancelled, and
/// taken only if it was not cancelled as the row was read. From a cancel
/// on, every read is its error: the rows were cut short there, and a row
/// read as it came is not taken, so none may seem to be the last.
pub(crate) fn checked<T>(
    rows: impl Iterator<Item = Result<T>> + Send + 'static,
    stop: Option<std::sync::Arc<Stop>>,
) -> impl Iterator<Item = Result<T>> + Send + 'static
where
    T: Send + 'static,
{
    let mut cut: Option<SqlError> = None;
    let mut rows = rows;
    std::iter::from_fn(move || {
        if let Some(e) = &cut {
            return Some(Err(e.clone()));
        }
        let looked = |flag: &Option<std::sync::Arc<Stop>>| flag.as_deref().map_or(Ok(()), Stop::check);
        let read = looked(&stop).and_then(|()| {
            let row = rows.next();
            looked(&stop).map(|()| row)
        });
        match read {
            Ok(row) => row,
            Err(e) => {
                cut = Some(e.clone());
                Some(Err(e))
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(stop: &Stop) -> Option<Code> {
        stop.check().err().map(|e| e.code)
    }

    /// Verifies: [DESIGN] RULES.md D44: a cancel stops what runs in the
    /// windows open as it is made, until they close; one made while none
    /// is open stops nothing; the next window runs on; windows open at once
    /// are cancelled together.
    #[test]
    fn a_cancel_stops_only_what_runs_as_it_is_made() {
        let stop = Stop::default();
        stop.cancel();
        let first = stop.window();
        assert_eq!(code(&stop), None, "a cancel with nothing running");
        let second = stop.window();
        stop.cancel();
        assert_eq!(code(&stop), Some(Code::Cancelled));
        drop(second);
        assert_eq!(code(&stop), Some(Code::Cancelled), "the other window still runs");
        drop(first);
        assert_eq!(code(&stop), None, "none runs");
        let _next = stop.window();
        assert_eq!(code(&stop), None, "the next statement runs on");
    }

    /// Verifies: [DESIGN] RULES.md D44: rows read under a cancel end with
    /// its error, and every read after is it too; with no stop, every row
    /// is read.
    #[test]
    fn rows_end_at_a_cancel() {
        let stop = std::sync::Arc::new(Stop::default());
        let _window = stop.window();
        let mut rows = checked((1_i32..=3_i32).map(Ok::<i32, SqlError>), Some(std::sync::Arc::clone(&stop)));
        assert_eq!(rows.next().map(Result::ok), Some(Some(1_i32)));
        stop.cancel();
        assert_eq!(rows.next().map(|r| r.err().map(|e| e.code)), Some(Some(Code::Cancelled)));
        assert_eq!(rows.next().map(|r| r.err().map(|e| e.code)), Some(Some(Code::Cancelled)), "cut short, never ended");
        assert_eq!(checked((1_i32..=3_i32).map(Ok::<i32, SqlError>), None).count(), 3);
    }
}

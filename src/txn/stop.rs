//! \[DESIGN\] Cancelling a running statement (RULES.md D44). A session's
//! `Stop` is shared with a `Canceller` any thread may hold. Each call into
//! the session, and each fetch of its cursors, is a window: the first
//! window opened while none is takes a new generation; a cancel marks the
//! generation running then, and only it, so one that arrives as a statement
//! ends never stops the next.
//!
//! A statement looks at it as it reads each row of a table or an index
//! (`exec.rs`), or each batch of a table's (`exec/batch/stream.rs`), as an
//! operator reads back each record it spilled (`exec/work.rs`), and while
//! it waits for a lock (`txn/locks.rs`): a
//! cancelled one fails there (SQLCODE -920), and is undone as any failed
//! statement is [X3.135-1986 §7.3 GR3b i]; its transaction goes on.
//!
//! A generation may have a deadline, STATEMENT.TIMEOUT_MS after it begins
//! (`set_timeout`): past it, the generation is cancelled as by a cancel,
//! and fails saying so. The clock is read every `CLOCK_EVERY` looks as rows
//! are read, so a row costs no more than a count, and at every look while
//! a lock is waited for.
//!
//! It holds the warnings the session's statements raise as they evaluate
//! expressions, too (`error::Warnings`): what a statement reaches through
//! its transaction, as it reaches the flag, wherever its rows are pulled;
//! and what a server takes of them for its client (`stop/served.rs`); and
//! the plan of a data change EXPLAIN ANALYZE runs (`stop/changed.rs`).

mod activity;
mod changed;
mod served;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::TxId;
use crate::error::{Code, Result, SqlError, Warnings};
use crate::sync::lock_builtin;

pub use activity::{Activity, Now, Tallied, Tallies, Tally, Waiting};
pub(crate) use changed::Changed;
pub use served::{MAX_NOTES, Served};

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
    /// STATEMENT.TIMEOUT_MS, in milliseconds, for the generations that begin
    /// from now on; 0 for none.
    timeout: AtomicU64,
    /// LOCKS.TIMEOUT_MS, in milliseconds; 0 for none.
    lock_timeout: AtomicU64,
    /// The deadline of the generation running, in nanoseconds since
    /// `epoch`; 0 for none.
    deadline: AtomicU64,
    /// The looks since the clock was last read.
    looks: AtomicU64,
    epoch: Instant,
    /// The warnings the statement running raised [ISO/IEC 9075:1992
    /// §3.3.4.1].
    warnings: Warnings,
    /// What a server takes of its statements for its client.
    served: Served,
    /// \[DESIGN\] Whether its transactions leave their garbage to be
    /// collected once their reply is sent (RULES.md D450).
    collect_later: AtomicBool,
    /// The plan of a data change EXPLAIN ANALYZE runs.
    changed: Changed,
    /// What the session does now, and what its statements did.
    activity: Activity,
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
            warnings: Warnings::default(),
            served: Served::default(),
            collect_later: AtomicBool::new(false),
            changed: Changed::default(),
            activity: Activity::default(),
        }
    }
}

/// The error of a statement cancelled (SQLCODE -920).
fn cancelled() -> SqlError {
    SqlError::new(Code::Cancelled, "[DESIGN] the statement was cancelled")
}

/// The error of a statement past its deadline (SQLCODE -920).
fn expired(timeout: u64) -> SqlError {
    SqlError::new(Code::Cancelled, format!("[DESIGN] the statement ran longer than STATEMENT.TIMEOUT_MS, {timeout} ms, and was cancelled"))
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
    /// begins from now on, after it begins (STATEMENT.TIMEOUT_MS).
    pub(crate) fn set_timeout(&self, millis: u64) {
        self.timeout.store(millis, Ordering::SeqCst);
    }

    /// Makes `millis`, if not 0, the longest a statement from now on waits
    /// for a lock (LOCKS.TIMEOUT_MS).
    pub(crate) fn set_lock_timeout(&self, millis: u64) {
        self.lock_timeout.store(millis, Ordering::SeqCst);
    }

    /// \[DESIGN\] Sets whether its transactions leave their garbage to be
    /// collected once their reply is sent (RULES.md D450).
    pub(crate) fn set_collect_later(&self, later: bool) {
        self.collect_later.store(later, Ordering::Relaxed);
    }

    /// \[DESIGN\] Whether its transactions' ends leave their tables noted
    /// for collection, which their session runs once its reply is sent
    /// (RULES.md D450).
    pub(crate) fn collects_later(&self) -> bool {
        self.collect_later.load(Ordering::Relaxed)
    }

    /// The longest a statement waits for a lock, if there is a limit.
    pub(crate) fn lock_timeout(&self) -> Option<std::time::Duration> {
        let millis = self.lock_timeout.load(Ordering::SeqCst);
        (millis > 0).then(|| std::time::Duration::from_millis(millis))
    }

    /// \[DESIGN\] What a server takes of the session's statements for its
    /// client: the last commit's position, the trace context, the notices
    /// (RULES.md D187, D191, D194).
    pub fn served(&self) -> &Served {
        &self.served
    }

    /// \[DESIGN\] What the session does now, and what its statements did
    /// (RULES.md D520, D521).
    pub fn activity(&self) -> &Activity {
        &self.activity
    }

    /// The warnings the session's statement raises as it runs, taken as it
    /// completes.
    pub(crate) fn warnings(&self) -> &Warnings {
        &self.warnings
    }

    /// \[EXTENSION\] The plan of a data change EXPLAIN ANALYZE runs
    /// (RULES.md D622).
    pub(crate) fn changed(&self) -> &Changed {
        &self.changed
    }

    /// Nanoseconds since `epoch`, never 0.
    fn now(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_nanos()).unwrap_or(u64::MAX).max(1)
    }

    /// The generation of the windows open now, or of the last: a reader
    /// that sees it change is in another window.
    pub(crate) fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
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

/// \[DESIGN\] The shards a database's map of open transactions' flags is
/// split in, by transaction id, so that transactions beginning and ending
/// on many threads at once seldom take the same lock.
const STOP_SHARDS: u64 = 32;

/// \[DESIGN\] What cancels the statements of each open transaction, for a
/// plan's scans to look at: in `STOP_SHARDS` shards, by transaction id.
pub(crate) struct Stops {
    shards: Vec<Mutex<HashMap<TxId, Arc<Stop>>>>,
}

impl Default for Stops {
    fn default() -> Self {
        Stops { shards: (0..STOP_SHARDS).map(|_| Mutex::new(HashMap::new())).collect() }
    }
}

impl Stops {
    /// The shard of `tx`, if there is one.
    fn shard(&self, tx: TxId) -> Option<&Mutex<HashMap<TxId, Arc<Stop>>>> {
        usize::try_from(tx % STOP_SHARDS).ok().and_then(|at| self.shards.get(at))
    }

    /// Notes `stop` as what cancels `tx`'s statements.
    pub(crate) fn insert(&self, tx: TxId, stop: Arc<Stop>) {
        stop.activity.began_transaction(tx);
        if let Some(shard) = self.shard(tx) {
            lock_builtin(shard).insert(tx, stop);
        }
    }

    /// What cancels the statements of `tx`, if it is open.
    pub(crate) fn of(&self, tx: TxId) -> Option<Arc<Stop>> {
        self.shard(tx).and_then(|shard| lock_builtin(shard).get(&tx).cloned())
    }

    /// Forgets `tx`'s, as it ends.
    pub(crate) fn remove(&self, tx: TxId) {
        let removed = self.shard(tx).and_then(|shard| lock_builtin(shard).remove(&tx));
        if let Some(stop) = removed {
            stop.activity.ended_transaction(tx);
        }
    }
}

/// `rows`, each read only while `stop`, if any, is not cancelled, and
/// taken only if it was not cancelled as the row was read. From a cancel
/// on, every read is its error: the rows were cut short there, and a row
/// read as it came is not taken, so none may seem to be the last.
pub(crate) fn checked<T>(
    rows: impl Iterator<Item = Result<T>> + Send + 'static,
    stop: Option<Arc<Stop>>,
) -> impl Iterator<Item = Result<T>> + Send + 'static
where
    T: Send + 'static,
{
    counted(rows, stop, |_| 0)
}

/// \[DESIGN\] `checked`, each item of `rows` weighed by `weigh` as rows
/// read: a row of a table or an index one, a batch its rows. The weights
/// are added up as the rows are read and given to `stop`'s session's
/// count once they are dropped (`Activity::read`), so a row read costs no
/// atomic count (RULES.md D521).
pub(crate) fn counted<T>(
    rows: impl Iterator<Item = Result<T>> + Send + 'static,
    stop: Option<Arc<Stop>>,
    weigh: fn(&T) -> u64,
) -> impl Iterator<Item = Result<T>> + Send + 'static
where
    T: Send + 'static,
{
    Checked { rows, stop, weigh, read: 0, cut: None }
}

/// The rows of `counted`.
struct Checked<I, T> {
    rows: I,
    stop: Option<Arc<Stop>>,
    weigh: fn(&T) -> u64,
    /// Their weights, not yet given to the count.
    read: u64,
    /// The error that cut them short, if one did.
    cut: Option<SqlError>,
}

impl<I: Iterator<Item = Result<T>>, T> Iterator for Checked<I, T> {
    type Item = Result<T>;

    fn next(&mut self) -> Option<Result<T>> {
        if let Some(e) = &self.cut {
            return Some(Err(e.clone()));
        }
        let looked = |flag: &Option<Arc<Stop>>| flag.as_deref().map_or(Ok(()), Stop::check);
        let read = looked(&self.stop).and_then(|()| {
            let row = self.rows.next();
            looked(&self.stop).map(|()| row)
        });
        match read {
            Ok(row) => {
                match &row {
                    Some(Ok(row)) => self.read = self.read.saturating_add((self.weigh)(row)),
                    Some(Err(_)) => {}
                    // The last read: counted now, not once the rows are
                    // dropped, which may be after their statement ends.
                    None => self.flush(),
                }
                row
            }
            Err(e) => {
                self.cut = Some(e.clone());
                Some(Err(e))
            }
        }
    }
}

impl<I, T> Checked<I, T> {
    /// Gives the weights read to the count.
    fn flush(&mut self) {
        if let Some(stop) = self.stop.as_deref().filter(|_| self.read > 0) {
            stop.activity.read(std::mem::take(&mut self.read));
        }
    }
}

impl<I, T> Drop for Checked<I, T> {
    fn drop(&mut self) {
        self.flush();
    }
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
        let stop = Arc::new(Stop::default());
        let _window = stop.window();
        let mut rows = checked((1_i32..=3_i32).map(Ok::<i32, SqlError>), Some(Arc::clone(&stop)));
        assert_eq!(rows.next().map(Result::ok), Some(Some(1_i32)));
        stop.cancel();
        assert_eq!(rows.next().map(|r| r.err().map(|e| e.code)), Some(Some(Code::Cancelled)));
        assert_eq!(rows.next().map(|r| r.err().map(|e| e.code)), Some(Some(Code::Cancelled)), "cut short, never ended");
        assert_eq!(checked((1_i32..=3_i32).map(Ok::<i32, SqlError>), None).count(), 3);
    }
}

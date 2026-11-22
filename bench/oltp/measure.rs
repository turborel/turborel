//! How a case is measured: its terminals, each on a thread of its own and
//! a session of its own, run for the set seconds, once untimed and then
//! five times, on the harness, which reports the median transactions per
//! second. Beside it go the figures the timed runs make together.
//!
//! The time per transaction is portable only as wall time: the standard
//! library reads no processor time. It is reported as the time of the
//! cores the terminals could keep busy, the wall time times the lesser of
//! the threads and the processor's parallelism, per transaction. With
//! commits that sync, that includes the waits for the disk, which
//! `TURBOREL_OLTP_SYNC=off` takes out; then, at no more threads than
//! cores, it is the processor time a transaction costs, a bound above it
//! otherwise. Allocations are not counted: that needs a global allocator
//! of the bench's own, which is unsafe code.

use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use crate::client::Client;
use crate::harness::note;

/// The figures of a case's timed runs, together.
#[derive(Default)]
pub struct Totals {
    /// The runs so far, the untimed one included.
    pub runs: usize,
    /// The timed runs' wall time.
    pub wall: Duration,
    /// Their transactions.
    pub transactions: u64,
}

impl Totals {
    /// Counts a run of `transactions` in `wall`; whether it was timed.
    pub fn count(&mut self, wall: Duration, transactions: u64) -> bool {
        self.runs += 1;
        if self.runs == 1 {
            return false;
        }
        self.wall += wall;
        self.transactions += transactions;
        true
    }

    /// The busy cores' microseconds per transaction at `threads`.
    pub fn micros_per_transaction(&self, threads: usize) -> f64 {
        let cores = thread::available_parallelism().map_or(1, usize::from);
        self.wall.as_secs_f64() * threads.min(cores) as f64 * 1e6_f64 / self.transactions.max(1) as f64
    }

    /// Prints the transactions' cost.
    pub fn report(&self, threads: usize) {
        let cores = thread::available_parallelism().map_or(1, usize::from);
        note(&format!(
            "{:.1} us a transaction of {} busy cores (wall x min(threads, {cores} cores)), {} transactions in {:.1} s",
            self.micros_per_transaction(threads),
            threads.min(cores),
            self.transactions,
            self.wall.as_secs_f64()
        ));
    }
}

/// Runs `terminal(t, until)` for `t` in `0..threads`, each on a thread of
/// its own, until `secs` from now: what each gave, and the wall time.
pub fn on_threads<T: Send>(threads: usize, secs: Duration, terminal: impl Fn(usize, Instant) -> T + Sync) -> (Vec<T>, Duration) {
    let results = Mutex::new(Vec::with_capacity(threads));
    let started = Instant::now();
    let until = started + secs;
    thread::scope(|s| {
        for t in 0..threads {
            let (terminal, results) = (&terminal, &results);
            s.spawn(move || {
                let done = terminal(t, until);
                results.lock().unwrap().push(done);
            });
        }
    });
    (results.into_inner().unwrap(), started.elapsed())
}

/// Prints the calls of each statement whose text is `like`, and the rows
/// each call wrote and read, every run counted, as the database's statement
/// statistics count them.
pub fn statement_rows(client: &mut impl Client, like: &str) {
    let sql = format!(
        "SELECT STATEMENT_TEXT, CALLS, ROW_COUNT, ROWS_READ FROM INFORMATION_SCHEMA.STATEMENT_STATISTICS \
            WHERE UPPER(STATEMENT_TEXT) LIKE '{like}' ORDER BY STATEMENT_TEXT"
    );
    let Ok(found) = client.direct(&sql, true) else {
        note("no statement statistics");
        return;
    };
    for row in found {
        let number = |i: usize| row[i].as_f64().unwrap_or(0.0_f64);
        let text = if let turborel::value::Value::Char(s) = &row[0] { s.as_str() } else { "" };
        let calls = number(1).max(1.0_f64);
        note(&format!("{text}: {:.0} calls, {:.2} rows a call, {:.2} read", number(1), number(2) / calls, number(3) / calls));
    }
}
